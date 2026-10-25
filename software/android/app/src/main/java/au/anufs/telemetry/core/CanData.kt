package au.anufs.telemetry.core

import java.io.BufferedReader
import org.json.JSONArray

/** Prefix SBG's database puts on every message name; dropped for display. */
private const val SBG_PREFIX = "SBG_ECAN_MSG_"

/** One decoded CAN signal over time, as logged. */
class SignalSeries(
    val bus: Int,
    val message: String,
    val signal: String,
    val unit: String,
    /** Log clock (`t_s`) of each value, seconds. */
    val t: DoubleArray,
    /** Physical value of each point. */
    val v: DoubleArray,
) {
    /** `EKF_EULER.ROLL` style name. */
    val label: String get() = "${message.removePrefix(SBG_PREFIX)}.$signal"

    val min: Double get() = v.filter { !it.isNaN() }.minOrNull() ?: Double.NaN
    val max: Double get() = v.filter { !it.isNaN() }.maxOrNull() ?: Double.NaN

    /**
     * The value in force at each time of [grid] (the newest point at or before it, within half a
     * millisecond of slack for rounding), NaN before the first point. Both must be ascending.
     */
    fun heldOn(grid: DoubleArray): DoubleArray {
        val out = DoubleArray(grid.size) { Double.NaN }
        var j = -1
        for (i in grid.indices) {
            while (j + 1 < t.size && t[j + 1] <= grid[i] + 0.0005) j++
            if (j >= 0) out[i] = v[j]
        }
        return out
    }
}

/** All signals of a session, in the order the log lists them. */
class SignalSet(val series: List<SignalSeries>) {
    fun find(bus: Int, message: String, signal: String): SignalSeries? =
        series.firstOrNull { it.bus == bus && it.message == message && it.signal == signal }

    /** Look an SBG signal up by the short message name, e.g. `find("EKF_EULER", "ROLL")`. */
    fun sbg(message: String, signal: String): SignalSeries? = find(2, SBG_PREFIX + message, signal)

    val hasEllipse: Boolean get() = series.any { it.bus == 2 && it.message.startsWith(SBG_PREFIX) }
}

/** Parse the `signals` CSV export (`session,t_s,bus,message,signal,raw,physical,unit`). */
fun parseSignalsCsv(reader: BufferedReader): SignalSet {
    reader.readLine() ?: return SignalSet(emptyList())
    class Acc(val bus: Int, val message: String, val signal: String, val unit: String) {
        val t = ArrayList<Double>()
        val v = ArrayList<Double>()
    }
    val acc = LinkedHashMap<String, Acc>()
    while (true) {
        val line = reader.readLine() ?: break
        if (line.isBlank()) continue
        val f = line.split(',')
        if (f.size < 8) continue
        val bus = f[2].toIntOrNull() ?: continue
        val a = acc.getOrPut("$bus|${f[3]}|${f[4]}") { Acc(bus, f[3], f[4], f[7]) }
        a.t.add(f[1].toDoubleOrNull() ?: continue)
        a.v.add(f[6].toDoubleOrNull() ?: Double.NaN)
    }
    return SignalSet(acc.values.map { SignalSeries(it.bus, it.message, it.signal, it.unit, it.t.toDoubleArray(), it.v.toDoubleArray()) })
}

/** What one CAN identifier did during a session. */
data class CanId(
    val bus: Int,
    val id: Long,
    val extended: Boolean,
    val name: String?,
    val count: Long,
    val firstUs: Long,
    val lastUs: Long,
    val dlc: Int,
    val lastHex: String,
) {
    /** Mean rate over the time between the first and last frame, Hz. */
    val rateHz: Double get() = if (count > 1 && lastUs > firstUs) (count - 1) * 1e6 / (lastUs - firstUs) else Double.NaN

    val idText: String get() = "0x" + id.toString(16).uppercase().padStart(if (extended) 8 else 3, '0')

    val shortName: String? get() = name?.removePrefix(SBG_PREFIX)
}

/** Parse the JSON array written for `can-summary`. */
fun parseCanSummary(json: String): List<CanId> {
    val a = JSONArray(json)
    return (0 until a.length()).map { i ->
        val o = a.getJSONObject(i)
        CanId(
            bus = o.getInt("bus"),
            id = o.getLong("id"),
            extended = o.getBoolean("extended"),
            name = if (o.has("name")) o.getString("name") else null,
            count = o.getLong("count"),
            firstUs = o.getLong("firstUs"),
            lastUs = o.getLong("lastUs"),
            dlc = o.getInt("dlc"),
            lastHex = o.getString("last"),
        )
    }
}

/** Everything the CAN tab shows. */
class CanData(val signals: SignalSet, val ids: List<CanId>)
