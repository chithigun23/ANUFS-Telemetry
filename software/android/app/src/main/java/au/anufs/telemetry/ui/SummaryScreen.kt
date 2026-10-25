package au.anufs.telemetry.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import au.anufs.telemetry.core.CanData
import au.anufs.telemetry.core.EllipseView
import au.anufs.telemetry.core.FusedTrack
import au.anufs.telemetry.core.GeoLine
import au.anufs.telemetry.core.LapResult
import au.anufs.telemetry.core.Samples
import au.anufs.telemetry.core.Session
import au.anufs.telemetry.core.Settings
import au.anufs.telemetry.core.percentile
import au.anufs.telemetry.core.presence
import au.anufs.telemetry.core.rms
import au.anufs.telemetry.core.trackGap
import au.anufs.telemetry.core.trackLengthMetres
import java.util.Locale

private fun f(format: String, vararg a: Any) = String.format(Locale.US, format, *a)

private fun pct(x: Double) = if (x.isNaN()) "-" else f("%.0f%%", x * 100)

private fun mean(a: DoubleArray): Double = a.filter { !it.isNaN() }.let { if (it.isEmpty()) Double.NaN else it.average() }

/**
 * One page that collates everything: the headline numbers of the fused best-estimate track, how
 * each source did, how closely the two receivers agree, and how sure the fused track is.
 */
@Composable
fun SummaryTab(
    samples: Samples,
    ellipse: EllipseView?,
    fused: FusedTrack,
    laps: LapResult?,
    line: GeoLine?,
    session: Session,
    can: CanData?,
    settings: Settings,
    cursor: Int,
    onCursor: (Int) -> Unit,
    onOpenLaps: () -> Unit,
) {
    val v = settings.speedUnit.label
    val d = settings.speedUnit.distanceLabel
    val onboardGap = remember(samples, fused) { trackGap(samples.lat, samples.lon, fused.lat, fused.lon) }
    val ellipseGap = remember(samples, fused, ellipse) { ellipse?.let { trackGap(it.lat, it.lon, fused.lat, fused.lon) } }
    val between = remember(samples, ellipse) { ellipse?.let { trackGap(samples.lat, samples.lon, it.lat, it.lon) } }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Heading("Best estimate")
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Tile("Distance", f("%.2f %s", settings.distance(trackLengthMetres(fused.lat, fused.lon) / 1000), d), Modifier.weight(1f))
            Tile("Top speed", f("%.0f %s", settings.speed(fused.speedKmh.filter { !it.isNaN() }.maxOrNull() ?: 0.0), v), Modifier.weight(1f))
            Tile("Time", hmsText(samples.durationSeconds), Modifier.weight(1f))
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val best = laps?.bestIndex?.takeIf { it >= 0 }?.let { laps.laps[it] }
            Tile("Laps", if (laps == null) "no line" else "${laps.laps.size}", Modifier.weight(1f))
            Tile("Best lap", best?.let { lapTimeText(it.timeS) } ?: "-", Modifier.weight(1f))
            // what can be measured: how far apart the two receivers put the car; the filter's own spread is too optimistic to headline
            if (between != null) Tile("Receivers differ", f("%.1f m", percentile(between, 0.5)), Modifier.weight(1f))
            else Tile("With a fix", pct(presence(samples.lat)), Modifier.weight(1f))
        }
        if (line == null || laps == null || laps.laps.isEmpty()) {
            Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Text(
                    "Set a start/finish line to get laps.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f),
                )
                OutlinedButton(onClick = onOpenLaps) { Text("Laps") }
            }
        }
        Note(
            "The fused track combines the board's receiver and the Ellipse, with their Doppler speeds. " +
                "It is a best guess, not a measurement of the truth: both receivers see the same satellites, so shared errors stay in it.",
        )

        Heading("Sources")
        Card(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                SourceRow(
                    "Onboard GNSS (LC29H)",
                    f(
                        "%s with a fix · %.0f satellites · HDOP %.2f · %s RTK",
                        pct(presence(samples.lat)), percentile(samples["num_sv"], 0.5), percentile(samples["hdop"], 0.5),
                        pct(samples["fix_quality"].filter { !it.isNaN() }.let { q -> if (q.isEmpty()) 0.0 else q.count { it >= 4.0 }.toDouble() / q.size }),
                    ),
                )
                SourceRow("Dual-antenna heading", f("%s of samples valid · baseline %.2f m", pct(mean(samples["heading_valid"])), percentile(samples["baseline_m"], 0.5)))
                SourceRow("Onboard IMU (ASM330)", f("%s of samples valid", pct(mean(samples["imu_valid"]))))
                if (ellipse != null) {
                    val nav = ellipse.solution.filter { !it.isNaN() }.let { s -> if (s.isEmpty()) Double.NaN else s.count { (it.toLong() and 0xF) == 4L }.toDouble() / s.size }
                    SourceRow("SBG Ellipse", f("position for %s of samples · navigating %s of the time", pct(presence(ellipse.lat)), pct(nav)))
                } else {
                    SourceRow("SBG Ellipse", "not in this session. Enable its CAN messages with SBG's tools, or the car's CAN2 was not connected.")
                }
                SourceRow(
                    "CAN",
                    f("CAN1 alive %s · CAN2 alive %s", pct(mean(samples["can1_alive"])), pct(mean(samples["can2_alive"]))) +
                        (can?.let { " · ${it.ids.size} ids" } ?: ""),
                )
                SourceRow(
                    "Log",
                    (if (session.cleanClose) "closed cleanly" else "not closed cleanly (power loss?)") + if (session.damaged) " · has gaps" else "",
                )
            }
        }

        Heading("Do the sources agree?")
        if (ellipse == null || between == null) {
            Note("Only the onboard receiver is in this session, so there is nothing to compare. The fused track is the onboard track smoothed with its own Doppler speed.")
        } else {
            Card(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    SourceRow("Position", f("the two receivers are a median %.1f m apart, 95%% of the time within %.1f m", percentile(between, 0.5), percentile(between, 0.95)))
                    SourceRow(
                        "Speed",
                        f(
                            "differ by %.2f %s rms",
                            settings.speed(rms(DoubleArray(samples.size) { samples.speedKmh[it] - ellipse.speedKmh[it] })), v,
                        ),
                    )
                    SourceRow(
                        "Heading",
                        f(
                            "Ellipse yaw and dual-antenna heading differ by %.1f° rms, which includes any mounting offset",
                            rms(ellipse.angleGap(ellipse.yawDeg, samples["heading_deg"])),
                        ),
                    )
                    SourceRow(
                        "Fused track",
                        f(
                            "a median %.1f m from the onboard position and %.1f m from the Ellipse's",
                            percentile(onboardGap, 0.5), percentile(ellipseGap ?: DoubleArray(0), 0.5),
                        ),
                    )
                }
            }
            Note("Neither receiver is taken as the truth. Agreement tells you the two are consistent, not that either is right, because both see the same satellites.")
        }

        Heading("How sure is the fused track?")
        LineChart("Filter spread (a lower bound)", "m", samples.t, listOf(Series("σ", fused.sigmaM, SeriesAlt)), cursor, onCursor, digits = 2)
        Note("The filter assumes each receiver's error is independent. Real errors are partly shared, so the real uncertainty is larger than this line, roughly the size of the gap between the receivers.")
        LineChart(
            "Distance from the fused track to each receiver", "m", samples.t,
            buildList {
                add(Series("onboard", onboardGap, SeriesZ))
                if (ellipseGap != null) add(Series("Ellipse", ellipseGap, SeriesX, dashed = true))
            },
            cursor, onCursor, digits = 2,
        )
        if (!laps?.laps.isNullOrEmpty()) {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                OutlinedButton(onClick = onOpenLaps) { Text("Open the lap table") }
            }
        }
    }
}

private fun hmsText(seconds: Double): String {
    val s = seconds.toInt()
    return if (s >= 3600) f("%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) else f("%d:%02d", s / 60, s % 60)
}

@Composable
private fun Heading(text: String) {
    Text(text, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 6.dp))
}

@Composable
private fun Note(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
}

@Composable
private fun Tile(label: String, value: String, modifier: Modifier) {
    Card(modifier) {
        Column(Modifier.padding(12.dp)) {
            Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, style = MaterialTheme.typography.titleMedium)
        }
    }
}

@Composable
private fun SourceRow(title: String, detail: String) {
    Column {
        Text(title, style = MaterialTheme.typography.labelLarge)
        Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}
