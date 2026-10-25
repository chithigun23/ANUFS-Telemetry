package au.anufs.telemetry.core

import org.json.JSONObject

/** The log reader reported an error (bad file, card not prepared, ...). */
class LogReadException(message: String) : Exception(message)

/** One stretch of consecutive blocks of a session, with the session header if it was found. */
data class Run(
    val session: Int,
    val blocks: Int,
    val firstSeq: Int,
    val lastSeq: Int,
    val cleanClose: Boolean,
    val gapBefore: Int,
    val firmware: String?,
    val utcUnixMs: Long?,
    val fixIntervalMs: Int?,
)

/** What a scan of a card or image found. */
data class CardScan(
    val cardId: Long,
    val regionBlocks: Long,
    val validBlocks: Long,
    val invalidBlocks: Long,
    val foreignBlocks: Long,
    val readErrors: Long,
    val runs: List<Run>,
) {
    /** Runs grouped by session number, in card order. */
    val sessions: List<Session>
        get() = runs.groupBy { it.session }.map { (n, rs) ->
            Session(
                number = n,
                blocks = rs.sumOf { it.blocks },
                cleanClose = rs.all { it.cleanClose },
                damaged = rs.size > 1 || rs.any { it.gapBefore > 0 },
                firmware = rs.firstNotNullOfOrNull { it.firmware },
                utcUnixMs = rs.firstNotNullOfOrNull { it.utcUnixMs }?.takeIf { it != 0L },
            )
        }
}

/** A logging session, possibly spread over several runs if blocks were lost. */
data class Session(
    val number: Int,
    val blocks: Int,
    val cleanClose: Boolean,
    val damaged: Boolean,
    val firmware: String?,
    val utcUnixMs: Long?,
)

/** Parse the JSON from [Native.scan]. Throws [LogReadException] for `{"error": ...}`. */
fun parseScan(json: String): CardScan {
    val o = JSONObject(json)
    if (o.has("error")) throw LogReadException(o.getString("error"))
    val runs = o.getJSONArray("runs")
    return CardScan(
        cardId = o.getLong("cardId"),
        regionBlocks = o.getLong("regionBlocks"),
        validBlocks = o.getLong("validBlocks"),
        invalidBlocks = o.getLong("invalidBlocks"),
        foreignBlocks = o.getLong("foreignBlocks"),
        readErrors = o.getLong("readErrors"),
        runs = (0 until runs.length()).map { i ->
            val r = runs.getJSONObject(i)
            Run(
                session = r.getInt("session"),
                blocks = r.getInt("blocks"),
                firstSeq = r.getInt("firstSeq"),
                lastSeq = r.getInt("lastSeq"),
                cleanClose = r.getBoolean("cleanClose"),
                gapBefore = r.getInt("gapBefore"),
                firmware = if (r.has("firmware")) r.getString("firmware") else null,
                utcUnixMs = if (r.has("utcUnixMs")) r.getLong("utcUnixMs") else null,
                fixIntervalMs = if (r.has("fixIntervalMs")) r.getInt("fixIntervalMs") else null,
            )
        },
    )
}

/** Number of rows from the JSON returned by [Native.export]. Throws [LogReadException] on error. */
fun parseExportRows(json: String): Long {
    val o = JSONObject(json)
    if (o.has("error")) throw LogReadException(o.getString("error"))
    return o.getLong("rows")
}
