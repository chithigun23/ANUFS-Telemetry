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
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import au.anufs.telemetry.core.GeoLine
import au.anufs.telemetry.core.LapResult
import au.anufs.telemetry.core.Samples
import au.anufs.telemetry.core.Settings
import au.anufs.telemetry.core.indexAtTime
import java.util.Locale

/** `1:02.314`, or `-` when the time is not a number. */
fun lapTimeText(seconds: Double): String {
    if (seconds.isNaN()) return "-"
    val ms = Math.round(seconds * 1000)
    return String.format(Locale.US, "%d:%02d.%03d", ms / 60000, ms / 1000 % 60, ms % 1000)
}

/** Laps against the start/finish line, with the ways to set the line. */
@Composable
fun LapsTab(
    samples: Samples,
    laps: LapResult?,
    line: GeoLine?,
    settings: Settings,
    onCursor: (Int) -> Unit,
    onDraw: () -> Unit,
    onSetAtCursor: () -> Unit,
    onClear: () -> Unit,
) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (line == null) {
                Text("No start/finish line yet", style = MaterialTheme.typography.titleMedium)
                Text(
                    "Draw one across the track on the map with the S Pen, or scrub the cursor to the moment the car is on the line and set it there. " +
                        "The line is remembered, so the next session on the same track finds its laps by itself.",
                    style = MaterialTheme.typography.bodyMedium,
                )
            } else {
                Text(
                    String.format(Locale.US, "Start/finish line, %.0f m wide", line.lengthMetres),
                    style = MaterialTheme.typography.titleMedium,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = onDraw) { Text(if (line == null) "Draw on map" else "Redraw") }
                OutlinedButton(onClick = onSetAtCursor) { Text("Set at cursor") }
                if (line != null) OutlinedButton(onClick = onClear) { Text("Remove") }
            }
        }
        if (laps == null) return@Column
        if (laps.laps.isEmpty()) {
            Text(
                "This line did not find a complete lap. The car has to cross it at least twice, in the same direction, more than " +
                    "${settings.minLapSeconds} s apart (Settings, Laps).",
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(horizontal = 16.dp),
            )
            return@Column
        }
        val best = laps.bestIndex
        val bestTime = laps.laps[best].timeS
        Text(
            "${laps.laps.size} laps  ·  best ${lapTimeText(bestTime)} (lap ${laps.laps[best].number})",
            style = MaterialTheme.typography.titleSmall,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        )
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp)) {
            Header("Lap", 0.6f)
            Header("Time", 1.3f)
            Header("Δ best", 1f)
            Header("Avg ${settings.speedUnit.label}", 1f)
            Header("Max", 0.9f)
        }
        laps.laps.forEachIndexed { i, lap ->
            val isBest = i == best
            Row(
                Modifier.fillMaxWidth()
                    .clickable { onCursor(indexAtTime(samples.t, lap.startS)) }
                    .padding(horizontal = 16.dp, vertical = 8.dp),
            ) {
                val weight = if (isBest) FontWeight.Bold else FontWeight.Normal
                Cell("${lap.number}", 0.6f, weight)
                Cell(lapTimeText(lap.timeS), 1.3f, weight)
                Cell(if (isBest) "best" else String.format(Locale.US, "+%.3f", lap.timeS - bestTime), 1f, weight)
                Cell(String.format(Locale.US, "%.1f", settings.speed(lap.avgKmh)), 1f, weight)
                Cell(String.format(Locale.US, "%.0f", settings.speed(lap.maxKmh)), 0.9f, weight)
            }
        }
        Text(
            "Out lap " + lapTimeText(laps.outLapS) + " and in lap " + lapTimeText(laps.inLapS) + " are not counted. Tap a lap to move the cursor to its start; the dashed lines on the charts are the crossings.",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(16.dp),
        )
    }
}

@Composable
private fun androidx.compose.foundation.layout.RowScope.Header(text: String, weight: Float) {
    Text(text, Modifier.weight(weight), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
private fun androidx.compose.foundation.layout.RowScope.Cell(text: String, weight: Float, fontWeight: FontWeight) {
    Text(text, Modifier.weight(weight), fontFamily = FontFamily.Monospace, fontSize = 13.sp, fontWeight = fontWeight)
}
