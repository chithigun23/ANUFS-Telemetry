package au.anufs.telemetry.core

import java.io.BufferedReader
import java.time.Instant
import kotlin.math.PI
import kotlin.math.asin
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * The periodic samples of one session as columns, straight from the `samples` CSV
 * export. Empty CSV fields (a value whose validity flag was clear) become NaN.
 */
class Samples(
    /** Number of rows. */
    val size: Int,
    private val columns: Map<String, DoubleArray>,
    /** UTC of each row in Unix ms, or [NO_TIME] if the row has none. */
    val utcMs: LongArray,
) {
    /** A column by CSV name, or an all-NaN column if the log does not have it. */
    operator fun get(name: String): DoubleArray = columns[name] ?: DoubleArray(size) { Double.NaN }

    /** The monotonic clock the log's own timestamps use, seconds (`t_s` in the CSV). */
    val tAbs: DoubleArray = get("t_s")

    /** Seconds since the first sample. */
    val t: DoubleArray = run {
        val t0 = tAbs.firstOrNull() ?: 0.0
        DoubleArray(size) { tAbs[it] - t0 }
    }

    val lat: DoubleArray get() = get("lat_deg")
    val lon: DoubleArray get() = get("lon_deg")

    /** Speed over ground in km/h. */
    val speedKmh: DoubleArray = get("sog_mps").let { s -> DoubleArray(size) { s[it] * 3.6 } }

    /** Rows that have a position. */
    fun hasFix(i: Int): Boolean = !lat[i].isNaN() && !lon[i].isNaN()

    /** First row at or after [seconds] since the start, clamped to the data. */
    fun indexAt(seconds: Double): Int = indexAtTime(t, seconds)

    val durationSeconds: Double get() = if (size == 0) 0.0 else t[size - 1]

    /** Great-circle length of the track in metres, skipping rows without a fix. */
    val distanceMetres: Double
        get() {
            var d = 0.0
            var prev = -1
            for (i in 0 until size) {
                if (!hasFix(i)) continue
                if (prev >= 0) d += haversine(lat[prev], lon[prev], lat[i], lon[i])
                prev = i
            }
            return d
        }

    val maxSpeedKmh: Double get() = speedKmh.filter { !it.isNaN() }.maxOrNull() ?: Double.NaN

    companion object {
        const val NO_TIME = Long.MIN_VALUE
    }
}

private val NON_NUMERIC = setOf("session", "utc", "flags")

/** Parse the CSV written by the `samples` export. */
fun parseSamplesCsv(reader: BufferedReader): Samples {
    val header = reader.readLine()?.split(',') ?: return Samples(0, emptyMap(), LongArray(0))
    val rows = ArrayList<List<String>>()
    while (true) {
        val line = reader.readLine() ?: break
        if (line.isNotBlank()) rows.add(line.split(','))
    }
    val n = rows.size
    val cols = HashMap<String, DoubleArray>()
    for ((c, name) in header.withIndex()) {
        if (name in NON_NUMERIC) continue
        cols[name] = DoubleArray(n) { r -> rows[r].getOrNull(c)?.toDoubleOrNull() ?: Double.NaN }
    }
    val utcCol = header.indexOf("utc")
    val utc = LongArray(n) { r ->
        val s = if (utcCol >= 0) rows[r].getOrNull(utcCol).orEmpty() else ""
        if (s.isEmpty()) Samples.NO_TIME else runCatching { Instant.parse(s).toEpochMilli() }.getOrDefault(Samples.NO_TIME)
    }
    return Samples(n, cols, utc)
}

/** Distance in metres between two points on the Earth, haversine. */
fun haversine(lat1: Double, lon1: Double, lat2: Double, lon2: Double): Double {
    val r = 6_371_008.8
    val p = PI / 180
    val a = sin((lat2 - lat1) * p / 2).let { it * it } +
        cos(lat1 * p) * cos(lat2 * p) * sin((lon2 - lon1) * p / 2).let { it * it }
    return 2 * r * asin(sqrt(max(0.0, minOf(1.0, a))))
}

/** Index of the first element of the ascending array [t] that is >= [seconds] (the last index if none). */
fun indexAtTime(t: DoubleArray, seconds: Double): Int {
    if (t.isEmpty()) return 0
    var lo = 0
    var hi = t.size - 1
    while (lo < hi) {
        val mid = (lo + hi) ushr 1
        if (t[mid] < seconds) lo = mid + 1 else hi = mid
    }
    return lo
}
