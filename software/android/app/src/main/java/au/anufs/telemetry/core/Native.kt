package au.anufs.telemetry.core

/** The Rust log reader (`software/tools/tel-jni`). Both calls return JSON text. */
object Native {
    init {
        System.loadLibrary("tel_jni")
    }

    /** Sessions on a card image: see [parseScan]. */
    @JvmStatic
    external fun scan(path: String): String

    /** Write `kind` (`samples`, `signals`, `events`, `can-raw`, `gpx`) of one session to [outPath]. */
    @JvmStatic
    external fun export(path: String, session: Int, kind: String, outPath: String): String
}
