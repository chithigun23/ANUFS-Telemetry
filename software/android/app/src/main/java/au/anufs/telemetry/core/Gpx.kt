package au.anufs.telemetry.core

import java.time.Instant
import java.util.Locale

/** A GPX document and how many points it holds. */
class GpxDocument(val xml: String, val points: Int)

private const val NS = "https://github.com/chithigun23/ANUFS-Telemetry/gpx/1"

/** A silence longer than this between two Ellipse positions starts a new segment, as in the onboard export. */
private const val SEGMENT_GAP_S = 2.0

/**
 * The Ellipse's own track as GPX 1.1, one point per log sample that has an Ellipse position.
 * The onboard receiver's track comes from the Rust exporter; this one is built from the logged
 * `EKF_POS`, `EKF_ALTITUDE` and `EKF_VEL_NED` signals, so it is at the 10 Hz sample rate.
 */
fun ellipseGpx(samples: Samples, e: EllipseView, session: Int): GpxDocument {
    fun f(v: Double, d: Int) = String.format(Locale.US, "%.${d}f", v)
    val sb = StringBuilder()
    sb.append("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
    sb.append("<gpx version=\"1.1\" creator=\"ANUFS telemetry\" xmlns=\"http://www.topografix.com/GPX/1/1\" xmlns:tel=\"$NS\">\n")
    sb.append("  <trk>\n    <name>ANUFS telemetry session $session (SBG Ellipse)</name>\n")
    var open = false
    var last = Double.NaN
    var points = 0
    for (i in 0 until samples.size) {
        if (!e.hasFix(i)) {
            if (open) sb.append("    </trkseg>\n")
            open = false
            continue
        }
        val t = samples.tAbs[i]
        if (open && !last.isNaN() && t - last > SEGMENT_GAP_S) {
            sb.append("    </trkseg>\n")
            open = false
        }
        if (!open) {
            sb.append("    <trkseg>\n")
            open = true
        }
        sb.append("      <trkpt lat=\"${f(e.lat[i], 9)}\" lon=\"${f(e.lon[i], 9)}\">\n")
        if (!e.altM[i].isNaN()) sb.append("        <ele>${f(e.altM[i], 3)}</ele>\n")
        if (samples.utcMs[i] != Samples.NO_TIME) sb.append("        <time>${Instant.ofEpochMilli(samples.utcMs[i])}</time>\n")
        if (!e.speedKmh[i].isNaN() && !e.yawDeg[i].isNaN()) {
            sb.append("        <extensions><tel:speed>${f(e.speedKmh[i] / 3.6, 3)}</tel:speed><tel:course>${f(e.yawDeg[i], 2)}</tel:course></extensions>\n")
        }
        sb.append("      </trkpt>\n")
        points++
        last = t
    }
    if (open) sb.append("    </trkseg>\n")
    sb.append("  </trk>\n</gpx>\n")
    return GpxDocument(sb.toString(), points)
}
