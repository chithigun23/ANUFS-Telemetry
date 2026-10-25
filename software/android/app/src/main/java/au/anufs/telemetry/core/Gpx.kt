package au.anufs.telemetry.core

import java.time.Instant
import java.util.Locale

/** A GPX document and how many points it holds. */
class GpxDocument(val xml: String, val points: Int)

private const val NS = "https://github.com/chithigun23/ANUFS-Telemetry/gpx/1"

/** A silence longer than this between two Ellipse positions starts a new segment, as in the onboard export. */
private const val SEGMENT_GAP_S = 2.0

/**
 * A track as GPX 1.1: one point per sample that has a position, a new segment after a loss of the
 * position or a gap of more than [SEGMENT_GAP_S]. [speedMs] and [courseDeg] go in the private
 * extensions; any array may be NaN where unknown.
 */
fun trackGpx(
    samples: Samples,
    lat: DoubleArray,
    lon: DoubleArray,
    altM: DoubleArray,
    speedMs: DoubleArray,
    courseDeg: DoubleArray,
    name: String,
): GpxDocument {
    fun f(v: Double, d: Int) = String.format(Locale.US, "%.${d}f", v)
    val sb = StringBuilder()
    sb.append("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
    sb.append("<gpx version=\"1.1\" creator=\"ANUFS telemetry\" xmlns=\"http://www.topografix.com/GPX/1/1\" xmlns:tel=\"$NS\">\n")
    sb.append("  <trk>\n    <name>$name</name>\n")
    var open = false
    var last = Double.NaN
    var points = 0
    for (i in 0 until samples.size) {
        if (lat[i].isNaN() || lon[i].isNaN()) {
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
        sb.append("      <trkpt lat=\"${f(lat[i], 9)}\" lon=\"${f(lon[i], 9)}\">\n")
        if (!altM[i].isNaN()) sb.append("        <ele>${f(altM[i], 3)}</ele>\n")
        if (samples.utcMs[i] != Samples.NO_TIME) sb.append("        <time>${Instant.ofEpochMilli(samples.utcMs[i])}</time>\n")
        if (!speedMs[i].isNaN() && !courseDeg[i].isNaN()) {
            sb.append("        <extensions><tel:speed>${f(speedMs[i], 3)}</tel:speed><tel:course>${f(courseDeg[i], 2)}</tel:course></extensions>\n")
        }
        sb.append("      </trkpt>\n")
        points++
        last = t
    }
    if (open) sb.append("    </trkseg>\n")
    sb.append("  </trk>\n</gpx>\n")
    return GpxDocument(sb.toString(), points)
}

/** The Ellipse's own track, built from the logged `EKF_POS`, `EKF_ALTITUDE` and `EKF_VEL_NED` signals. */
fun ellipseGpx(samples: Samples, e: EllipseView, session: Int): GpxDocument =
    trackGpx(
        samples, e.lat, e.lon, e.altM,
        DoubleArray(samples.size) { e.speedKmh[it] / 3.6 }, e.yawDeg,
        "ANUFS telemetry session $session (SBG Ellipse)",
    )

/** The fused best-estimate track. */
fun fusedGpx(samples: Samples, f: FusedTrack, session: Int): GpxDocument =
    trackGpx(
        samples, f.lat, f.lon, f.altM,
        DoubleArray(samples.size) { f.speedKmh[it] / 3.6 }, f.courseDeg,
        "ANUFS telemetry session $session (fused best estimate)",
    )
