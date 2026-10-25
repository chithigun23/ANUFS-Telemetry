package au.anufs.telemetry.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Tab
import androidx.compose.material3.PrimaryScrollableTabRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import au.anufs.telemetry.core.EllipseView
import au.anufs.telemetry.core.DefaultSource
import au.anufs.telemetry.core.FusedTrack
import au.anufs.telemetry.core.GeoLine
import au.anufs.telemetry.core.fuseSession
import au.anufs.telemetry.core.bestSavedLine
import au.anufs.telemetry.core.detectLaps
import au.anufs.telemetry.core.lineAtSample
import androidx.compose.runtime.CompositionLocalProvider
import au.anufs.telemetry.core.Samples
import au.anufs.telemetry.core.Settings
import au.anufs.telemetry.core.Session
import java.text.DateFormat
import java.util.Date
import java.util.Locale

private fun hms(seconds: Double): String {
    val s = seconds.toInt()
    return if (s >= 3600) String.format(Locale.US, "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60)
    else String.format(Locale.US, "%d:%02d", s / 60, s % 60)
}

/** Home: pick a card image, list its sessions. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(vm: AppViewModel, onPickImage: () -> Unit, onSettings: () -> Unit) {
    val snack = remember { SnackbarHostState() }
    LaunchedEffect(vm.message) {
        vm.message?.let {
            snack.showSnackbar(it)
            vm.dismissMessage()
        }
    }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("ANUFS Telemetry") },
                actions = { TextButton(onClick = onSettings) { Text("Settings") } },
            )
        },
        snackbarHost = { SnackbarHost(snack) },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Button(onClick = onPickImage, enabled = !vm.busy) { Text("Open card image…") }
            if (vm.busy) CircularProgressIndicator()
            val card = vm.card
            if (card == null) {
                Text(
                    "Pick a raw image of a log card (made with dd, or the card itself once the USB reader is wired in).",
                    style = MaterialTheme.typography.bodyMedium,
                )
            } else {
                Text(
                    "${vm.sourceName}  ·  ${card.validBlocks} blocks read" +
                        if (card.invalidBlocks > 0) ", ${card.invalidBlocks} damaged" else "",
                    style = MaterialTheme.typography.bodyMedium,
                )
                val sessions = card.sessions
                if (sessions.isEmpty()) Text("No sessions on this card.")
                Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    sessions.forEach { SessionCard(it, onClick = { vm.openSession(it) }) }
                }
            }
        }
    }
}

@Composable
private fun SessionCard(s: Session, onClick: () -> Unit) {
    Card(Modifier.fillMaxWidth().clickable(onClick = onClick)) {
        Column(Modifier.padding(12.dp)) {
            Text("Session ${s.number}", style = MaterialTheme.typography.titleMedium)
            val whenText = s.utcUnixMs?.let { DateFormat.getDateTimeInstance().format(Date(it)) } ?: "start time not recorded"
            Text(whenText, style = MaterialTheme.typography.bodyMedium)
            val kib = s.blocks / 2
            Text(
                "$kib KiB" + (if (s.cleanClose) "" else "  ·  not closed cleanly (power loss?)") +
                    (if (s.damaged) "  ·  has gaps" else "") +
                    (s.firmware?.let { "  ·  fw $it" } ?: ""),
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

/** One session: map, IMU, GNSS and CAN tabs sharing one cursor and one choice of source. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionScreen(
    vm: AppViewModel,
    open: OpenSession,
    onBack: () -> Unit,
    onSettings: () -> Unit,
    onExport: (ExportKind) -> Unit,
) {
    val settings = LocalSettings.current
    val snack = remember { SnackbarHostState() }
    LaunchedEffect(vm.message) {
        vm.message?.let {
            snack.showSnackbar(it)
            vm.dismissMessage()
        }
    }
    val samples = open.samples
    LaunchedEffect(open) { vm.loadCan() }
    val can = vm.can
    val ellipse = remember(samples, can) { can?.let { EllipseView(samples, it.signals) }?.takeIf { it.present } }
    var tab by rememberSaveable { mutableIntStateOf(0) } // 0 Summary, 1 Map, 2 Laps, 3 IMU, 4 GNSS, 5 CAN
    var cursor by remember(samples) { mutableIntStateOf(0) }
    val mapCamera = remember(samples) { MapCamera() }
    val fused = remember(samples, ellipse, settings.weighting, settings.useImu) { fuseSession(samples, ellipse, settings.fusionTuning) }
    val initialSource = when (settings.defaultSource) {
        DefaultSource.Fused -> Source.Fused
        DefaultSource.Onboard -> Source.Onboard
        DefaultSource.Ellipse -> Source.Ellipse
        DefaultSource.Both -> Source.Both
        DefaultSource.LastUsed -> Source.entries.firstOrNull { it.name == settings.lastSource } ?: Source.Fused
    }
    var picked by rememberSaveable { mutableStateOf(initialSource) }
    // Without Ellipse data there is only the board's receiver and what can be made of it alone.
    val source = if (ellipse == null && (picked == Source.Ellipse || picked == Source.Both)) Source.Fused else picked

    // Laps follow the chosen source: fused, Ellipse on its own, or the board's receiver (also for Both).
    val trackEllipse = ellipse?.takeIf { source == Source.Ellipse }
    val trackFused = fused.takeIf { source == Source.Fused }
    val trackLat = trackFused?.lat ?: trackEllipse?.lat ?: samples.lat
    val trackLon = trackFused?.lon ?: trackEllipse?.lon ?: samples.lon
    val trackKmh = trackFused?.speedKmh ?: trackEllipse?.speedKmh ?: samples.speedKmh
    val minLap = settings.minLapSeconds.toDouble()
    var chosenLine by remember(samples) { mutableStateOf<GeoLine?>(null) }
    var lineRemoved by remember(samples) { mutableStateOf(false) }
    var drawing by remember { mutableStateOf(false) }
    val savedLines = vm.savedLines
    val autoLine = remember(samples, source, fused, trackEllipse, savedLines, minLap) {
        bestSavedLine(samples.t, trackLat, trackLon, trackKmh, savedLines, minLap)
    }
    val line = chosenLine ?: if (lineRemoved) null else autoLine
    val laps = remember(samples, source, fused, trackEllipse, line, minLap) {
        line?.let { detectLaps(samples.t, trackLat, trackLon, trackKmh, it, minLap) }
    }
    fun setLine(l: GeoLine) {
        chosenLine = l
        lineRemoved = false
        drawing = false
        vm.saveLine(l)
        val found = detectLaps(samples.t, trackLat, trackLon, trackKmh, l, minLap).laps.size
        vm.notify(if (found == 0) "Start/finish set, but no complete lap crosses it" else "Start/finish set: $found ${if (found == 1) "lap" else "laps"}")
    }
    val lineControls = LineControls(
        line = line,
        drawing = drawing,
        onDrawn = { setLine(it) },
        onStartDraw = { drawing = true; tab = 1 },
        onCancelDraw = { drawing = false },
        onSetAtCursor = {
            val made = lineAtSample(trackLat, trackLon, cursor)
            if (made != null) setLine(made) else vm.notify("The car has no direction here (no fix, or standing still)")
        },
        onClear = {
            line?.let { vm.forgetLine(it) }
            chosenLine = null
            lineRemoved = true
        },
    )
    var menu by remember { mutableStateOf(false) }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Session ${open.session.number}") },
                navigationIcon = { TextButton(onClick = onBack) { Text("‹ Back") } },
                actions = {
                    TextButton(onClick = onSettings) { Text("Settings") }
                    Box {
                        TextButton(onClick = { menu = true }) { Text("Export") }
                        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                            DropdownMenuItem(
                                text = { Text("GPX track, fused (best estimate)") },
                                onClick = { menu = false; onExport(ExportKind.GPX_FUSED) },
                            )
                            DropdownMenuItem(
                                text = { Text("GPX track, onboard") },
                                onClick = { menu = false; onExport(ExportKind.GPX) },
                            )
                            if (ellipse != null) {
                                DropdownMenuItem(
                                    text = { Text("GPX track, Ellipse") },
                                    onClick = { menu = false; onExport(ExportKind.GPX_ELLIPSE) },
                                )
                            }
                            DropdownMenuItem(text = { Text("Samples CSV") }, onClick = { menu = false; onExport(ExportKind.CSV) })
                        }
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snack) },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize()) {
            StatsRow(samples, ellipse, fused, source, settings)
            if (tab != 0 && tab != 5) {
                SourceBar(source, hasEllipse = ellipse != null) { s ->
                    picked = s
                    vm.updateSettings { it.copy(lastSource = s.name) }
                }
            }
            PrimaryScrollableTabRow(selectedTabIndex = tab, edgePadding = 8.dp) {
                listOf("Summary", "Map", "Laps", "IMU", "GNSS", "CAN").forEachIndexed { i, name ->
                    Tab(selected = tab == i, onClick = { tab = i }, text = { Text(name) })
                }
            }
            if (samples.size == 0) {
                Text("This session has no samples.", Modifier.padding(16.dp))
            } else CompositionLocalProvider(LocalLapMarks provides (laps?.crossings ?: DoubleArray(0))) {
                when (tab) {
                    0 -> SummaryTab(
                        samples, ellipse, fused, laps, line, open.session, can, settings,
                        cursor = cursor, onCursor = { cursor = it }, onOpenLaps = { tab = 2 },
                    )
                    1 -> MapTab(samples, ellipse, fused, source, settings, mapCamera, lineControls, cursor) { cursor = it }
                    2 -> LapsTab(
                        samples, laps, line, settings,
                        onCursor = { cursor = it },
                        onDraw = lineControls.onStartDraw,
                        onSetAtCursor = lineControls.onSetAtCursor,
                        onClear = lineControls.onClear,
                    )
                    3 -> ImuTab(samples, ellipse, source, settings, cursor) { cursor = it }
                    4 -> GnssTab(samples, ellipse, fused, source, settings, cursor) { cursor = it }
                    else -> CanTab(samples, can, vm.canBusy, cursor) { cursor = it }
                }
            }
        }
    }
}

@Composable
private fun StatsRow(s: Samples, e: EllipseView?, fused: FusedTrack, source: Source, settings: Settings) {
    // One line about whichever source is shown; with both, each source's distance and top speed.
    val d = settings.speedUnit.distanceLabel
    val v = settings.speedUnit.label
    val onboardKm = settings.distance(s.distanceMetres / 1000)
    val onboardMax = settings.speed(s.maxSpeedKmh).let { if (it.isNaN()) 0.0 else it }
    val ellipseKm = e?.let { settings.distance(ellipseDistance(s, it) / 1000) } ?: 0.0
    val ellipseMax = e?.speedKmh?.filter { !it.isNaN() }?.maxOrNull()?.let { settings.speed(it) } ?: 0.0
    val fixes = (0 until s.size).count { s.hasFix(it) }
    val fusedKm = settings.distance(trackLength(s, fused.lat, fused.lon) / 1000)
    val fusedMax = fused.speedKmh.filter { !it.isNaN() }.maxOrNull()?.let { settings.speed(it) } ?: 0.0
    val text = when (source) {
        Source.Fused -> String.format(
            Locale.US, "%s  ·  %.2f %s  ·  max %.0f %s  ·  fused, %d%% of samples have a position",
            hms(s.durationSeconds), fusedKm, d, fusedMax, v, if (s.size == 0) 0 else fused.validCount * 100 / s.size,
        )
        Source.Ellipse -> String.format(
            Locale.US, "%s  ·  %.2f %s  ·  max %.0f %s  ·  Ellipse, %d samples",
            hms(s.durationSeconds), ellipseKm, d, ellipseMax, v, s.size,
        )
        Source.Both -> String.format(
            Locale.US, "%s  ·  %.2f %s onboard, %.2f %s Ellipse  ·  max %.0f / %.0f %s",
            hms(s.durationSeconds), onboardKm, d, ellipseKm, d, onboardMax, ellipseMax, v,
        )
        Source.Onboard -> String.format(
            Locale.US, "%s  ·  %.2f %s  ·  max %.0f %s  ·  %d samples (%d%% with a fix)",
            hms(s.durationSeconds), onboardKm, d, onboardMax, v, s.size,
            if (s.size == 0) 0 else fixes * 100 / s.size,
        )
    }
    Text(text, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
}

/** Length of the Ellipse's track in metres. */
private fun ellipseDistance(s: Samples, e: EllipseView): Double {
    var d = 0.0
    var prev = -1
    for (i in 0 until s.size) {
        if (!e.hasFix(i)) continue
        if (prev >= 0) d += au.anufs.telemetry.core.haversine(e.lat[prev], e.lon[prev], e.lat[i], e.lon[i])
        prev = i
    }
    return d
}

@Composable
private fun MapTab(s: Samples, e: EllipseView?, fused: FusedTrack, source: Source, settings: Settings, camera: MapCamera, controls: LineControls, cursor: Int, onCursor: (Int) -> Unit) {
    Column(Modifier.fillMaxSize()) {
        TrackMap(s, e, fused, source, camera, controls, cursor, Modifier.weight(1f).fillMaxWidth())
        if (source == Source.Both) Text(
            "White marker and coloured line: onboard · pink: Ellipse",
            style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 2.dp),
        )
        LineChart("Speed", settings.speedUnit.label, s.t, speedSeries(s, e, fused, source, settings), cursor, onCursor, digits = 1)
    }
}

/** Speed lines for the chosen source(s): onboard in blue, Ellipse in red, dashed when both are drawn. */
private fun speedSeries(s: Samples, e: EllipseView?, fused: FusedTrack, source: Source, settings: Settings): List<Series> = buildList {
    if (source == Source.Fused) add(Series("fused", settings.speed(fused.speedKmh), SeriesY))
    if (source.showsOnboard) add(Series("onboard", settings.speed(s.speedKmh), SeriesZ))
    if (source.showsEllipse && e != null) add(Series("Ellipse", settings.speed(e.speedKmh), SeriesX, dashed = source == Source.Both))
}

/** Three axis lines per source: solid for the onboard sensor, dashed for the Ellipse when both show. */
private fun axisSeries(onboard: Triple<DoubleArray, DoubleArray, DoubleArray>?, ellipse: Triple<DoubleArray, DoubleArray, DoubleArray>?): List<Series> = buildList {
    val dashed = onboard != null && ellipse != null
    onboard?.let { add(Series("X", it.first, SeriesX)); add(Series("Y", it.second, SeriesY)); add(Series("Z", it.third, SeriesZ)) }
    ellipse?.let { add(Series("Xe", it.first, SeriesX, dashed)); add(Series("Ye", it.second, SeriesY, dashed)); add(Series("Ze", it.third, SeriesZ, dashed)) }
}

@Composable
private fun ImuTab(s: Samples, e: EllipseView?, sourceIn: Source, settings: Settings, cursor: Int, onCursor: (Int) -> Unit) {
    // The two IMUs are not combined (their axes differ), so Fused shows both.
    val source = if (sourceIn == Source.Fused) (if (e != null) Source.Both else Source.Onboard) else sourceIn
    val a = settings::accel
    val gDigits = if (settings.accelUnit.label == "g") 3 else 2
    val ob = source.showsOnboard
    val el = source.showsEllipse && e != null
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        if (source == Source.Both) BothLegend()
        LineChart(
            "Acceleration", settings.accelUnit.label, s.t,
            axisSeries(
                if (ob) Triple(a(s["accel_mean_x_g"]), a(s["accel_mean_y_g"]), a(s["accel_mean_z_g"])) else null,
                if (el) Triple(a(e!!.accelXg), a(e.accelYg), a(e.accelZg)) else null,
            ),
            cursor, onCursor, digits = gDigits,
        )
        LineChart(
            "Angular rate", "°/s", s.t,
            axisSeries(
                if (ob) Triple(s["gyro_mean_x_dps"], s["gyro_mean_y_dps"], s["gyro_mean_z_dps"]) else null,
                if (el) Triple(e!!.gyroXdps, e.gyroYdps, e.gyroZdps) else null,
            ),
            cursor, onCursor, digits = 2,
        )
        if (ob) {
            LineChart(
                "Peak acceleration in each 100 ms window", settings.accelUnit.label, s.t,
                listOf(Series("X", a(s["accel_peak_x_g"]), SeriesX), Series("Y", a(s["accel_peak_y_g"]), SeriesY), Series("Z", a(s["accel_peak_z_g"]), SeriesZ)),
                cursor, onCursor, digits = gDigits,
            )
        }
        ScatterChart(
            "g-g diagram", 
            if (ob) s["accel_mean_x_g"] else e!!.accelXg, if (ob) s["accel_mean_y_g"] else e!!.accelYg,
            cursor, xLabel = "X", yLabel = "Y",
            xs2 = if (ob && el) e!!.accelXg else null, ys2 = if (ob && el) e!!.accelYg else null,
        )
        if (source == Source.Both) Text("Blue: onboard · orange: Ellipse", style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(horizontal = 16.dp))
        if (ob) LineChart("IMU temperature", "°C", s.t, listOf(Series("T", s["imu_temp_c"], SeriesAlt)), cursor, onCursor, digits = 1)
        if (!ob) Text(
            "Peak values and temperature are logged for the onboard IMU only.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(16.dp),
        )
    }
}

@Composable
private fun GnssTab(s: Samples, e: EllipseView?, fused: FusedTrack, source: Source, settings: Settings, cursor: Int, onCursor: (Int) -> Unit) {
    val isFused = source == Source.Fused
    val ob = source.showsOnboard
    val el = source.showsEllipse && e != null
    val dashed = (ob && el) || isFused
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        if (source == Source.Both) BothLegend()
        LineChart("Speed", settings.speedUnit.label, s.t, speedSeries(s, e, fused, source, settings), cursor, onCursor, digits = 1)
        LineChart(
            "Heading", "°", s.t,
            buildList {
                if (isFused) add(Series("fused course", fused.courseDeg, SeriesX))
                if (ob) {
                    add(Series("course", s["cog_deg"], SeriesX))
                }
                // the dual-antenna heading and the Ellipse's yaw are where the car points, not where it goes
                if (ob || isFused) add(Series("dual-antenna", s["heading_deg"], SeriesY, dashed = isFused))
                if (el || (isFused && e != null)) add(Series("Ellipse yaw", e!!.yawDeg, SeriesZ, dashed))
            },
            cursor, onCursor, digits = 1, wrapDegrees = true,
        )
        LineChart(
            "Altitude", "m", s.t,
            buildList {
                if (isFused) add(Series("fused", fused.altM, SeriesAlt))
                if (ob) add(Series("onboard", s["alt_m"], SeriesAlt))
                if (el) add(Series("Ellipse", e!!.altM, SeriesX, dashed))
            },
            cursor, onCursor, digits = 1,
        )
        if (isFused) {
            LineChart("Fused filter spread (a lower bound)", "m", s.t, listOf(Series("σ", fused.sigmaM, SeriesAlt)), cursor, onCursor, digits = 2)
        }
        if (ob || isFused) {
            LineChart("Satellites used", "", s.t, listOf(Series("sv", s["num_sv"], SeriesY)), cursor, onCursor, digits = 0)
            LineChart("HDOP", "", s.t, listOf(Series("hdop", s["hdop"], SeriesX)), cursor, onCursor, digits = 2)
            LineChart("Fix quality (GGA)", "", s.t, listOf(Series("q", s["fix_quality"], SeriesZ)), cursor, onCursor, digits = 0)
        } else {
            Text(
                "Satellite count, HDOP and fix quality are logged for the onboard receiver only. The Ellipse's own GNSS status is on the CAN tab if its GPS messages are enabled.",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(16.dp),
            )
        }
    }
}

/** Length of a track in metres, over the samples that have a position. */
private fun trackLength(s: Samples, lat: DoubleArray, lon: DoubleArray): Double {
    var d = 0.0
    var prev = -1
    for (i in 0 until s.size) {
        if (lat[i].isNaN() || lon[i].isNaN()) continue
        if (prev >= 0) d += au.anufs.telemetry.core.haversine(lat[prev], lon[prev], lat[i], lon[i])
        prev = i
    }
    return d
}
