package au.anufs.telemetry

import android.app.Instrumentation
import android.content.Intent
import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Runs on the phone against the demo card (three laps of a made-up circuit): draws a start/finish
 * line with a real stylus event stream, and sets one at the cursor.
 */
class LapsOnDeviceTest {
    private val inst: Instrumentation = InstrumentationRegistry.getInstrumentation()
    private val device = UiDevice.getInstance(inst)

    private fun launch() {
        val ctx = inst.targetContext
        ctx.startActivity(
            Intent(ctx, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
                .putExtra("reset", true) // nothing saved: no remembered start/finish line, default settings
                .putExtra("open_path", ctx.filesDir.resolve("demo_laps.img").path)
                .putExtra("open_session", 1),
        )
        assertNotNull("the session should open", device.wait(Until.findObject(By.text("Session 1")), 15000))
        SystemClock.sleep(3000)
        device.findObject(By.text("Map")).click() // the session opens on the Summary tab
        SystemClock.sleep(1500)
    }

    /** One finger or pen drag from (x1, y1) to (x2, y2). */
    private fun drag(tool: Int, x1: Float, y1: Float, x2: Float, y2: Float, steps: Int = 20) {
        val t0 = SystemClock.uptimeMillis()
        fun ev(action: Int, x: Float, y: Float, t: Long): MotionEvent {
            val props = arrayOf(MotionEvent.PointerProperties().apply { id = 0; toolType = tool })
            val coords = arrayOf(MotionEvent.PointerCoords().apply { this.x = x; this.y = y; pressure = 1f; size = 1f })
            val source = if (tool == MotionEvent.TOOL_TYPE_STYLUS) InputDevice.SOURCE_STYLUS or InputDevice.SOURCE_TOUCHSCREEN else InputDevice.SOURCE_TOUCHSCREEN
            return MotionEvent.obtain(t0, t, action, 1, props, coords, 0, 0, 1f, 1f, 0, 0, source, 0)
        }
        val ui = inst.uiAutomation
        var t = t0
        ui.injectInputEvent(ev(MotionEvent.ACTION_DOWN, x1, y1, t), true)
        for (i in 1..steps) {
            t += 16
            ui.injectInputEvent(ev(MotionEvent.ACTION_MOVE, x1 + (x2 - x1) * i / steps, y1 + (y2 - y1) * i / steps, t), true)
            SystemClock.sleep(12)
        }
        t += 16
        ui.injectInputEvent(ev(MotionEvent.ACTION_UP, x2, y2, t), true)
    }

    private fun snack(prefix: String): String {
        val o = device.wait(Until.findObject(By.textStartsWith(prefix)), 6000)
        assertNotNull("expected a message starting \"$prefix\"", o)
        return o.text
    }

    @Test
    fun aLineDrawnWithTheStylusFindsTheLaps() {
        launch()
        device.findObject(By.text("Start/finish")).click()
        SystemClock.sleep(500)
        device.findObject(By.text("Draw with S Pen")).click()
        assertNotNull("draw banner", device.wait(Until.findObject(By.textStartsWith("Draw the start/finish line")), 3000))
        // across the bottom straight of the fitted circuit (see the map screenshot), square to it
        val x = device.displayWidth * 0.48f
        val y = device.displayHeight * 0.64f
        drag(MotionEvent.TOOL_TYPE_STYLUS, x, y - 110f, x, y + 110f)
        val msg = snack("Start/finish set")
        println("LAPS pen: $msg")
        assertTrue(msg, msg.contains("2 laps"))
        device.findObject(By.text("Laps")).click()
        assertNotNull("lap table", device.wait(Until.findObject(By.textContains("best")), 4000))
    }

    @Test
    fun aFingerDoesNotDrawWhileThePenIsTheOnlyDrawer() {
        launch()
        device.findObject(By.text("Start/finish")).click()
        SystemClock.sleep(500)
        device.findObject(By.text("Draw with S Pen")).click()
        SystemClock.sleep(500)
        val x = device.displayWidth * 0.48f
        val y = device.displayHeight * 0.64f
        drag(MotionEvent.TOOL_TYPE_FINGER, x, y - 110f, x, y + 110f)
        SystemClock.sleep(1500)
        assertEquals("a finger drag pans the map, it does not set a line", null, device.findObject(By.textStartsWith("Start/finish set")))
        assertNotNull("still waiting for the pen", device.findObject(By.textStartsWith("Draw the start/finish line")))
    }

    @Test
    fun aLineSetAtTheCursorFindsTheLaps() {
        launch()
        // put the cursor a quarter of the way along the speed chart, then set the line there
        device.click((device.displayWidth * 0.4f).toInt(), (device.displayHeight * 0.80f).toInt())
        SystemClock.sleep(500)
        device.findObject(By.text("Start/finish")).click()
        SystemClock.sleep(500)
        device.findObject(By.text("Set at the cursor")).click()
        val msg = snack("Start/finish set")
        println("LAPS cursor: $msg")
        assertTrue(msg, msg.contains("laps") || msg.contains("lap"))
        device.findObject(By.text("Laps")).click()
        assertNotNull("lap table", device.wait(Until.findObject(By.textContains("best")), 4000))
    }
}
