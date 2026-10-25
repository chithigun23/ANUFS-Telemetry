package au.anufs.telemetry.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.ln
import kotlin.math.sin
import kotlin.math.sqrt

class FusionTest {
    private val lat0 = -35.28
    private val lon0 = 149.13
    private val r = 50.0
    private val speed = 10.0
    private val mLat = 111_194.9
    private val mLon = 111_194.9 * cos(lat0 * PI / 180)

    private class Rng(var s: Long) {
        fun u(): Double { s = s * 6364136223846793005L + 1442695040888963407L; return ((s ushr 11) + 1).toDouble() / (1L shl 53).toDouble() }
        fun g(sigma: Double): Double = sigma * sqrt(-2 * ln(u())) * cos(2 * PI * u())
    }

    /** A circuit driven anticlockwise for [seconds], and what each source reports of it. */
    private inner class World(seconds: Double, val ellipse: Boolean = true, onSigma: Double = 0.8, elSigma: Double = 0.8, elBiasE: Double = 0.0, walk: Double = 0.1, seed: Long = 1) {
        val n = (seconds * 10).toInt() + 1
        val t = DoubleArray(n) { it / 10.0 }
        val w = speed / r
        // the speed varies by +-3.75 m/s about 10 so there is longitudinal acceleration too
        private val amp = 0.15
        private val om = 0.5
        val theta = DoubleArray(n) { w * t[it] + amp * sin(om * t[it]) }
        val trueSpeed = DoubleArray(n) { r * (w + amp * om * cos(om * t[it])) }
        val trueE = DoubleArray(n) { r * cos(theta[it]) }
        val trueN = DoubleArray(n) { r * sin(theta[it]) }
        val rng = Rng(seed)
        // the onboard error drifts slowly (a first-order walk, about 20 s long) as well as jittering
        private var bx = 0.0
        private var by = 0.0
        val onLat = DoubleArray(n)
        val onLon = DoubleArray(n)
        val elLat = DoubleArray(n)
        val elLon = DoubleArray(n)
        val sog = DoubleArray(n) { trueSpeed[it] + rng.g(0.05) }
        val cog = DoubleArray(n) { ((90 - Math.toDegrees(theta[it] + PI / 2)) % 360 + 360) % 360 + rng.g(0.3) }
        val velN = DoubleArray(n) { trueSpeed[it] * cos(theta[it]) + rng.g(0.05) }
        val velE = DoubleArray(n) { -trueSpeed[it] * sin(theta[it]) + rng.g(0.05) }
        val hdop = DoubleArray(n) { 0.8 }
        val fixQ = DoubleArray(n) { 1.0 }
        val alt = DoubleArray(n) { 584.0 }
        init {
            for (i in 0 until n) {
                bx = bx * 0.995 + rng.g(walk)
                by = by * 0.995 + rng.g(walk)
                onLat[i] = lat0 + (trueN[i] + by + rng.g(onSigma)) / mLat
                onLon[i] = lon0 + (trueE[i] + bx + rng.g(onSigma)) / mLon
                elLat[i] = lat0 + (trueN[i] + rng.g(elSigma)) / mLat
                elLon[i] = lon0 + (trueE[i] + elBiasE + rng.g(elSigma)) / mLon
            }
        }

        /** What an accelerometer reads on this circuit: no speed change, 2 m/s² to the car's left; the mount can be turned and flipped. */
        fun imu(offsetDeg: Double = 0.0, yLeft: Boolean = true, noise: Double = 0.2, bias: Double = 0.0, seed2: Long = 99): ImuInput {
            val g = Rng(seed2)
            val a = Math.toRadians(offsetDeg)
            // the car's (forward, left) acceleration; the IMU is turned by `a` and has its Y flipped if it points right
            val fwd = DoubleArray(n) { -r * amp * om * om * sin(om * t[it]) }
            val left = DoubleArray(n) { trueSpeed[it] * trueSpeed[it] / r }
            val fx = DoubleArray(n) { Math.cos(a) * fwd[it] + Math.sin(a) * left[it] + g.g(noise) + bias }
            val fy = DoubleArray(n) { (-Math.sin(a) * fwd[it] + Math.cos(a) * left[it]) * (if (yLeft) 1 else -1) + g.g(noise) }
            return ImuInput("test IMU", fx, fy, cog.copyOf())
        }

        fun inputs(withEllipse: Boolean = ellipse, imu: ImuInput? = null) = FusionInputs(
            t, onLat.copyOf(), onLon.copyOf(), sog, cog, hdop, fixQ, alt,
            if (withEllipse) elLat else null, if (withEllipse) elLon else null,
            if (withEllipse) velN else null, if (withEllipse) velE else null, if (withEllipse) alt else null,
            imus = listOfNotNull(imu),
        )

        /** Root-mean-square distance from the truth over the samples [from, to) that have a position. */
        fun rms(lat: DoubleArray, lon: DoubleArray, from: Int = 0, to: Int = n): Double {
            var s = 0.0
            var k = 0
            for (i in from until to) {
                if (lat[i].isNaN()) continue
                val e = (lon[i] - lon0) * mLon - trueE[i]
                val nn = (lat[i] - lat0) * mLat - trueN[i]
                s += e * e + nn * nn
                k++
            }
            return sqrt(s / k)
        }
    }

    @Test
    fun fusingTwoReceiversBeatsEitherAlone() {
        val w = World(120.0)
        val f = fuse(w.inputs())
        val on = w.rms(w.onLat, w.onLon)
        val el = w.rms(w.elLat, w.elLon)
        val fused = w.rms(f.lat, f.lon)
        println("rms onboard $on ellipse $el fused $fused")
        assertTrue("fused $fused vs onboard $on", fused < 0.7 * on)
        assertTrue("fused $fused vs ellipse $el", fused < 0.7 * el)
        assertEquals(w.n, f.validCount)
        // speed from the fused velocity is within a few cm/s of the truth, and the course is the direction of travel
        val sErr = (0 until w.n).map { Math.abs(f.speedKmh[it] / 3.6 - w.trueSpeed[it]) }.average()
        assertTrue("mean speed error $sErr m/s", sErr < 0.1)
        assertTrue("sigma is reported and small", f.sigmaM.all { it > 0 && it < 2.0 })
    }

    @Test
    fun onboardAloneIsStillSmoothedWithItsDopplerSpeed() {
        val w = World(120.0, ellipse = false)
        val f = fuse(w.inputs(withEllipse = false))
        val on = w.rms(w.onLat, w.onLon)
        val fused = w.rms(f.lat, f.lon)
        println("onboard only: rms raw $on fused $fused")
        assertTrue("fused $fused vs raw $on", fused < on)
        assertEquals(w.n, f.validCount)
    }

    @Test
    fun theEllipseCoversALostFixButAloneTheTrackGoesMissingAfterThreeSeconds() {
        val w = World(60.0)
        // the onboard receiver loses the fix from 20 s to 30 s
        for (i in 200..300) { w.onLat[i] = Double.NaN; w.onLon[i] = Double.NaN }
        val both = fuse(w.inputs())
        assertEquals("the Ellipse keeps the position alive", w.n, both.validCount)
        assertTrue(w.rms(both.lat, both.lon, 200, 301) < 1.5)

        val ellipseGone = w.inputs(withEllipse = false)
        val alone = fuse(ellipseGone)
        assertFalse("1.5 s in: still coasting", alone.lat[215].isNaN())
        assertTrue("5 s in: nothing to stand on", alone.lat[250].isNaN())
        assertFalse("fix is back", alone.lat[320].isNaN())
        assertTrue(alone.validCount < w.n)
        assertTrue(alone.sigmaM[250].isNaN())
    }

    @Test
    fun theWeightingMovesTheTrackTowardTheTrustedSource() {
        // the Ellipse reads 3 m east of the truth, the onboard receiver is unbiased
        val w = World(60.0, elBiasE = 3.0, onSigma = 0.3, elSigma = 0.3)
        fun eastOffset(tuning: FusionTuning): Double {
            val f = fuse(w.inputs(), tuning)
            return (0 until w.n).map { (f.lon[it] - lon0) * mLon - w.trueE[it] }.average() // mean east error
        }
        // average over a lap, because the errors rotate with the car
        val equal = Math.abs(w.run { val f = fuse(inputs()); (0 until n).map { ((f.lon[it] - lon0) * mLon - trueE[it]) * 1.0 }.average() })
        val preferOnboard = Math.abs(eastOffset(FusionTuning(onboardPosScale = 0.3, ellipsePosScale = 3.0)))
        val preferEllipse = Math.abs(eastOffset(FusionTuning(onboardPosScale = 3.0, ellipsePosScale = 0.3)))
        println("mean east error: equal $equal, prefer onboard $preferOnboard, prefer Ellipse $preferEllipse")
        assertTrue(preferOnboard < equal)
        assertTrue(equal < preferEllipse)
        assertEquals("trusting the Ellipse brings the bias across", 3.0, preferEllipse, 0.5)
    }

    @Test
    fun anEmptyOrAllMissingSessionGivesAnEmptyTrack() {
        val n = 5
        val nan = DoubleArray(n) { Double.NaN }
        val f = fuse(FusionInputs(DoubleArray(n) { it / 10.0 }, nan, nan, nan, nan, nan, nan, nan, null, null, null, null, null))
        assertEquals(0, f.validCount)
        assertTrue(f.lat.all { it.isNaN() })
        assertEquals(0, fuse(FusionInputs(DoubleArray(0), DoubleArray(0), DoubleArray(0), DoubleArray(0), DoubleArray(0), DoubleArray(0), DoubleArray(0), DoubleArray(0), null, null, null, null, null)).validCount)
    }

    @Test
    fun anRtkFixedPositionOutweighsAMetreClassOne() {
        val w = World(30.0, onSigma = 0.02, elSigma = 1.5, elBiasE = 4.0, walk = 0.0)
        for (i in 0 until w.n) w.fixQ[i] = 4.0 // the onboard receiver reports an RTK fixed solution
        val f = fuse(w.inputs())
        assertTrue("rms ${w.rms(f.lat, f.lon)}", w.rms(f.lat, f.lon) < 0.5)
    }

    @Test
    fun anAccelerometerThatMatchesTheMotionCarriesTheTrackThroughALostFix() {
        val w = World(60.0, ellipse = false)
        for (i in 200..270) { w.onLat[i] = Double.NaN; w.onLon[i] = Double.NaN } // seven seconds without a fix
        val without = fuse(w.inputs(withEllipse = false))
        val withImu = fuse(w.inputs(withEllipse = false, imu = w.imu()))
        val rep = withImu.imu.single()
        println("imu: used=${rep.used} r=${rep.correlation} scale=${rep.scale} offset=${rep.offsetDeg} yLeft=${rep.yLeft}")
        assertTrue(rep.used)
        assertTrue("r ${rep.correlation}", rep.correlation > 0.9)
        assertEquals(1.0, rep.scale, 0.15)
        assertEquals(0.0, rep.offsetDeg, 3.0)
        assertTrue(rep.yLeft)
        assertTrue("without the accelerometer the middle of the gap is missing", without.lat[235].isNaN())
        assertFalse("with it the track is carried across", withImu.lat[235].isNaN())
        val err = w.rms(withImu.lat, withImu.lon, 200, 271)
        println("rms in the gap with the IMU: $err")
        assertTrue("error across the gap $err", err < 3.0)
    }

    @Test
    fun theMountingIsFoundWhateverWayTheAccelerometerIsTurned() {
        val w = World(60.0, ellipse = false)
        val straight = fuse(w.inputs(withEllipse = false, imu = w.imu()))
        val turned = fuse(w.inputs(withEllipse = false, imu = w.imu(offsetDeg = 40.0, yLeft = false)))
        val rep = turned.imu.single()
        println("turned: used=${rep.used} r=${rep.correlation} offset=${rep.offsetDeg} yLeft=${rep.yLeft}")
        assertTrue(rep.used)
        assertFalse("the Y axis points right", rep.yLeft)
        assertEquals(40.0, Math.abs(rep.offsetDeg), 5.0)
        assertEquals("same track as an IMU mounted straight", w.rms(straight.lat, straight.lon), w.rms(turned.lat, turned.lon), 0.1)
    }

    @Test
    fun anAccelerometerThatDoesNotMatchTheMotionIsLeftOut() {
        val w = World(60.0, ellipse = false)
        val rng = Rng(5)
        val junk = ImuInput("junk", DoubleArray(w.n) { rng.g(3.0) }, DoubleArray(w.n) { rng.g(3.0) }, w.cog.copyOf())
        val plain = fuse(w.inputs(withEllipse = false))
        val withJunk = fuse(w.inputs(withEllipse = false, imu = junk))
        val rep = withJunk.imu.single()
        println("junk: used=${rep.used} r=${rep.correlation}")
        assertFalse(rep.used)
        assertTrue("r ${rep.correlation}", rep.correlation < 0.5)
        for (i in 0 until w.n) assertEquals(plain.lat[i], withJunk.lat[i], 0.0)
    }

    @Test
    fun theImuSwitchTurnsAidingOff() {
        val w = World(60.0, ellipse = false)
        for (i in 200..270) { w.onLat[i] = Double.NaN; w.onLon[i] = Double.NaN }
        val off = fuse(w.inputs(withEllipse = false, imu = w.imu()), FusionTuning(useImu = false))
        assertFalse(off.imu.single().used)
        assertTrue(off.lat[235].isNaN())
    }

    @Test
    fun withAGoodAccelerometerTheFullLapIsNoWorse() {
        val w = World(120.0)
        val a = fuse(w.inputs())
        val b = fuse(w.inputs(imu = w.imu()))
        println("rms without ${w.rms(a.lat, a.lon)} with ${w.rms(b.lat, b.lon)}")
        assertTrue(w.rms(b.lat, b.lon) <= w.rms(a.lat, a.lon) * 1.05)
    }
}
