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
    if (origin < 0) return FusedTrack(nan(), nan(), nan(), nan(), nan(), nan(), 0)
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

    val east = smoothAxis(input.t, pos[0], vel[0], tuning.accelNoise)
    val north = smoothAxis(input.t, pos[1], vel[1], tuning.accelNoise)

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
        if (nearest[i] > tuning.coastSeconds) continue
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
    return FusedTrack(lat, lon, alt, spd, crs, sig, valid)
}

private class AxisResult(val p: DoubleArray, val v: DoubleArray, val pVar: DoubleArray)

/** Kalman filter and RTS smoother for one axis: state (position, velocity), scalar measurement updates. */
private fun smoothAxis(t: DoubleArray, pos: List<AxisMeas>, vel: List<AxisMeas>, q: Double): AxisResult {
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
            // x = F x with F = [[1, dt], [0, 1]];  P = F P F' + Q
            p += v * dt
            val n00 = c00 + 2 * dt * c01 + dt * dt * c11 + q * dt * dt * dt / 3
            val n01 = c01 + dt * c11 + q * dt * dt / 2
            val n11 = c11 + q * dt
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
