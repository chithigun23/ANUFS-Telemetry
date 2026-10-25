package au.anufs.telemetry.core

import kotlin.math.PI
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.sqrt

/**
 * What the fusion reads, one value per sample, NaN where a source has nothing. Positions are
 * degrees, speeds m/s, courses degrees clockwise from north, altitudes metres.
 */
class FusionInputs(
    val t: DoubleArray,
    val onLat: DoubleArray,
    val onLon: DoubleArray,
    val onSog: DoubleArray,
    val onCog: DoubleArray,
    val onHdop: DoubleArray,
    val onFixQuality: DoubleArray,
    val onAlt: DoubleArray,
    val elLat: DoubleArray?,
    val elLon: DoubleArray?,
    val elVelN: DoubleArray?,
    val elVelE: DoubleArray?,
    val elAlt: DoubleArray?,
    /** Accelerometers that may aid the filter; each is checked against the GNSS motion first. */
    val imus: List<ImuInput> = emptyList(),
)

/**
 * Horizontal body-axis accelerations of one IMU, m/s², and the compass heading (degrees clockwise
 * from north) of something fixed to the car, such as the dual-antenna baseline or the Ellipse's yaw.
 * The mounting need not be known: [fuse] works out the angle between the axes and the heading.
 */
class ImuInput(val name: String, val ax: DoubleArray, val ay: DoubleArray, val heading: DoubleArray)

/** What was found when an IMU was checked against the GNSS accelerations. */
class ImuReport(
    val name: String,
    /** The IMU aids the filter. */
    val used: Boolean,
    /** Correlation (0 to 1) between the IMU's accelerations and those of the GNSS velocity, over the samples compared. */
    val correlation: Double,
    /** Fitted gain: 1 means the accelerations match in size. */
    val scale: Double,
    /** Angle between the IMU's forward axis and the heading reference, degrees. */
    val offsetDeg: Double,
    /** The IMU's Y axis points to the car's left (otherwise right). */
    val yLeft: Boolean,
    /** Samples compared. */
    val samples: Int,
)

/**
 * How much each source's position is trusted, as a multiplier on its assumed one-sigma error
 * (so a smaller number means more trust), and how hard the car can accelerate.
 */
data class FusionTuning(
    val onboardPosScale: Double = 1.0,
    val ellipsePosScale: Double = 1.0,
    /** Spectral density of the unmodelled acceleration, (m/s²)² per Hz. A car cornering at 1 g wants about 50. */
    val accelNoise: Double = 50.0,
    /** A position with no measurement within this many seconds either side is reported as missing. */
    val coastSeconds: Double = 3.0,
    /** Let accelerometers that agree with the GNSS motion aid the filter. */
    val useImu: Boolean = true,
    /** Error of an accelerometer once calibrated (tilt, noise, drift), m/s². */
    val imuSigma: Double = 0.5,
    /** With an accelerometer carrying the car between fixes, a position can coast this long. */
    val coastWithImuSeconds: Double = 6.0,
)

/** The best guess of where the car was, one value per sample. */
class FusedTrack(
    val lat: DoubleArray,
    val lon: DoubleArray,
    val altM: DoubleArray,
    val speedKmh: DoubleArray,
    val courseDeg: DoubleArray,
    /**
     * One-sigma horizontal position uncertainty from the filter, metres. It assumes the sources'
     * errors are independent; receivers that see the same sky share some error, so read it as a
     * lower bound.
     */
    val sigmaM: DoubleArray,
    /** Samples the track has a position for. */
    val validCount: Int,
    /** How each IMU fared when checked against the GNSS accelerations. */
    val imu: List<ImuReport> = emptyList(),
)

// ---- assumed measurement errors (one sigma). They are guesses until real logs say otherwise.
private const val BASE_POS_SIGMA = 1.0 // a single-band receiver in the open, metres
private const val RTK_FIXED_SIGMA = 0.03
private const val RTK_FLOAT_SIGMA = 0.3
private const val VEL_SIGMA = 0.1 // m/s, from Doppler
private const val STILL_SPEED = 0.5 // below this a GNSS course means nothing

private const val EARTH_R = 6_371_008.8
private const val RAD = PI / 180

/** One measurement of position or velocity along one axis; NaN [z] means none at that sample. */
private class AxisMeas(val z: DoubleArray, val variance: DoubleArray)

// 2x2 symmetric covariance as (p00, p01, p11)

/**
 * Combine the onboard receiver and the Ellipse (when there is one) into one track. Each axis (east,
 * north) gets a constant-velocity Kalman filter with position and velocity measurements from every
 * source, then a Rauch-Tung-Striebel pass backwards, so a sample is estimated from the samples after
 * it as well as before. Velocity measurements are what make the track smooth: Doppler speed is far
 * more accurate than the difference of two positions.
 */
fun fuse(input: FusionInputs, tuning: FusionTuning = FusionTuning()): FusedTrack {
    val n = input.t.size
    val nan = { DoubleArray(n) { Double.NaN } }
    // local origin: the first position any source has
    var origin = -1
    for (i in 0 until n) {
        if (!input.onLat[i].isNaN() || input.elLat?.get(i)?.isNaN() == false) {
            origin = i
            break
        }
    }
    if (origin < 0) return FusedTrack(nan(), nan(), nan(), nan(), nan(), nan(), 0, input.imus.map { ImuReport(it.name, false, Double.NaN, Double.NaN, Double.NaN, true, 0) })
    val lat0 = if (!input.onLat[origin].isNaN()) input.onLat[origin] else input.elLat!![origin]
    val lon0 = if (!input.onLon[origin].isNaN()) input.onLon[origin] else input.elLon!![origin]
    val kLat = EARTH_R * RAD
    val kLon = EARTH_R * RAD * cos(lat0 * RAD)

    // ---- build the measurements, east axis (0) and north axis (1)
    val pos = Array(2) { ArrayList<AxisMeas>() }
    val vel = Array(2) { ArrayList<AxisMeas>() }
    run {
        val z = Array(2) { nan() }
        val r = Array(2) { nan() }
        for (i in 0 until n) {
            if (input.onLat[i].isNaN() || input.onLon[i].isNaN()) continue
            val sigma = when (input.onFixQuality[i].toInt()) {
                4 -> RTK_FIXED_SIGMA
                5 -> RTK_FLOAT_SIGMA
                else -> BASE_POS_SIGMA * max(1.0, if (input.onHdop[i].isNaN()) 1.0 else input.onHdop[i])
            } * tuning.onboardPosScale
            z[0][i] = (input.onLon[i] - lon0) * kLon
            z[1][i] = (input.onLat[i] - lat0) * kLat
            r[0][i] = sigma * sigma
            r[1][i] = sigma * sigma
        }
        for (a in 0..1) pos[a] += AxisMeas(z[a], r[a])
    }
    if (input.elLat != null && input.elLon != null) {
        val z = Array(2) { nan() }
        val r = Array(2) { nan() }
        val sigma = BASE_POS_SIGMA * tuning.ellipsePosScale
        for (i in 0 until n) {
            if (input.elLat[i].isNaN() || input.elLon[i].isNaN()) continue
            z[0][i] = (input.elLon[i] - lon0) * kLon
            z[1][i] = (input.elLat[i] - lat0) * kLat
            r[0][i] = sigma * sigma
            r[1][i] = sigma * sigma
        }
        for (a in 0..1) pos[a] += AxisMeas(z[a], r[a])
    }
    run {
        val z = Array(2) { nan() }
        val r = Array(2) { nan() }
        for (i in 0 until n) {
            val sog = input.onSog[i]
            if (sog.isNaN()) continue
            if (sog < STILL_SPEED || input.onCog[i].isNaN()) {
                // standing still: the course is noise, but the speed says the velocity is about zero
                if (sog < STILL_SPEED) for (a in 0..1) { z[a][i] = 0.0; r[a][i] = 0.3 * 0.3 }
                continue
            }
            val c = input.onCog[i] * RAD
            z[0][i] = sog * kotlin.math.sin(c)
            z[1][i] = sog * cos(c)
            val sigma = VEL_SIGMA + 0.01 * sog // a half-degree course error at speed
            for (a in 0..1) r[a][i] = sigma * sigma
        }
        for (a in 0..1) vel[a] += AxisMeas(z[a], r[a])
    }
    if (input.elVelN != null && input.elVelE != null) {
        val z = Array(2) { nan() }
        val r = Array(2) { nan() }
        for (i in 0 until n) {
            if (input.elVelN[i].isNaN() || input.elVelE[i].isNaN()) continue
            z[0][i] = input.elVelE[i]
            z[1][i] = input.elVelN[i]
            for (a in 0..1) r[a][i] = VEL_SIGMA * VEL_SIGMA
        }
        for (a in 0..1) vel[a] += AxisMeas(z[a], r[a])
    }

    // ---- accelerometers: checked against the GNSS accelerations, then used to carry the car between fixes
    val reports = ArrayList<ImuReport>()
    var accE: DoubleArray? = null
    var accN: DoubleArray? = null
    if (input.imus.isNotEmpty()) {
        val refVel = referenceVelocity(input, n)
        val sumE = DoubleArray(n)
        val sumN = DoubleArray(n)
        val cnt = IntArray(n)
        for (imu in input.imus) {
            val (report, acc) = calibrateImu(input.t, imu, refVel)
            val use = tuning.useImu && report.used && acc != null
            reports += if (use || !report.used) report else ImuReport(report.name, false, report.correlation, report.scale, report.offsetDeg, report.yLeft, report.samples)
            if (use) for (i in 0 until n) {
                if (acc!![0][i].isNaN() || acc[1][i].isNaN()) continue
                sumE[i] += acc[0][i]
                sumN[i] += acc[1][i]
                cnt[i]++
            }
        }
        if (cnt.any { it > 0 }) {
            accE = DoubleArray(n) { if (cnt[it] > 0) sumE[it] / cnt[it] else Double.NaN }
            accN = DoubleArray(n) { if (cnt[it] > 0) sumN[it] / cnt[it] else Double.NaN }
        }
    }
    val imuAids = accE != null
    val east = smoothAxis(input.t, pos[0], vel[0], tuning.accelNoise, accE, tuning.imuSigma)
    val north = smoothAxis(input.t, pos[1], vel[1], tuning.accelNoise, accN, tuning.imuSigma)
    val coast = if (imuAids) tuning.coastWithImuSeconds else tuning.coastSeconds

    // ---- how recent the nearest position measurement is, either side
    val anyPos = BooleanArray(n) { i -> pos[0].any { !it.z[i].isNaN() } }
    val nearest = DoubleArray(n) { Double.POSITIVE_INFINITY }
    var last = Double.NEGATIVE_INFINITY
    for (i in 0 until n) {
        if (anyPos[i]) last = input.t[i]
        nearest[i] = input.t[i] - last
    }
    var next = Double.POSITIVE_INFINITY
    for (i in n - 1 downTo 0) {
        if (anyPos[i]) next = input.t[i]
        nearest[i] = minOf(nearest[i], next - input.t[i])
    }

    val lat = nan()
    val lon = nan()
    val alt = nan()
    val spd = nan()
    val crs = nan()
    val sig = nan()
    var valid = 0
    for (i in 0 until n) {
        if (nearest[i] > coast) continue
        valid++
        lat[i] = lat0 + north.p[i] / kLat
        lon[i] = lon0 + east.p[i] / kLon
        val vE = east.v[i]
        val vN = north.v[i]
        val s = hypot(vE, vN)
        spd[i] = s * 3.6
        if (s >= STILL_SPEED) crs[i] = (atan2(vE, vN) / RAD + 360) % 360
        sig[i] = sqrt(east.pVar[i] + north.pVar[i])
        val a = input.onAlt[i]
        val b = input.elAlt?.get(i) ?: Double.NaN
        alt[i] = when {
            !a.isNaN() && !b.isNaN() -> (a + b) / 2
            !a.isNaN() -> a
            else -> b
        }
    }
    return FusedTrack(lat, lon, alt, spd, crs, sig, valid, reports)
}

private class AxisResult(val p: DoubleArray, val v: DoubleArray, val pVar: DoubleArray)

/** Kalman filter and RTS smoother for one axis: state (position, velocity), scalar measurement updates. */
private fun smoothAxis(
    t: DoubleArray, pos: List<AxisMeas>, vel: List<AxisMeas>, q: Double,
    acc: DoubleArray?, accSigma: Double,
): AxisResult {
    val n = t.size
    // filtered and predicted state and covariance at every sample
    val xf = Array(n) { DoubleArray(2) }
    val pf = Array(n) { DoubleArray(3) }
    val xp = Array(n) { DoubleArray(2) }
    val pp = Array(n) { DoubleArray(3) }
    var firstPos = Double.NaN
    search@ for (i in 0 until n) for (m in pos) if (!m.z[i].isNaN()) { firstPos = m.z[i]; break@search }
    var p = if (firstPos.isNaN()) 0.0 else firstPos
    var v = 0.0
    var c00 = 100.0 * 100.0
    var c01 = 0.0
    var c11 = 30.0 * 30.0
    for (i in 0 until n) {
        if (i > 0) {
            val dt = max(0.0, t[i] - t[i - 1])
            // x = F x + B a with F = [[1, dt], [0, 1]], B = [dt^2/2, dt] and a the accelerometer's
            // acceleration over the step; P = F P F' + Q. Without an accelerometer reading the
            // acceleration is unknown and Q is the white-noise-acceleration model.
            val a = acc?.get(i) ?: Double.NaN
            val known = !a.isNaN()
            p += v * dt + if (known) 0.5 * a * dt * dt else 0.0
            if (known) v += a * dt
            val s2 = accSigma * accSigma
            val n00 = c00 + 2 * dt * c01 + dt * dt * c11 + if (known) s2 * dt * dt * dt * dt / 4 else q * dt * dt * dt / 3
            val n01 = c01 + dt * c11 + if (known) s2 * dt * dt * dt / 2 else q * dt * dt / 2
            val n11 = c11 + if (known) s2 * dt * dt else q * dt
            c00 = n00
            c01 = n01
            c11 = n11
        }
        xp[i][0] = p; xp[i][1] = v
        pp[i][0] = c00; pp[i][1] = c01; pp[i][2] = c11
        for (m in pos) {
            val z = m.z[i]
            if (z.isNaN()) continue
            // H = [1, 0]
            val s = c00 + m.variance[i]
            val k0 = c00 / s
            val k1 = c01 / s
            val innov = z - p
            p += k0 * innov
            v += k1 * innov
            val n00 = (1 - k0) * c00
            val n01 = (1 - k0) * c01
            val n11 = c11 - k1 * c01
            c00 = n00; c01 = n01; c11 = n11
        }
        for (m in vel) {
            val z = m.z[i]
            if (z.isNaN()) continue
            // H = [0, 1]
            val s = c11 + m.variance[i]
            val k0 = c01 / s
            val k1 = c11 / s
            val innov = z - v
            p += k0 * innov
            v += k1 * innov
            val n00 = c00 - k0 * c01
            val n01 = (1 - k1) * c01
            val n11 = (1 - k1) * c11
            c00 = n00; c01 = n01; c11 = n11
        }
        xf[i][0] = p; xf[i][1] = v
        pf[i][0] = c00; pf[i][1] = c01; pf[i][2] = c11
    }
    // backwards: xs[i] = xf[i] + C (xs[i+1] - xp[i+1]),  C = Pf[i] F' Pp[i+1]^-1
    val xs = Array(n) { xf[it].copyOf() }
    val ps = Array(n) { pf[it].copyOf() }
    for (i in n - 2 downTo 0) {
        val dt = max(0.0, t[i + 1] - t[i])
        val a = pf[i]
        // Pf F' = [[a00 + dt a01, a01], [a01 + dt a11, a11]]
        val m00 = a[0] + dt * a[1]
        val m01 = a[1]
        val m10 = a[1] + dt * a[2]
        val m11 = a[2]
        val b = pp[i + 1]
        val det = b[0] * b[2] - b[1] * b[1]
        if (det <= 1e-12) continue
        // inverse of the symmetric 2x2 Pp
        val i00 = b[2] / det
        val i01 = -b[1] / det
        val i11 = b[0] / det
        val k00 = m00 * i00 + m01 * i01
        val k01 = m00 * i01 + m01 * i11
        val k10 = m10 * i00 + m11 * i01
        val k11 = m10 * i01 + m11 * i11
        val d0 = xs[i + 1][0] - xp[i + 1][0]
        val d1 = xs[i + 1][1] - xp[i + 1][1]
        xs[i][0] = xf[i][0] + k00 * d0 + k01 * d1
        xs[i][1] = xf[i][1] + k10 * d0 + k11 * d1
        // Ps = Pf + C (Ps[i+1] - Pp[i+1]) C'
        val e00 = ps[i + 1][0] - b[0]
        val e01 = ps[i + 1][1] - b[1]
        val e11 = ps[i + 1][2] - b[2]
        val t00 = k00 * e00 + k01 * e01
        val t01 = k00 * e01 + k01 * e11
        val t10 = k10 * e00 + k11 * e01
        val t11 = k10 * e01 + k11 * e11
        ps[i][0] = a[0] + t00 * k00 + t01 * k01
        ps[i][1] = a[1] + t00 * k10 + t01 * k11
        ps[i][2] = a[2] + t10 * k10 + t11 * k11
    }
    return AxisResult(DoubleArray(n) { xs[it][0] }, DoubleArray(n) { xs[it][1] }, DoubleArray(n) { max(0.0, ps[it][0]) })
}

/** The east and north velocity the GNSS sources agree on, per sample: their mean where there is more than one. */
private class RefVel(val e: DoubleArray, val n: DoubleArray)

private fun referenceVelocity(input: FusionInputs, count: Int): RefVel {
    val e = DoubleArray(count) { Double.NaN }
    val nn = DoubleArray(count) { Double.NaN }
    for (i in 0 until count) {
        var se = 0.0
        var sn = 0.0
        var k = 0
        val sog = input.onSog[i]
        if (!sog.isNaN() && sog >= STILL_SPEED && !input.onCog[i].isNaN()) {
            val c = input.onCog[i] * RAD
            se += sog * kotlin.math.sin(c)
            sn += sog * cos(c)
            k++
        }
        val ve = input.elVelE?.get(i) ?: Double.NaN
        val vn = input.elVelN?.get(i) ?: Double.NaN
        if (!ve.isNaN() && !vn.isNaN()) {
            se += ve
            sn += vn
            k++
        }
        if (k > 0) {
            e[i] = se / k
            nn[i] = sn / k
        }
    }
    return RefVel(e, nn)
}

private const val MIN_CAL_SPEED = 3.0 // m/s: below this the course, and so the reference acceleration, is noise
private const val MIN_CAL_SAMPLES = 30
private const val MIN_CORRELATION = 0.7

/**
 * Check an accelerometer against the GNSS accelerations (the derivative of the reference velocity) and,
 * if they agree, return its accelerations in north and east.
 *
 * In complex form with north real and west imaginary, the acceleration is
 * `g = rho * z + c`, where `z = (ax + i s ay) e^(-i heading)` is the body acceleration turned by the
 * heading, `s` is +1 if the IMU's Y points left and -1 if right, `rho` a fixed complex gain (its
 * argument is the mounting offset, its size should be near 1) and `c` a constant (tilt and bias).
 * Both signs of `s` are tried and the better fit kept.
 */
private fun calibrateImu(t: DoubleArray, imu: ImuInput, ref: RefVel): Pair<ImuReport, Array<DoubleArray>?> {
    val n = t.size
    // reference acceleration by a central difference, as north + i*west
    val gRe = DoubleArray(n) { Double.NaN }
    val gIm = DoubleArray(n) { Double.NaN }
    for (i in 1 until n - 1) {
        val dt = t[i + 1] - t[i - 1]
        if (dt <= 0 || dt > 0.35) continue
        val an = (ref.n[i + 1] - ref.n[i - 1]) / dt
        val ae = (ref.e[i + 1] - ref.e[i - 1]) / dt
        if (an.isNaN() || ae.isNaN()) continue
        if (hypot(ref.e[i], ref.n[i]) < MIN_CAL_SPEED) continue
        gRe[i] = an
        gIm[i] = -ae
    }
    class Fit(val flip: Int, val rhoRe: Double, val rhoIm: Double, val cRe: Double, val cIm: Double, val r: Double, val used: Int)
    fun zOf(i: Int, s: Int): Pair<Double, Double>? {
        val ax = imu.ax[i]
        val ay = imu.ay[i]
        val h = imu.heading[i]
        if (ax.isNaN() || ay.isNaN() || h.isNaN()) return null
        val hr = h * RAD
        val c = cos(hr)
        val sn = kotlin.math.sin(hr)
        return (ax * c + s * ay * sn) to (s * ay * c - ax * sn)
    }
    var best: Fit? = null
    for (s in intArrayOf(1, -1)) {
        var k = 0
        var mzr = 0.0; var mzi = 0.0; var mgr = 0.0; var mgi = 0.0
        for (i in 0 until n) {
            if (gRe[i].isNaN()) continue
            val z = zOf(i, s) ?: continue
            mzr += z.first; mzi += z.second; mgr += gRe[i]; mgi += gIm[i]
            k++
        }
        if (k < MIN_CAL_SAMPLES) continue
        mzr /= k; mzi /= k; mgr /= k; mgi /= k
        var sxzRe = 0.0; var sxzIm = 0.0; var szz = 0.0; var sgg = 0.0
        for (i in 0 until n) {
            if (gRe[i].isNaN()) continue
            val z = zOf(i, s) ?: continue
            val zr = z.first - mzr
            val zi = z.second - mzi
            val gr = gRe[i] - mgr
            val gi = gIm[i] - mgi
            // conj(z) * g
            sxzRe += zr * gr + zi * gi
            sxzIm += zr * gi - zi * gr
            szz += zr * zr + zi * zi
            sgg += gr * gr + gi * gi
        }
        if (szz < 1e-9 || sgg < 1e-9) continue
        val rhoRe = sxzRe / szz
        val rhoIm = sxzIm / szz
        val r = sqrt((sxzRe * sxzRe + sxzIm * sxzIm) / (szz * sgg))
        val cRe = mgr - (rhoRe * mzr - rhoIm * mzi)
        val cIm = mgi - (rhoRe * mzi + rhoIm * mzr)
        if (best == null || r > best.r) best = Fit(s, rhoRe, rhoIm, cRe, cIm, r, k)
    }
    val b = best ?: return ImuReport(imu.name, false, Double.NaN, Double.NaN, Double.NaN, true, 0) to null
    val scale = hypot(b.rhoRe, b.rhoIm)
    val offset = -atan2(b.rhoIm, b.rhoRe) / RAD
    val ok = b.r >= MIN_CORRELATION && scale in 0.7..1.4
    val report = ImuReport(imu.name, ok, b.r, scale, offset, b.flip > 0, b.used)
    if (!ok) return report to null
    val aE = DoubleArray(n) { Double.NaN }
    val aN = DoubleArray(n) { Double.NaN }
    for (i in 0 until n) {
        val z = zOf(i, b.flip) ?: continue
        val re = b.rhoRe * z.first - b.rhoIm * z.second + b.cRe
        val im = b.rhoRe * z.second + b.rhoIm * z.first + b.cIm
        aN[i] = re
        aE[i] = -im
    }
    return report to arrayOf(aE, aN)
}
