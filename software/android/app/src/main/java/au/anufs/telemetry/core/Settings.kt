package au.anufs.telemetry.core

/** Light, dark, or whatever the phone is set to. */
enum class ThemeMode(val label: String) { System("System"), Light("Light"), Dark("Dark") }

/** Unit for speeds and, with it, distances. Values are logged in m/s and shown converted. */
enum class SpeedUnit(val label: String, val distanceLabel: String, internal val perKmh: Double, internal val perKm: Double) {
    KilometresPerHour("km/h", "km", 1.0, 1.0),
    MilesPerHour("mph", "mi", 0.621371192, 0.621371192),
    MetresPerSecond("m/s", "km", 1.0 / 3.6, 1.0),
}

/** Unit for accelerations. The g-g diagram always uses g. */
enum class AccelUnit(val label: String, internal val perG: Double) {
    G("g", 1.0),
    MetresPerSecondSquared("m/s²", 9.80665),
}

/** Which source a session opens on when it has Ellipse data. */
enum class DefaultSource(val label: String) {
    Onboard("Onboard"),
    Ellipse("Ellipse"),
    Both("Both"),
    LastUsed("Last used"),
}

/** What the user can change. New fields get a default, so old saved settings still load. */
data class Settings(
    val theme: ThemeMode = ThemeMode.System,
    val speedUnit: SpeedUnit = SpeedUnit.KilometresPerHour,
    val accelUnit: AccelUnit = AccelUnit.G,
    val defaultSource: DefaultSource = DefaultSource.Onboard,
    /** The source chosen last, for [DefaultSource.LastUsed]: "Onboard", "Ellipse" or "Both". */
    val lastSource: String = "Onboard",
) {
    /** Speeds from km/h into the chosen unit. NaN stays NaN. */
    fun speed(kmh: DoubleArray): DoubleArray = scaled(kmh, speedUnit.perKmh)

    fun speed(kmh: Double): Double = kmh * speedUnit.perKmh

    /** Distances from kilometres into the chosen unit. */
    fun distance(km: Double): Double = km * speedUnit.perKm

    /** Accelerations from g into the chosen unit. */
    fun accel(g: DoubleArray): DoubleArray = scaled(g, accelUnit.perG)

    private fun scaled(a: DoubleArray, f: Double): DoubleArray = if (f == 1.0) a else DoubleArray(a.size) { a[it] * f }

    /** The key/value form that is saved. */
    fun toMap(): Map<String, String> = mapOf(
        "theme" to theme.name,
        "speedUnit" to speedUnit.name,
        "accelUnit" to accelUnit.name,
        "defaultSource" to defaultSource.name,
        "lastSource" to lastSource,
    )

    companion object {
        /** Rebuild from saved key/value pairs, using the default for anything missing or unrecognised. */
        fun from(get: (String) -> String?): Settings {
            val d = Settings()
            return Settings(
                theme = parse(get("theme"), d.theme),
                speedUnit = parse(get("speedUnit"), d.speedUnit),
                accelUnit = parse(get("accelUnit"), d.accelUnit),
                defaultSource = parse(get("defaultSource"), d.defaultSource),
                lastSource = get("lastSource")?.takeIf { it in setOf("Onboard", "Ellipse", "Both") } ?: d.lastSource,
            )
        }

        private inline fun <reified E : Enum<E>> parse(name: String?, default: E): E =
            enumValues<E>().firstOrNull { it.name == name } ?: default
    }
}
