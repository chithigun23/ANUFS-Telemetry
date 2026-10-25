package au.anufs.telemetry

import android.app.Instrumentation
import android.content.Intent
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Runs on the phone: opens the demo session, then drives the map with real two-finger gestures
 * (adb's `input` cannot) and reads the zoom level the debug build logs under `TrackMap`.
 *
 * Needs `files/demo_laps.img` in the app's data folder (see the bring-up notes).
 */
class MapPinchTest {
    private val inst: Instrumentation = InstrumentationRegistry.getInstrumentation()
    private val device = UiDevice.getInstance(inst)

    private fun zoomNow(): Double {
        val log = device.executeShellCommand("logcat -d -s TrackMap:D")
        val m = Regex("zoom=([0-9.]+)|zoom ([0-9.]+) \\(view").findAll(log).lastOrNull() ?: error("no zoom in log:\n$log")
        return (m.groupValues[1].ifEmpty { m.groupValues[2] }).toDouble()
    }

    private fun gesture(events: List<MotionEvent>) {
        for (e in events) {
            inst.uiAutomation.injectInputEvent(e, true)
            e.recycle()
            SystemClock.sleep(12)
        }
    }

    /** Two fingers on a horizontal line through (cx, cy), each [from] px from the centre, moving to [to] px. */
    private fun pinch(cx: Float, cy: Float, from: Float, to: Float, steps: Int = 25) {
        val t0 = SystemClock.uptimeMillis()
        fun ev(action: Int, half: Float, n: Int, t: Long): MotionEvent {
            val props = Array(n) { MotionEvent.PointerProperties().apply { id = it; toolType = MotionEvent.TOOL_TYPE_FINGER } }
            val coords = Array(n) { i ->
                MotionEvent.PointerCoords().apply {
                    x = if (i == 0) cx - half else cx + half
                    y = cy
                    pressure = 1f
                    size = 1f
                }
            }
            return MotionEvent.obtain(t0, t, action, n, props, coords, 0, 0, 1f, 1f, 0, 0, InputDevice.SOURCE_TOUCHSCREEN, 0)
        }
        val list = ArrayList<MotionEvent>()
        var t = t0
        list += ev(MotionEvent.ACTION_DOWN, from, 1, t).also { it.setLocation(cx - from, cy) }
        t += 20
        list += ev(MotionEvent.ACTION_POINTER_DOWN or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT), from, 2, t)
        for (i in 1..steps) {
            t += 16
            list += ev(MotionEvent.ACTION_MOVE, from + (to - from) * i / steps, 2, t)
        }
        t += 20
        list += ev(MotionEvent.ACTION_POINTER_UP or (1 shl MotionEvent.ACTION_POINTER_INDEX_SHIFT), to, 2, t)
        t += 20
        list += ev(MotionEvent.ACTION_UP, to, 1, t)
        gesture(list)
    }

    @Test
    fun pinchOutZoomsInAndPinchInZoomsOut() {
        val ctx = inst.targetContext
        device.executeShellCommand("logcat -c")
        ctx.startActivity(
            Intent(ctx, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
                .putExtra("open_path", ctx.filesDir.resolve("demo_laps.img").path)
                .putExtra("open_session", 1),
        )
        SystemClock.sleep(7000)
        val cx = device.displayWidth / 2f
        val cy = device.displayHeight * 0.48f
        val fitted = zoomNow()
        println("PINCH fitted zoom = $fitted")

        pinch(cx, cy, from = 120f, to = 420f)
        SystemClock.sleep(800)
        val zoomedIn = zoomNow()
        println("PINCH after pinch-out zoom = $zoomedIn")
        assertTrue("pinch-out should zoom in: $fitted -> $zoomedIn", zoomedIn > fitted + 0.3)

        pinch(cx, cy, from = 420f, to = 120f)
        SystemClock.sleep(800)
        val zoomedOut = zoomNow()
        println("PINCH after pinch-in zoom = $zoomedOut")
        assertTrue("pinch-in should zoom out: $zoomedIn -> $zoomedOut", zoomedOut < zoomedIn - 0.3)
    }

    @Test
    fun yourZoomSurvivesSwitchingTabsAndSources() {
        val ctx = inst.targetContext
        device.executeShellCommand("logcat -c")
        ctx.startActivity(
            Intent(ctx, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
                .putExtra("open_path", ctx.filesDir.resolve("demo_laps.img").path)
                .putExtra("open_session", 1),
        )
        SystemClock.sleep(7000)
        val cx = device.displayWidth / 2f
        val cy = device.displayHeight * 0.48f
        pinch(cx, cy, from = 120f, to = 400f)
        SystemClock.sleep(800)
        val chosen = zoomNow()
        println("PINCH chosen zoom = $chosen")

        // another source, same zoom
        device.findObject(By.text("Both")).click()
        SystemClock.sleep(1200)
        val afterSource = zoomNow()
        println("PINCH after Source=Both zoom = $afterSource")
        assertTrue("choosing a source must not re-fit: $chosen -> $afterSource", kotlin.math.abs(afterSource - chosen) < 0.05)

        // another tab and back
        device.findObject(By.text("IMU")).click()
        SystemClock.sleep(1000)
        device.findObject(By.text("Map")).click()
        SystemClock.sleep(2500)
        val afterTabs = zoomNow()
        println("PINCH after IMU and back zoom = $afterTabs")
        assertTrue("going to another tab and back must keep the zoom: $chosen -> $afterTabs", kotlin.math.abs(afterTabs - chosen) < 0.05)

        // Fit track gives the framing back
        device.findObject(By.text("Fit track")).click()
        SystemClock.sleep(800)
        val fitted = zoomNow()
        println("PINCH after Fit track zoom = $fitted")
        assertTrue("Fit track should zoom back out: $chosen -> $fitted", fitted < chosen - 0.5)
    }
}
