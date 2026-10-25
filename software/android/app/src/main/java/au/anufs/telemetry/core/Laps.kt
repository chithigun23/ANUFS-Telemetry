package au.anufs.telemetry.core

import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.max

/**
 * A start/finish line on the ground, from one end to the other, in degrees.
 *
 * [dir] says which way the car must cross it: +1 when it goes from the line's right to its left
 * as seen walking from the first end to the second (the line made by [lineAtSample] is always
 * +1), -1 for the opposite, and 0 when unknown. With 0, [detectLaps] uses the direction most
 * crossings go.
 */
data class GeoLine(val lat1: Double, val lon1: Double, val lat2: Double, val lon2: Double, val dir: Int = 0) {
    val midLat: Double get() = (lat1 + lat2) / 2
    val midLon: Double get() = (lon1 + lon2) / 2

    /** Length in metres. */
    val lengthMetres: Double get() = haversine(lat1, lon1, lat2, lon2)

    /** `lat1,lon1,lat2,lon2,dir`, for saving. */
    fun encode(): String = "%.7f,%.7f,%.7f,%.7f,%d".format(java.util.Locale.US, lat1, lon1, lat2, lon2, dir)

    companion object {
        fun parse(s: String): GeoLine? {
            val f = s.split(',')
            if (f.size != 5) return null
            val n = f.take(4).map { it.toDoubleOrNull() ?: return null }
            return GeoLine(n[0], n[1], n[2], n[3], f[4].toIntOrNull()?.coerceIn(-1, 1) ?: return null)
        }
    }
}

/** One complete lap, from one crossing of the line to the next. */
data class Lap(
    val number: Int,
    /** Seconds since the first sample at which the lap starts and ends (interpolated at the line). */
    val startS: Double,
    val endS: Double,
    val distanceMetres: Double,
    val maxKmh: Double,
) {
    val timeS: Double get() = endS - startS
    val avgKmh: Double get() = if (timeS > 0) distanceMetres / timeS * 3.6 else Double.NaN
}

/** Everything found for one line on one track. */
class LapResult(
    /** Times of every accepted crossing, seconds since the first sample. */
    val crossings: DoubleArray,
    val laps: List<Lap>,
    /** Seconds from the start of the session to the first crossing (the out lap), or NaN. */
    val outLapS: Double,
    /** Seconds from the last crossing to the end of the session (the in lap), or NaN. */
    val inLapS: Double,
) {
    /** Index into [laps] of the fastest, or -1. */
    val bestIndex: Int get() = laps.indices.minByOrNull { laps[it].timeS } ?: -1
}

private const val EARTH_R = 6_371_008.8

/** Metres east and north of (lat0, lon0), flat-earth: good to well under a millimetre over a track. */
private fun toXy(lat: Double, lon: Double, lat0: Double, lon0: Double): Pair<Double, Double> {
    val k = PI / 180 * EARTH_R
    return (lon - lon0) * cos(lat0 * PI / 180) * k to (lat - lat0) * k
}

/** The line across the track at sample [i], square to the direction of travel, [halfLengthMetres] each side. */
fun lineAtSample(lat: DoubleArray, lon: DoubleArray, i: Int, halfLengthMetres: Double = 20.0): GeoLine? {
    if (i !in lat.indices) return null
    // direction from the fixes a little before and after, so noise in one fix does not tilt the line
    var a = i
    var b = i
    for (k in 1..5) {
        if ((i - k) in lat.indices && !lat[i - k].isNaN() && !lon[i - k].isNaN()) a = i - k
        if ((i + k) in lat.indices && !lat[i + k].isNaN() && !lon[i + k].isNaN()) b = i + k
    }
    if (a == b || lat[i].isNaN() || lon[i].isNaN()) return null
    val (dx, dy) = toXy(lat[b], lon[b], lat[a], lon[a])
    val len = hypot(dx, dy)
    if (len < 0.5) return null // standing still: no direction
    // unit vector of travel v = (dx, dy)/len; the line runs along (vy, -vx), so travel crosses it with dir +1
    val ux = dy / len
    val uy = -dx / len
    val k = 180 / PI / EARTH_R
    val dLat = uy * halfLengthMetres * k
    val dLon = ux * halfLengthMetres * k / cos(lat[i] * PI / 180)
    return GeoLine(lat[i] - dLat, lon[i] - dLon, lat[i] + dLat, lon[i] + dLon, dir = 1)
}

/**
 * Find the laps of a track against [line]. A crossing is where a step between two consecutive
 * fixes cuts the line; its time is interpolated inside the step. Crossings closer than
 * [minLapSeconds] to the previous one are ignored (a car wobbling over the line), as are steps
 * across a gap of more than a second between fixes and, when the line has a direction, crossings
 * the wrong way. Time before the first crossing and after the last is not a lap.
 */
fun detectLaps(
    t: DoubleArray,
    lat: DoubleArray,
    lon: DoubleArray,
    speedKmh: DoubleArray,
    line: GeoLine,
    minLapSeconds: Double = 10.0,
): LapResult {
    val n = t.size
    val lat0 = line.midLat
    val lon0 = line.midLon
    val (x1, y1) = toXy(line.lat1, line.lon1, lat0, lon0)
    val (x2, y2) = toXy(line.lat2, line.lon2, lat0, lon0)
    val ex = x2 - x1
    val ey = y2 - y1
    // (time, direction sign) of every step that cuts the line
    val raw = ArrayList<Pair<Double, Int>>()
    var prevOk = -1
    for (i in 0 until n) {
        if (lat[i].isNaN() || lon[i].isNaN()) {
            prevOk = -1
            continue
        }
        if (prevOk >= 0 && t[i] - t[prevOk] <= 1.0) {
            val (px, py) = toXy(lat[prevOk], lon[prevOk], lat0, lon0)
            val (qx, qy) = toXy(lat[i], lon[i], lat0, lon0)
            val sx = qx - px
            val sy = qy - py
            // solve p + u*s = a + w*e
            val den = sx * ey - sy * ex
            if (abs(den) > 1e-9) {
                val u = ((x1 - px) * ey - (y1 - py) * ex) / den
                val w = ((x1 - px) * sy - (y1 - py) * sx) / den
                if (u >= 0 && u < 1 && w >= 0 && w <= 1) {
                    val dirSign = if (ex * sy - ey * sx > 0) 1 else -1
                    raw += (t[prevOk] + u * (t[i] - t[prevOk])) to dirSign
                }
            }
        }
        prevOk = i
    }
    val wanted = when {
        line.dir != 0 -> line.dir
        raw.isEmpty() -> 0
        else -> if (raw.count { it.second > 0 } >= raw.size - raw.count { it.second > 0 }) 1 else -1
    }
    val crossings = ArrayList<Double>()
    for ((time, sign) in raw) {
        if (sign != wanted) continue
        if (crossings.isNotEmpty() && time - crossings.last() < minLapSeconds) continue
        crossings += time
    }
    val laps = ArrayList<Lap>()
    for (k in 0 until crossings.size - 1) {
        val a = crossings[k]
        val b = crossings[k + 1]
        var dist = 0.0
        var top = Double.NaN
        var prev = -1
        for (i in 0 until n) {
            if (t[i] < a || t[i] > b) continue
            if (!speedKmh[i].isNaN()) top = if (top.isNaN()) speedKmh[i] else max(top, speedKmh[i])
            if (lat[i].isNaN() || lon[i].isNaN()) {
                prev = -1
                continue
            }
            if (prev >= 0) dist += haversine(lat[prev], lon[prev], lat[i], lon[i])
            prev = i
        }
        laps += Lap(k + 1, a, b, dist, top)
    }
    val end = if (n > 0) t[n - 1] else Double.NaN
    return LapResult(
        crossings = crossings.toDoubleArray(),
        laps = laps,
        outLapS = crossings.firstOrNull() ?: Double.NaN,
        inLapS = crossings.lastOrNull()?.let { end - it } ?: Double.NaN,
    )
}

/** Of the saved lines, the one that finds the most laps on this track, or null if none finds any. */
fun bestSavedLine(
    t: DoubleArray, lat: DoubleArray, lon: DoubleArray, speedKmh: DoubleArray,
    saved: List<GeoLine>, minLapSeconds: Double = 10.0,
): GeoLine? = saved
    .map { it to detectLaps(t, lat, lon, speedKmh, it, minLapSeconds).laps.size }
    .filter { it.second > 0 }
    .maxByOrNull { it.second }?.first

/** Add [line] to [saved]: a line whose middle is within 30 m of an existing one replaces it. Newest first, at most 20. */
fun rememberLine(saved: List<GeoLine>, line: GeoLine): List<GeoLine> =
    (listOf(line) + saved.filter { haversine(it.midLat, it.midLon, line.midLat, line.midLon) > 30.0 }).take(20)

/** Compass bearing in degrees of travel between two fixes, for labels. */
fun bearing(lat1: Double, lon1: Double, lat2: Double, lon2: Double): Double {
    val (dx, dy) = toXy(lat2, lon2, lat1, lon1)
    return (90 - atan2(dy, dx) * 180 / PI + 360) % 360
}
