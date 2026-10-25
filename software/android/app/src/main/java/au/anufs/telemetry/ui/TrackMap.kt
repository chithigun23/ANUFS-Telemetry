package au.anufs.telemetry.ui

import android.graphics.Color as AColor
import android.graphics.Paint
import android.graphics.drawable.ShapeDrawable
import android.graphics.drawable.shapes.OvalShape
import android.graphics.Canvas
import android.graphics.Point
import android.view.InputDevice
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.material3.Card
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Switch
import androidx.compose.material3.TextButton
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import au.anufs.telemetry.core.GeoLine
import au.anufs.telemetry.core.haversine
import org.osmdroid.views.overlay.Overlay
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.foundation.layout.padding
import androidx.compose.ui.unit.dp
import android.view.MotionEvent
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import au.anufs.telemetry.core.EllipseView
import au.anufs.telemetry.core.Samples
import android.util.Log
import au.anufs.telemetry.BuildConfig
import org.osmdroid.config.Configuration
import org.osmdroid.events.MapListener
import org.osmdroid.events.ScrollEvent
import org.osmdroid.events.ZoomEvent
import org.osmdroid.tileprovider.tilesource.TileSourceFactory
import org.osmdroid.util.BoundingBox
import org.osmdroid.util.GeoPoint
import org.osmdroid.views.CustomZoomButtonsController
import org.osmdroid.views.MapView
import org.osmdroid.views.overlay.Marker
import org.osmdroid.views.overlay.Polyline
import java.io.File
import kotlin.math.max

private const val SPEED_BINS = 24

/** Okabe-Ito reddish purple: the Ellipse's track and marker. */
private val ELLIPSE_COLOR = AColor.rgb(0xCC, 0x79, 0xA7)

/**
 * What the map needs to remember between compositions: the box to fit, and whether the user has
 * touched the map. Until they do, the view re-fits whenever its size or its tracks change, so the
 * track stays framed while the screen settles (the Source chips appear once CAN data has loaded).
 * After the first touch the view is theirs; the Fit button gives the framing back.
 */
private class FitState {
    var box: BoundingBox? = null
    var touched = false
    var fit: (() -> Unit)? = null
}

/**
 * The part of the map view the user has chosen, kept by the session screen so that switching to
 * another tab and back does not throw their zoom away. Empty until they touch the map.
 */
class MapCamera {
    var lat = 0.0
    var lon = 0.0
    var zoom = 0.0
    var userChosen = false
}

/** What the map's start/finish menu does, and what it draws. */
class LineControls(
    val line: GeoLine?,
    /** The user is drawing a line now. */
    val drawing: Boolean,
    val onDrawn: (GeoLine) -> Unit,
    val onStartDraw: () -> Unit,
    val onCancelDraw: () -> Unit,
    val onSetAtCursor: () -> Unit,
    val onClear: () -> Unit,
)

/**
 * Takes the touches that draw a start/finish line while drawing is on. The S Pen draws; fingers
 * keep panning and zooming unless [fingerToo] is set (for phones without a pen). A second finger
 * down means a pinch, so it is never taken.
 */
private class DrawOverlay : Overlay() {
    var active = false
    var fingerToo = false
    var onLine: (GeoLine) -> Unit = {}
    private var a: GeoPoint? = null
    private var b: GeoPoint? = null
    private val paint = Paint().apply {
        color = AColor.rgb(0xD5, 0x5E, 0x00)
        strokeWidth = 10f
        strokeCap = Paint.Cap.ROUND
        isAntiAlias = true
    }

    override fun onTouchEvent(e: MotionEvent, map: MapView): Boolean {
        if (!active) return false
        val tool = e.getToolType(0)
        val pen = tool == MotionEvent.TOOL_TYPE_STYLUS || tool == MotionEvent.TOOL_TYPE_ERASER
        if (!pen && !fingerToo) return false
        if (e.pointerCount > 1) return false
        val p = map.projection.fromPixels(e.x.toInt(), e.y.toInt()) as GeoPoint
        when (e.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                a = p
                b = p
            }
            MotionEvent.ACTION_MOVE -> b = p
            MotionEvent.ACTION_UP -> {
                b = p
                val s = a
                val t = b
                a = null
                b = null
                // a tap or a scratch is not a line: at least 5 m
                if (s != null && t != null && haversine(s.latitude, s.longitude, t.latitude, t.longitude) >= 5.0) {
                    onLine(GeoLine(s.latitude, s.longitude, t.latitude, t.longitude, 0))
                }
            }
            MotionEvent.ACTION_CANCEL -> {
                a = null
                b = null
            }
        }
        map.invalidate()
        return true
    }

    override fun draw(c: Canvas, map: MapView, shadow: Boolean) {
        if (shadow) return
        val s = a ?: return
        val t = b ?: return
        val ps = map.projection.toPixels(s, Point())
        val pt = map.projection.toPixels(t, Point())
        c.drawLine(ps.x.toFloat(), ps.y.toFloat(), pt.x.toFloat(), pt.y.toFloat(), paint)
    }
}

private fun deviceHasStylus(): Boolean =
    InputDevice.getDeviceIds().any { InputDevice.getDevice(it)?.supportsSource(InputDevice.SOURCE_STYLUS) == true }

/** Blue (slow) to red (fast). */
private fun speedColor(bin: Int): Int {
    val hue = 240f * (1f - bin.toFloat() / (SPEED_BINS - 1))
    return AColor.HSVToColor(floatArrayOf(hue, 0.85f, 0.9f))
}

/**
 * OpenStreetMap view of the track, coloured by speed, with a marker at the cursor sample.
 * Tiles are downloaded on first view and kept in the app cache.
 */
@Composable
fun TrackMap(
    samples: Samples,
    ellipse: EllipseView?,
    source: Source,
    camera: MapCamera,
    lineControls: LineControls,
    cursor: Int,
    modifier: Modifier = Modifier,
) {
    val context = LocalContext.current
    val lifecycle = LocalLifecycleOwner.current.lifecycle

    val fitState = remember(samples) { FitState().also { it.touched = camera.userChosen } }
    val holder = remember { object { var current = fitState } }
    holder.current = fitState
    val mapView = remember {
        Configuration.getInstance().apply {
            userAgentValue = context.packageName
            osmdroidBasePath = File(context.cacheDir, "osmdroid")
            osmdroidTileCache = File(context.cacheDir, "osmdroid/tiles")
        }
        MapView(context).apply {
            setTileSource(TileSourceFactory.MAPNIK)
            setMultiTouchControls(true)
            zoomController.setVisibility(CustomZoomButtonsController.Visibility.NEVER)
            minZoomLevel = 3.0
            addOnLayoutChangeListener { _, l, t, r, b, ol, ot, or2, ob ->
                if (r - l != or2 - ol || b - t != ob - ot) holder.current.let { if (!it.touched) it.fit?.invoke() }
            }
            setOnTouchListener { _, ev ->
                if (ev.actionMasked == MotionEvent.ACTION_DOWN) holder.current.touched = true
                false
            }
            if (BuildConfig.DEBUG) {
                addMapListener(object : MapListener {
                    override fun onScroll(event: ScrollEvent?): Boolean {
                        Log.d("TrackMap", "scroll centre=${mapCenter.latitude},${mapCenter.longitude} zoom=$zoomLevelDouble")
                        return false
                    }

                    override fun onZoom(event: ZoomEvent?): Boolean {
                        Log.d("TrackMap", "zoom ${event?.zoomLevel} (view ${width}x$height)")
                        return false
                    }
                })
            }
        }
    }
    DisposableEffect(lifecycle, fitState) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_RESUME -> mapView.onResume()
                Lifecycle.Event.ON_PAUSE -> mapView.onPause()
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            if (fitState.touched) {
                camera.lat = mapView.mapCenter.latitude
                camera.lon = mapView.mapCenter.longitude
                camera.zoom = mapView.zoomLevelDouble
                camera.userChosen = true
            }
            mapView.onDetach()
        }
    }

    fun dot(fill: Int) = Marker(mapView).apply {
        icon = ShapeDrawable(OvalShape()).apply {
            intrinsicWidth = 36
            intrinsicHeight = 36
            paint.color = fill
            paint.style = Paint.Style.FILL
        }
        setAnchor(Marker.ANCHOR_CENTER, Marker.ANCHOR_CENTER)
        setInfoWindow(null)
    }
    val onboardMarker = remember(samples) { dot(AColor.WHITE) }
    val ellipseMarker = remember(samples, ellipse) { dot(ELLIPSE_COLOR) }
    val drawOverlay = remember { DrawOverlay() }
    val fingerDefault = remember { !deviceHasStylus() }
    var fingerDraws by remember { mutableStateOf(fingerDefault) }
    drawOverlay.active = lineControls.drawing
    drawOverlay.fingerToo = fingerDraws
    drawOverlay.onLine = lineControls.onDrawn
    val showOnboard = source.showsOnboard
    val showEllipse = source.showsEllipse && ellipse != null

    LaunchedEffect(samples, ellipse, source) {
        mapView.overlays.clear()
        val all = ArrayList<GeoPoint>()

        /** Speed-coloured track, broken wherever a position is missing. */
        fun addTrack(lat: DoubleArray, lon: DoubleArray, speed: DoubleArray, width: Float) {
            val maxSpeed = max(1.0, speed.filter { !it.isNaN() }.maxOrNull() ?: 1.0)
            var current: Polyline? = null
            var currentBin = -1
            var last: GeoPoint? = null
            for (i in 0 until samples.size) {
                if (lat[i].isNaN() || lon[i].isNaN()) {
                    current = null
                    last = null
                    continue
                }
                val p = GeoPoint(lat[i], lon[i])
                all.add(p)
                val sp = speed[i]
                val bin = if (sp.isNaN()) 0 else ((sp / maxSpeed) * (SPEED_BINS - 1)).toInt().coerceIn(0, SPEED_BINS - 1)
                if (current == null || bin != currentBin) {
                    current = Polyline(mapView).apply {
                        outlinePaint.color = speedColor(bin)
                        outlinePaint.strokeWidth = width
                        outlinePaint.strokeCap = Paint.Cap.ROUND
                        last?.let { addPoint(it) } // join to the previous colour with no gap
                    }
                    mapView.overlays.add(current)
                    currentBin = bin
                }
                current.addPoint(p)
                last = p
            }
        }

        /** A thin single-colour line for the second source. */
        fun addLine(lat: DoubleArray, lon: DoubleArray, color: Int, width: Float) {
            var current: Polyline? = null
            for (i in 0 until samples.size) {
                if (lat[i].isNaN() || lon[i].isNaN()) {
                    current = null
                    continue
                }
                val p = GeoPoint(lat[i], lon[i])
                all.add(p)
                if (current == null) {
                    current = Polyline(mapView).apply {
                        outlinePaint.color = color
                        outlinePaint.strokeWidth = width
                        outlinePaint.strokeCap = Paint.Cap.ROUND
                    }
                    mapView.overlays.add(current)
                }
                current.addPoint(p)
            }
        }

        val onboardLat = DoubleArray(samples.size) { if (samples.hasFix(it)) samples.lat[it] else Double.NaN }
        val onboardLon = DoubleArray(samples.size) { if (samples.hasFix(it)) samples.lon[it] else Double.NaN }
        when {
            showOnboard && showEllipse -> {
                addTrack(onboardLat, onboardLon, samples.speedKmh, 9f)
                addLine(ellipse!!.lat, ellipse.lon, ELLIPSE_COLOR, 4f)
            }
            showEllipse -> addTrack(ellipse!!.lat, ellipse.lon, ellipse.speedKmh, 9f)
            else -> addTrack(onboardLat, onboardLon, samples.speedKmh, 9f)
        }
        if (showOnboard) mapView.overlays.add(onboardMarker)
        if (showEllipse) mapView.overlays.add(ellipseMarker)
        mapView.overlays.add(drawOverlay)
        if (all.isNotEmpty()) {
            var box = BoundingBox.fromGeoPoints(all)
            val minSpan = 0.0015 // about 150 m, so a very short track does not zoom to the tile limit
            if (box.latNorth - box.latSouth < minSpan || box.lonEast - box.lonWest < minSpan) {
                val c = box.centerWithDateLine
                box = BoundingBox(c.latitude + minSpan / 2, c.longitude + minSpan / 2, c.latitude - minSpan / 2, c.longitude - minSpan / 2)
            }
            fitState.box = box
            fitState.fit = {
                if (mapView.width > 0 && mapView.height > 0) {
                    mapView.zoomToBoundingBox(box.increaseByScale(1.3f), false)
                    if (BuildConfig.DEBUG) Log.d("TrackMap", "fit: zoom=${mapView.zoomLevelDouble} view ${mapView.width}x${mapView.height}")
                }
            }
            if (camera.userChosen) {
                // back from another tab: keep the zoom and position the user had
                val restore = {
                    mapView.controller.setZoom(camera.zoom)
                    mapView.controller.setCenter(GeoPoint(camera.lat, camera.lon))
                    if (BuildConfig.DEBUG) Log.d("TrackMap", "restored camera zoom=${camera.zoom}")
                }
                if (mapView.width > 0) restore() else mapView.addOnFirstLayoutListener { _, _, _, _, _ -> restore() }
            } else if (!fitState.touched) {
                fitState.fit?.invoke()
            }
        }
        mapView.invalidate()
    }

    // The start/finish line: a black casing with a white dashed core, over the tracks and under the markers.
    val lineOverlays = remember { ArrayList<Overlay>() }
    LaunchedEffect(lineControls.line, samples, ellipse, source) {
        mapView.overlays.removeAll(lineOverlays.toSet())
        lineOverlays.clear()
        lineControls.line?.let { l ->
            val casing = Polyline(mapView).apply {
                addPoint(GeoPoint(l.lat1, l.lon1))
                addPoint(GeoPoint(l.lat2, l.lon2))
                outlinePaint.color = AColor.BLACK
                outlinePaint.strokeWidth = 16f
            }
            val core = Polyline(mapView).apply {
                addPoint(GeoPoint(l.lat1, l.lon1))
                addPoint(GeoPoint(l.lat2, l.lon2))
                outlinePaint.color = AColor.WHITE
                outlinePaint.strokeWidth = 8f
                outlinePaint.pathEffect = android.graphics.DashPathEffect(floatArrayOf(14f, 14f), 0f)
            }
            lineOverlays += casing
            lineOverlays += core
        }
        mapView.overlays.addAll(lineOverlays)
        // markers and the pen layer go back on top
        for (o in listOf<Overlay>(onboardMarker, ellipseMarker, drawOverlay)) mapView.overlays.remove(o)
        if (showOnboard) mapView.overlays.add(onboardMarker)
        if (showEllipse) mapView.overlays.add(ellipseMarker)
        mapView.overlays.add(drawOverlay)
        mapView.invalidate()
    }

    LaunchedEffect(samples, ellipse, cursor) {
        val i = cursor.coerceIn(0, max(0, samples.size - 1))
        if (samples.size > 0 && samples.hasFix(i)) {
            onboardMarker.position = GeoPoint(samples.lat[i], samples.lon[i])
            onboardMarker.alpha = 1f
        } else {
            onboardMarker.alpha = 0f
        }
        if (ellipse != null && samples.size > 0 && ellipse.hasFix(i)) {
            ellipseMarker.position = GeoPoint(ellipse.lat[i], ellipse.lon[i])
            ellipseMarker.alpha = 1f
        } else {
            ellipseMarker.alpha = 0f
        }
        mapView.invalidate()
    }

    // osmdroid draws past its own bounds; clip it so it cannot cover the tabs above or the chart below.
    Box(modifier.clipToBounds()) {
        AndroidView(factory = { mapView }, modifier = Modifier.matchParentSize())
        var menu by remember { mutableStateOf(false) }
        Column(Modifier.align(Alignment.TopEnd).padding(8.dp), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            FilledTonalButton(onClick = { fitState.touched = false; fitState.fit?.invoke() }) { Text("Fit track") }
            Box {
                FilledTonalButton(onClick = { menu = true }) { Text(if (lineControls.line == null) "Start/finish" else "Start/finish ✓") }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(text = { Text("Draw with S Pen") }, onClick = { menu = false; lineControls.onStartDraw() })
                    DropdownMenuItem(text = { Text("Set at the cursor") }, onClick = { menu = false; lineControls.onSetAtCursor() })
                    if (lineControls.line != null) {
                        DropdownMenuItem(text = { Text("Remove") }, onClick = { menu = false; lineControls.onClear() })
                    }
                }
            }
        }
        if (lineControls.drawing) {
            Card(Modifier.align(Alignment.TopStart).padding(8.dp)) {
                Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp)) {
                    Text("Draw the start/finish line across the track")
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(if (fingerDraws) "Finger draws" else "S Pen draws, finger pans", style = androidx.compose.material3.MaterialTheme.typography.labelMedium)
                        Switch(checked = fingerDraws, onCheckedChange = { fingerDraws = it })
                    }
                    TextButton(onClick = lineControls.onCancelDraw) { Text("Cancel") }
                }
            }
        }
    }
}
