package au.anufs.telemetry.core

import kotlin.math.PI
import kotlin.math.hypot

/**
 * The SBG Ellipse's signals laid on the log's sample grid, with the derived quantities the
 * comparison charts need. Every array has one value per sample, NaN where the Ellipse had not
 * yet spoken. The Ellipse values are the last ones received before each sample, so they can be
 * up to one CAN period (10 ms at 100 Hz) old.
 */
class EllipseView(private val samples: Samples, private val signals: SignalSet) {
    private fun held(message: String, signal: String): DoubleArray =
        signals.sbg(message, signal)?.heldOn(samples.tAbs) ?: DoubleArray(samples.size) { Double.NaN }

    val present: Boolean = signals.hasEllipse

    private fun deg(rad: DoubleArray) = DoubleArray(rad.size) { rad[it] * 180 / PI }

    val rollDeg: DoubleArray = deg(held("EKF_EULER", "ROLL"))
    val pitchDeg: DoubleArray = deg(held("EKF_EULER", "PITCH"))

    /** Yaw as a compass bearing, 0 to 360 clockwise from north (the Ellipse reports -180 to 180). */
    val yawDeg: DoubleArray = deg(held("EKF_EULER", "YAW")).let { y -> DoubleArray(y.size) { if (y[it].isNaN()) Double.NaN else ((y[it] % 360) + 360) % 360 } }

    private val vn = held("EKF_VEL_NED", "VELOCITY_N")
    private val ve = held("EKF_VEL_NED", "VELOCITY_E")

    /** Ground speed from the north and east velocities, km/h. */
    val speedKmh: DoubleArray = DoubleArray(samples.size) { hypot(vn[it], ve[it]) * 3.6 }

    val lat: DoubleArray = held("EKF_POS", "LATITUDE")
    val lon: DoubleArray = held("EKF_POS", "LONGITUDE")

    /** Distance between the onboard GNSS position and the Ellipse's, metres. */
    val positionGapMetres: DoubleArray = DoubleArray(samples.size) {
        if (samples.hasFix(it) && !lat[it].isNaN() && !lon[it].isNaN()) haversine(samples.lat[it], samples.lon[it], lat[it], lon[it]) else Double.NaN
    }

    private fun g(a: DoubleArray) = DoubleArray(a.size) { a[it] / 9.80665 }
    val accelXg: DoubleArray = g(held("IMU_ACCEL", "ACCEL_X"))
    val accelYg: DoubleArray = g(held("IMU_ACCEL", "ACCEL_Y"))
    val accelZg: DoubleArray = g(held("IMU_ACCEL", "ACCEL_Z"))
    val gyroXdps: DoubleArray = deg(held("IMU_GYRO", "GYRO_X"))
    val gyroYdps: DoubleArray = deg(held("IMU_GYRO", "GYRO_Y"))
    val gyroZdps: DoubleArray = deg(held("IMU_GYRO", "GYRO_Z"))

    /** Altitude above mean sea level from `EKF_ALTITUDE`, metres. */
    val altM: DoubleArray = held("EKF_ALTITUDE", "ALTITUDE")

    /** True where the Ellipse has a position for this sample. */
    fun hasFix(i: Int): Boolean = !lat[i].isNaN() && !lon[i].isNaN()

    /** The EKF solution status word of each sample (`STATUS_03.SOLUTION`), NaN if never received. */
    val solution: DoubleArray = held("STATUS_03", "SOLUTION")

    /** Difference between two angle series in degrees, folded into -180..180. */
    fun angleGap(a: DoubleArray, b: DoubleArray): DoubleArray = DoubleArray(a.size) {
        if (a[it].isNaN() || b[it].isNaN()) Double.NaN else ((a[it] - b[it] + 540) % 360) - 180
    }
}

/**
 * Words for the EKF solution status word, `STATUS_03.SOLUTION`.
 *
 * The layout comes from SBG's sbgECom documentation, not from `sbgCan.dbc` (which carries only
 * the raw 32-bit field). It has not been checked against the Ellipse-D manual yet.
 */
fun describeSolution(word: Long): String {
    val mode = when ((word and 0xF).toInt()) {
        0 -> "Not initialised"
        1 -> "Vertical gyro"
        2 -> "AHRS (attitude only)"
        3 -> "Navigation, velocity"
        4 -> "Navigation, position"
        else -> "Unknown mode"
    }
    fun bits(vararg named: Pair<Int, String>) = named.filter { word and (1L shl it.first) != 0L }.joinToString(", ") { it.second }
    val valid = bits(4 to "attitude", 5 to "heading", 6 to "velocity", 7 to "position")
    val used = bits(
        8 to "vertical reference", 9 to "magnetometer",
        10 to "GPS1 velocity", 11 to "GPS1 position", 12 to "GPS1 course", 13 to "GPS1 dual-antenna heading",
        14 to "GPS2 velocity", 15 to "GPS2 position", 16 to "GPS2 course", 17 to "GPS2 dual-antenna heading",
        18 to "odometer",
    )
    return buildString {
        append(mode)
        if (valid.isNotEmpty()) append(" · valid: ").append(valid)
        if (used.isNotEmpty()) append(" · using: ").append(used)
    }
}
