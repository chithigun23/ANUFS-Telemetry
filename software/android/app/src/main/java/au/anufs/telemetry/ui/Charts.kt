package au.anufs.telemetry.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import au.anufs.telemetry.core.indexAtTime
import java.util.Locale
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

/** Times (seconds since the first sample) of the start/finish crossings; every [LineChart] marks them. */
val LocalLapMarks = androidx.compose.runtime.compositionLocalOf { DoubleArray(0) }

/** One line on a [LineChart]. NaN breaks the line. */
class Series(val name: String, val values: DoubleArray, val color: Color, val dashed: Boolean = false)

// Okabe-Ito colours: distinguishable with common colour-vision deficiencies.
val SeriesX = Color(0xFFD55E00)
val SeriesY = Color(0xFF009E73)
val SeriesZ = Color(0xFF0072B2)
val SeriesAlt = Color(0xFFE69F00)

private fun fmt(v: Double, digits: Int = 2): String =
    if (v.isNaN()) "-" else String.format(Locale.US, "%.${digits}f", v)

/**
 * Time-series chart with a draggable cursor shared with the other charts and the map.
 * Tap or drag to move the cursor; the values at the cursor are printed above the plot.
 */
@Composable
fun LineChart(
    title: String,
    unit: String,
    t: DoubleArray,
    series: List<Series>,
    cursor: Int,
    onCursor: (Int) -> Unit,
    modifier: Modifier = Modifier,
    digits: Int = 2,
    /** Angles in degrees that wrap at 360: fixed 0 to 360 axis, and no line drawn across a wrap. */
    wrapDegrees: Boolean = false,
) {
    val measurer = rememberTextMeasurer()
    val labelColor = MaterialTheme.colorScheme.onSurfaceVariant
    val gridColor = MaterialTheme.colorScheme.outlineVariant
    val cursorColor = MaterialTheme.colorScheme.onSurface
    val lapMarks = LocalLapMarks.current
    val markColor = MaterialTheme.colorScheme.tertiary
    val labelStyle = TextStyle(fontSize = 10.sp, color = labelColor)

    Column(modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp)) {
        Row {
            Text(
                "$title ($unit)",
                style = MaterialTheme.typography.labelLarge,
                modifier = Modifier.padding(end = 12.dp),
            )
            val i = cursor.coerceIn(0, max(0, t.size - 1))
            series.forEach { s ->
                Text(
                    "${s.name} ${fmt(s.values.getOrNull(i) ?: Double.NaN, digits)}  ",
                    style = MaterialTheme.typography.labelMedium,
                    color = s.color,
                )
            }
        }
        val left = 44f
        Canvas(
            Modifier
                .fillMaxWidth()
                .height(140.dp)
                .pointerInput(t) {
                    fun setFrom(x: Float) {
                        if (t.isEmpty()) return
                        val f = ((x - left) / (size.width - left)).coerceIn(0f, 1f)
                        onCursor(indexAtTime(t, t.first() + f * (t.last() - t.first())))
                    }
                    detectTapGestures { setFrom(it.x) }
                }
                .pointerInput(t) {
                    fun setFrom(x: Float) {
                        if (t.isEmpty()) return
                        val f = ((x - left) / (size.width - left)).coerceIn(0f, 1f)
                        onCursor(indexAtTime(t, t.first() + f * (t.last() - t.first())))
                    }
                    detectHorizontalDragGestures { change, _ -> setFrom(change.position.x) }
                },
        ) {
            if (t.size < 2) return@Canvas
            var lo = Double.POSITIVE_INFINITY
            var hi = Double.NEGATIVE_INFINITY
            for (s in series) for (v in s.values) if (!v.isNaN()) {
                lo = min(lo, v)
                hi = max(hi, v)
            }
            if (lo > hi) return@Canvas
            if (hi - lo < 1e-9) {
                lo -= 0.5
                hi += 0.5
            }
            val pad = (hi - lo) * 0.08
            lo -= pad
            hi += pad
            if (wrapDegrees) {
                lo = 0.0
                hi = 360.0
            }
            val w = size.width - left
            val h = size.height
            val t0 = t.first()
            val span = t.last() - t0
            fun x(i: Int) = left + (((t[i] - t0) / span).toFloat()) * w
            fun y(v: Double) = h - (((v - lo) / (hi - lo)).toFloat()) * h

            for (k in 0..3) {
                val v = lo + (hi - lo) * k / 3
                val yy = y(v)
                drawLine(gridColor, Offset(left, yy), Offset(size.width, yy), strokeWidth = 1f)
                val label = fmt(v, digits)
                val m = measurer.measure(label, labelStyle)
                drawText(m, topLeft = Offset(left - m.size.width - 4f, (yy - m.size.height / 2f).coerceIn(0f, h - m.size.height)))
            }
            for (s in series) {
                val path = Path()
                var pen = false
                for (i in t.indices) {
                    val v = s.values.getOrNull(i) ?: Double.NaN
                    if (v.isNaN()) {
                        pen = false
                        continue
                    }
                    val jumped = wrapDegrees && i > 0 && abs(v - (s.values.getOrNull(i - 1) ?: v)) > 180.0
                    if (pen && !jumped) path.lineTo(x(i), y(v)) else path.moveTo(x(i), y(v))
                    pen = true
                }
                drawPath(
                    path, s.color,
                    style = Stroke(width = 2f, pathEffect = if (s.dashed) PathEffect.dashPathEffect(floatArrayOf(14f, 9f)) else null),
                )
            }
            for (m in lapMarks) {
                if (m < t0 || m > t.last()) continue
                val xm = left + (((m - t0) / span).toFloat()) * w
                drawLine(markColor, Offset(xm, 0f), Offset(xm, h), strokeWidth = 1.5f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(6f, 6f)))
            }
            val c = cursor.coerceIn(0, t.size - 1)
            drawLine(cursorColor, Offset(x(c), 0f), Offset(x(c), h), strokeWidth = 1.5f)
        }
        Row {
            Text(
                "0 s",
                style = MaterialTheme.typography.labelSmall,
                color = labelColor,
                modifier = Modifier.padding(start = 44.dp),
            )
            Text(
                "  t = ${fmt(t.getOrElse(cursor) { 0.0 }, 1)} s   of ${fmt(t.lastOrNull() ?: 0.0, 1)} s",
                style = MaterialTheme.typography.labelSmall,
                color = labelColor,
            )
        }
    }
}

/**
 * X-versus-Y scatter with equal axes and rings at whole units, used for the g-g diagram.
 * The cursor sample is drawn as a ring.
 */
@Composable
fun ScatterChart(
    title: String,
    xs: DoubleArray,
    ys: DoubleArray,
    cursor: Int,
    modifier: Modifier = Modifier,
    xLabel: String = "x",
    yLabel: String = "y",
    /** A second point set drawn in another colour, e.g. the other source. */
    xs2: DoubleArray? = null,
    ys2: DoubleArray? = null,
) {
    val measurer = rememberTextMeasurer()
    val labelColor = MaterialTheme.colorScheme.onSurfaceVariant
    val gridColor = MaterialTheme.colorScheme.outlineVariant
    val dotColor = SeriesZ
    val cursorColor = SeriesX
    Column(modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp)) {
        Text("$title  ($xLabel across, $yLabel up)", style = MaterialTheme.typography.labelLarge)
        Canvas(Modifier.fillMaxWidth().height(260.dp)) {
            var m = 0.5
            for (a in listOfNotNull(xs, ys, xs2, ys2)) for (v in a) if (!v.isNaN()) m = max(m, abs(v))
            m = kotlin.math.ceil(m * 2) / 2 // whole half-units
            val side = min(size.width, size.height)
            val origin = Offset(size.width / 2, size.height / 2)
            val scale = (side / 2 - 12f) / m.toFloat()
            var r = 0.5
            while (r <= m + 1e-9) {
                drawCircle(gridColor, radius = (r * scale).toFloat(), center = origin, style = Stroke(1f))
                val label = String.format(Locale.US, "%.1f", r)
                val tm = measurer.measure(label, TextStyle(fontSize = 10.sp, color = labelColor))
                drawText(tm, topLeft = Offset(origin.x + 3f, origin.y - (r * scale).toFloat() - tm.size.height))
                r += 0.5
            }
            drawLine(gridColor, Offset(origin.x - side / 2, origin.y), Offset(origin.x + side / 2, origin.y), 1f)
            drawLine(gridColor, Offset(origin.x, origin.y - side / 2), Offset(origin.x, origin.y + side / 2), 1f)
            fun px(v: Double) = origin.x + (v * scale).toFloat()
            fun py(v: Double) = origin.y - (v * scale).toFloat()
            for (i in xs.indices) {
                if (xs[i].isNaN() || ys[i].isNaN()) continue
                drawCircle(dotColor.copy(alpha = 0.55f), radius = 3f, center = Offset(px(xs[i]), py(ys[i])))
            }
            if (xs2 != null && ys2 != null) {
                for (i in xs2.indices) {
                    if (xs2[i].isNaN() || ys2[i].isNaN()) continue
                    drawCircle(SeriesAlt.copy(alpha = 0.55f), radius = 3f, center = Offset(px(xs2[i]), py(ys2[i])))
                }
            }
            val c = cursor.coerceIn(0, max(0, xs.size - 1))
            if (c < xs.size && !xs[c].isNaN() && !ys[c].isNaN()) {
                drawCircle(cursorColor, radius = 9f, center = Offset(px(xs[c]), py(ys[c])), style = Stroke(3f))
            }
        }
    }
}
