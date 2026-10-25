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
        val trueE = DoubleArray(n) { r * cos(w * t[it]) }
        val trueN = DoubleArray(n) { r * sin(w * t[it]) }
        val rng = Rng(seed)
        // the onboard error drifts slowly (a first-order walk, about 20 s long) as well as jittering
        private var bx = 0.0
        private var by = 0.0
        val onLat = DoubleArray(n)
        val onLon = DoubleArray(n)
        val elLat = DoubleArray(n)
        val elLon = DoubleArray(n)
        val sog = DoubleArray(n) { speed + rng.g(0.05) }
        val cog = DoubleArray(n) { ((90 - Math.toDegrees(w * t[it] + PI / 2)) % 360 + 360) % 360 + rng.g(0.3) }
        val velN = DoubleArray(n) { speed * cos(w * t[it]) + rng.g(0.05) }
        val velE = DoubleArray(n) { -speed * sin(w * t[it]) + rng.g(0.05) }
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

        fun inputs(withEllipse: Boolean = ellipse) = FusionInputs(
            t, onLat.copyOf(), onLon.copyOf(), sog, cog, hdop, fixQ, alt,
            if (withEllipse) elLat else null, if (withEllipse) elLon else null,
            if (withEllipse) velN else null, if (withEllipse) velE else null, if (withEllipse) alt else null,
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
        val sErr = (0 until w.n).map { Math.abs(f.speedKmh[it] / 3.6 - speed) }.average()
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
}
