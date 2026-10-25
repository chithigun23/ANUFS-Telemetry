package au.anufs.telemetry.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import au.anufs.telemetry.core.CanData
import au.anufs.telemetry.core.CanId
import au.anufs.telemetry.core.EllipseView
import au.anufs.telemetry.core.Samples
import au.anufs.telemetry.core.SignalSeries
import au.anufs.telemetry.core.describeSolution
import java.util.Locale

private val Mono = FontFamily.Monospace

@Composable
private fun SectionTitle(text: String) {
    Text(text, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 2.dp))
}

@Composable
private fun Note(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
}

/** CAN: bus health, the Ellipse against the onboard sensors, every logged signal, and the raw ids. */
@Composable
fun CanTab(samples: Samples, can: CanData?, busy: Boolean, cursor: Int, onCursor: (Int) -> Unit) {
    // Charts here always draw both sources, onboard solid and Ellipse dashed.
    if (can == null) {
        Column(Modifier.fillMaxSize().padding(24.dp)) {
            if (busy) CircularProgressIndicator() else Text("No CAN data loaded.")
        }
        return
    }
    val settings = LocalSettings.current
    val ellipse = remember(samples, can) { EllipseView(samples, can.signals) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        SectionTitle("Buses")
        BusSummary(samples, can)

        SectionTitle("Onboard and Ellipse side by side")
        if (!ellipse.present) {
            Note(
                "No Ellipse signals in this session. The Ellipse sends nothing on CAN until its messages are enabled " +
                    "with SBG's serial or Ethernet tools; CAN cannot configure it.",
            )
        } else {
            EllipseStatus(ellipse, cursor)
            LineChart(
                "Speed", settings.speedUnit.label, samples.t,
                listOf(Series("onboard", settings.speed(samples.speedKmh), SeriesZ), Series("Ellipse", settings.speed(ellipse.speedKmh), SeriesX, dashed = true)),
                cursor, onCursor, digits = 1,
            )
            LineChart(
                "Heading", "°", samples.t,
                listOf(
                    Series("Ellipse yaw", ellipse.yawDeg, SeriesX, dashed = true),
                    Series("dual-antenna", samples["heading_deg"], SeriesY),
                    Series("course", samples["cog_deg"], SeriesZ),
                ),
                cursor, onCursor, digits = 1, wrapDegrees = true,
            )
            LineChart(
                "Distance between the two positions", "m", samples.t,
                listOf(Series("gap", ellipse.positionGapMetres, SeriesAlt)),
                cursor, onCursor, digits = 2,
            )
            LineChart(
                "Acceleration X", settings.accelUnit.label, samples.t,
                listOf(Series("onboard", settings.accel(samples["accel_mean_x_g"]), SeriesZ), Series("Ellipse", settings.accel(ellipse.accelXg), SeriesX, dashed = true)),
                cursor, onCursor, digits = 3,
            )
            LineChart(
                "Acceleration Y", settings.accelUnit.label, samples.t,
                listOf(Series("onboard", settings.accel(samples["accel_mean_y_g"]), SeriesZ), Series("Ellipse", settings.accel(ellipse.accelYg), SeriesX, dashed = true)),
                cursor, onCursor, digits = 3,
            )
            LineChart(
                "Roll and pitch (Ellipse)", "°", samples.t,
                listOf(Series("roll", ellipse.rollDeg, SeriesX), Series("pitch", ellipse.pitchDeg, SeriesY)),
                cursor, onCursor, digits = 2,
            )
            Note("Two independent sources, neither taken as truth. The IMUs only agree where their axes point the same way: a steady difference is mounting, a growing one is drift. The position distance is how far apart the two receivers put the car.")
        }

        SectionTitle("Logged signals")
        SignalBrowser(samples, can.signals.series, cursor, onCursor)

        SectionTitle("Raw frames")
        RawFrames(can.ids)
    }
}

@Composable
private fun BusSummary(samples: Samples, can: CanData) {
    val dur = samples.durationSeconds
    Card(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            for ((bus, label, aliveCol, framesCol) in listOf(
                BusRow(1, "CAN1  MoTeC", "can1_alive", "can1_frames"),
                BusRow(2, "CAN2  SBG Ellipse", "can2_alive", "can2_frames"),
            )) {
                val alive = samples[aliveCol].filter { !it.isNaN() }
                val pct = if (alive.isEmpty()) 0 else (alive.sum() * 100 / alive.size).toInt()
                val frames = samples[framesCol].filter { !it.isNaN() }.maxOrNull() ?: 0.0
                val ids = can.ids.count { it.bus == bus }
                Text(label, style = MaterialTheme.typography.titleSmall)
                Text(
                    String.format(Locale.US, "alive %d%% of samples  ·  %.0f frames (%.0f/s)  ·  %d ids", pct, frames, if (dur > 0) frames / dur else 0.0, ids),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
    }
}

private data class BusRow(val bus: Int, val label: String, val aliveCol: String, val framesCol: String)

@Composable
private fun EllipseStatus(e: EllipseView, cursor: Int) {
    val i = cursor.coerceIn(0, maxOf(0, e.solution.size - 1))
    val w = e.solution.getOrNull(i) ?: Double.NaN
    Note(if (w.isNaN()) "EKF status: not received yet" else "EKF status: " + describeSolution(w.toLong()))
}

@Composable
private fun SignalBrowser(samples: Samples, all: List<SignalSeries>, cursor: Int, onCursor: (Int) -> Unit) {
    var picked by remember { mutableStateOf(listOf<String>()) }
    val key = { s: SignalSeries -> "${s.bus}|${s.message}|${s.signal}" }
    if (all.isEmpty()) {
        Note("The logger recorded no decoded CAN signals in this session.")
        return
    }
    picked.mapNotNull { k -> all.firstOrNull { key(it) == k } }.forEach { s ->
        LineChart(s.label, s.unit.ifEmpty { "raw" }, samples.t, listOf(Series(s.signal, s.heldOn(samples.tAbs), SeriesZ)), cursor, onCursor, digits = 3)
    }
    Note("Tap a signal to plot it. Values are the newest received before each 100 ms sample.")
    Column(Modifier.padding(horizontal = 12.dp)) {
        all.forEach { s ->
            val on = key(s) in picked
            Row(
                Modifier.fillMaxWidth().clickable { picked = if (on) picked - key(s) else picked + key(s) }.padding(vertical = 6.dp, horizontal = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(if (on) "●" else "○", color = if (on) SeriesZ else Color.Gray)
                Text("CAN${s.bus}", style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(top = 2.dp))
                Text(s.label, style = MaterialTheme.typography.bodySmall, modifier = Modifier.weight(1f), fontFamily = Mono, fontSize = 11.sp)
                Text(
                    String.format(Locale.US, "%.3g … %.3g %s", s.min, s.max, s.unit),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun RawFrames(ids: List<CanId>) {
    if (ids.isEmpty()) {
        Note("No raw CAN frames in this session.")
        return
    }
    Note("Every frame the logger saw, so layouts that are not decoded yet (MoTeC) can be worked out later.")
    Column(Modifier.padding(horizontal = 12.dp, vertical = 4.dp)) {
        Row(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
            Text("bus id", Modifier.weight(1.1f), style = MaterialTheme.typography.labelSmall)
            Text("frames", Modifier.weight(0.9f), style = MaterialTheme.typography.labelSmall)
            Text("Hz", Modifier.weight(0.6f), style = MaterialTheme.typography.labelSmall)
            Text("last data", Modifier.weight(1.8f), style = MaterialTheme.typography.labelSmall)
        }
        ids.forEach { c ->
            Column(Modifier.padding(vertical = 3.dp)) {
                Row(Modifier.fillMaxWidth()) {
                    Text("${c.bus}  ${c.idText}", Modifier.weight(1.1f), fontFamily = Mono, fontSize = 11.sp)
                    Text("${c.count}", Modifier.weight(0.9f), fontFamily = Mono, fontSize = 11.sp)
                    Text(if (c.rateHz.isNaN()) "-" else String.format(Locale.US, "%.0f", c.rateHz), Modifier.weight(0.6f), fontFamily = Mono, fontSize = 11.sp)
                    Text(c.lastHex, Modifier.weight(1.8f), fontFamily = Mono, fontSize = 11.sp)
                }
                c.shortName?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
    }
}
