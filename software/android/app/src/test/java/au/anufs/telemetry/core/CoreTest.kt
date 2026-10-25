package au.anufs.telemetry.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

private fun resource(name: String): String =
    object {}.javaClass.classLoader!!.getResource(name)!!.readText()

class ScanParsingTest {
    @Test
    fun parsesACleanCard() {
        val scan = parseScan(resource("scan_ok.json"))
        assertEquals(3149594626L, scan.cardId)
        assertEquals(1, scan.sessions.size)
        val s = scan.sessions.single()
        assertEquals(1, s.number)
        assertEquals(150, s.blocks)
        assertTrue(s.cleanClose)
        assertFalse(s.damaged)
        assertNull(s.firmware)
        assertNull(s.utcUnixMs)
    }

    @Test
    fun runsOfOneSessionAreMergedAndFlaggedAsDamaged() {
        val scan = parseScan(resource("scan_two_runs.json"))
        val s = scan.sessions.single()
        assertEquals(2, s.number)
        assertEquals(30, s.blocks)
        assertFalse("one run was never closed", s.cleanClose)
        assertTrue(s.damaged)
        assertEquals("fw \"1.0\"", s.firmware)
        assertEquals(1761229500000L, s.utcUnixMs)
    }

    @Test
    fun errorsBecomeExceptions() {
        try {
            parseScan(resource("scan_error.json"))
            fail("expected LogReadException")
        } catch (e: LogReadException) {
            assertTrue(e.message!!.contains("not prepared"))
        }
        try {
            parseExportRows("""{"error":"boom"}""")
            fail("expected LogReadException")
        } catch (e: LogReadException) {
            assertEquals("boom", e.message)
        }
        assertEquals(300L, parseExportRows("""{"rows":300}"""))
    }
}

class SamplesTest {
    // session1_samples.csv is `tel-decode samples card_b.img --session 1`, the simulated 30 s drive.
    private val samples = parseSamplesCsv(resource("session1_samples.csv").reader().buffered())

    @Test
    fun readsEveryRowAndColumn() {
        assertEquals(300, samples.size)
        assertEquals(0.0, samples.t.first(), 0.0)
        assertEquals(29.9, samples.t.last(), 0.1)
        assertEquals(-35.2809, samples.lat.first(), 1e-9)
        assertEquals(149.13, samples.lon.first(), 1e-9)
        assertEquals(1.00002, samples["accel_mean_x_g"].first(), 1e-9)
        assertEquals(2.572 * 3.6, samples.speedKmh.first(), 1e-9)
    }

    @Test
    fun readsUtcAsEpochMilliseconds() {
        // 2025-10-23T14:25:00.020Z
        assertEquals(1761229500020L, samples.utcMs.first())
    }

    @Test
    fun missingColumnsAreNaN() {
        assertTrue(samples["no_such_column"].all { it.isNaN() })
    }

    @Test
    fun emptyFieldsAreNaNAndMeanNoFix() {
        val csv = "t_s,lat_deg,lon_deg,sog_mps,utc\n1.0,,,,\n1.1,-35.0,149.0,2.0,2025-10-23T14:25:00.000Z\n"
        val s = parseSamplesCsv(csv.reader().buffered())
        assertFalse(s.hasFix(0))
        assertTrue(s.hasFix(1))
        assertEquals(Samples.NO_TIME, s.utcMs[0])
        assertEquals(0.1, s.t[1], 1e-9)
    }

    @Test
    fun distanceAndMaxSpeed() {
        // 0.001 degrees of latitude is about 111.2 m
        val csv = "t_s,lat_deg,lon_deg,sog_mps\n0.0,-35.000,149.0,1.0\n1.0,-35.001,149.0,5.0\n"
        val s = parseSamplesCsv(csv.reader().buffered())
        assertEquals(111.2, s.distanceMetres, 0.5)
        assertEquals(18.0, s.maxSpeedKmh, 1e-9)
    }

    @Test
    fun indexAtTimeFindsTheFirstSampleAtOrAfter() {
        val t = doubleArrayOf(0.0, 0.1, 0.2, 0.3)
        assertEquals(0, indexAtTime(t, -1.0))
        assertEquals(2, indexAtTime(t, 0.15))
        assertEquals(2, indexAtTime(t, 0.2))
        assertEquals(3, indexAtTime(t, 99.0))
        assertEquals(0, indexAtTime(DoubleArray(0), 1.0))
    }

    @Test
    fun emptyInputIsAnEmptySession() {
        val s = parseSamplesCsv("".reader().buffered())
        assertEquals(0, s.size)
        assertEquals(0.0, s.distanceMetres, 0.0)
    }
}

class CanDataTest {
    private val signalsCsv = """session,t_s,bus,message,signal,raw,physical,unit
1,5.000000,2,SBG_ECAN_MSG_EKF_EULER,ROLL,100,0.01,rad
1,5.000000,2,SBG_ECAN_MSG_EKF_VEL_NED,VELOCITY_N,300,3.0,m.s-1
1,5.100000,2,SBG_ECAN_MSG_EKF_EULER,ROLL,200,0.02,rad
1,5.100000,2,SBG_ECAN_MSG_EKF_VEL_NED,VELOCITY_N,400,4.0,m.s-1
1,5.200000,2,SBG_ECAN_MSG_EKF_EULER,ROLL,300,0.03,rad
1,5.200000,2,SBG_ECAN_MSG_STATUS_03,SOLUTION,11508,11508,
"""

    @Test
    fun groupsSignalsAndHoldsValuesOntoTheSampleGrid() {
        val set = parseSignalsCsv(signalsCsv.reader().buffered())
        assertEquals(3, set.series.size)
        assertTrue(set.hasEllipse)
        val roll = set.sbg("EKF_EULER", "ROLL")!!
        assertEquals("EKF_EULER.ROLL", roll.label)
        assertEquals(0.01, roll.min, 1e-12)
        assertEquals(0.03, roll.max, 1e-12)
        // a grid that starts before the first value and has a point between two values
        val held = roll.heldOn(doubleArrayOf(4.9, 5.0, 5.05, 5.1, 5.25))
        assertTrue(held[0].isNaN())
        assertEquals(0.01, held[1], 0.0)
        assertEquals(0.01, held[2], 0.0)
        assertEquals(0.02, held[3], 0.0)
        assertEquals(0.03, held[4], 0.0)
        assertEquals(null, set.sbg("EKF_EULER", "PITCH"))
    }

    @Test
    fun parsesTheCanSummaryAndComputesRates() {
        val ids = parseCanSummary(
            """[{"bus":2,"id":306,"extended":false,"count":101,"firstUs":0,"lastUs":1000000,"dlc":6,"last":"0A0B0C0D0E0F","name":"SBG_ECAN_MSG_EKF_EULER"},
               {"bus":1,"id":1600,"extended":false,"count":1,"firstUs":5,"lastUs":5,"dlc":2,"last":"0102"}]""",
        )
        assertEquals(2, ids.size)
        assertEquals("0x132", ids[0].idText)
        assertEquals("EKF_EULER", ids[0].shortName)
        assertEquals(100.0, ids[0].rateHz, 1e-9)
        assertEquals("0x640", ids[1].idText)
        assertTrue(ids[1].rateHz.isNaN())
        assertEquals(null, ids[1].name)
    }

    @Test
    fun describesTheSolutionStatusWord() {
        val text = describeSolution(0x2CF4)
        assertTrue(text, text.startsWith("Navigation, position"))
        assertTrue(text, text.contains("valid: attitude, heading, velocity, position"))
        assertTrue(text, text.contains("GPS1 velocity, GPS1 position, GPS1 dual-antenna heading"))
        assertTrue(describeSolution(0x12).startsWith("AHRS"))
    }

    @Test
    fun ellipseViewDerivesSpeedAndYawOnTheSampleGrid() {
        val csv = "t_s,lat_deg,lon_deg,sog_mps\n5.0,-35.0,149.0,1.0\n5.1,-35.0,149.0,1.0\n"
        val samples = parseSamplesCsv(csv.reader().buffered())
        val sig = """session,t_s,bus,message,signal,raw,physical,unit
1,5.0,2,SBG_ECAN_MSG_EKF_VEL_NED,VELOCITY_N,0,3.0,m.s-1
1,5.0,2,SBG_ECAN_MSG_EKF_VEL_NED,VELOCITY_E,0,4.0,m.s-1
1,5.0,2,SBG_ECAN_MSG_EKF_EULER,YAW,0,-1.5707963267948966,rad
"""
        val e = EllipseView(samples, parseSignalsCsv(sig.reader().buffered()))
        assertTrue(e.present)
        assertEquals(5.0 * 3.6, e.speedKmh[1], 1e-9)
        assertEquals(270.0, e.yawDeg[0], 1e-9)
        assertTrue("no position from the Ellipse, so no gap", e.positionGapMetres[0].isNaN())
    }
}

class EllipseGpxTest {
    @Test
    fun writesOnePointPerSampleWithAnEllipsePositionAndSplitsAtGaps() {
        val csv = "t_s,lat_deg,lon_deg,sog_mps,utc\n" +
            "5.0,-35.0,149.0,1.0,2025-10-23T14:25:00.000Z\n" +
            "5.1,-35.0,149.0,1.0,2025-10-23T14:25:00.100Z\n" +
            "9.0,-35.0,149.0,1.0,2025-10-23T14:25:04.000Z\n"
        val samples = parseSamplesCsv(csv.reader().buffered())
        val sig = "session,t_s,bus,message,signal,raw,physical,unit\n" +
            "1,5.0,2,SBG_ECAN_MSG_EKF_POS,LATITUDE,0,-35.1234567,deg\n" +
            "1,5.0,2,SBG_ECAN_MSG_EKF_POS,LONGITUDE,0,149.7654321,deg\n" +
            "1,5.0,2,SBG_ECAN_MSG_EKF_ALTITUDE,ALTITUDE,0,584.25,m\n" +
            "1,5.0,2,SBG_ECAN_MSG_EKF_VEL_NED,VELOCITY_N,0,3.0,m.s-1\n" +
            "1,5.0,2,SBG_ECAN_MSG_EKF_VEL_NED,VELOCITY_E,0,4.0,m.s-1\n" +
            "1,5.0,2,SBG_ECAN_MSG_EKF_EULER,YAW,0,1.5707963267948966,rad\n"
        val e = EllipseView(samples, parseSignalsCsv(sig.reader().buffered()))
        val doc = ellipseGpx(samples, e, 3)
        // the Ellipse spoke once, at 5.0 s, and the value is held for every later sample
        assertEquals(3, doc.points)
        assertTrue(doc.xml.contains("<trkpt lat=\"-35.123456700\" lon=\"149.765432100\">"))
        assertTrue(doc.xml.contains("<ele>584.250</ele>"))
        assertTrue(doc.xml.contains("<time>2025-10-23T14:25:00Z</time>") || doc.xml.contains("<time>2025-10-23T14:25:00.000Z</time>"))
        assertTrue(doc.xml.contains("<tel:speed>5.000</tel:speed><tel:course>90.00</tel:course>"))
        // 3.9 s between the second and third sample starts a new segment
        assertEquals(2, Regex("<trkseg>").findAll(doc.xml).count())
        assertEquals(Regex("<trkseg>").findAll(doc.xml).count(), Regex("</trkseg>").findAll(doc.xml).count())
        assertTrue(doc.xml.contains("session 3 (SBG Ellipse)"))
    }
}

class SettingsTest {
    @Test
    fun defaultsRoundTripThroughTheSavedForm() {
        val s = Settings(
            theme = ThemeMode.Dark, speedUnit = SpeedUnit.MilesPerHour, accelUnit = AccelUnit.MetresPerSecondSquared,
            defaultSource = DefaultSource.LastUsed, lastSource = "Both",
        )
        val saved = s.toMap()
        assertEquals(s, Settings.from { saved[it] })
        assertEquals(Settings(), Settings.from { null })
    }

    @Test
    fun unknownOrOldSavedValuesFallBackToDefaults() {
        val saved = mapOf("theme" to "Sepia", "speedUnit" to "KilometresPerHour", "lastSource" to "Nonsense")
        val s = Settings.from { saved[it] }
        assertEquals(ThemeMode.System, s.theme)
        assertEquals("Onboard", s.lastSource)
        assertEquals(AccelUnit.G, s.accelUnit)
    }

    @Test
    fun convertsSpeedDistanceAndAcceleration() {
        val mph = Settings(speedUnit = SpeedUnit.MilesPerHour)
        assertEquals(62.1371192, mph.speed(100.0), 1e-6)
        assertEquals(6.21371192, mph.distance(10.0), 1e-6)
        val ms = Settings(speedUnit = SpeedUnit.MetresPerSecond)
        assertEquals(10.0, ms.speed(36.0), 1e-9)
        assertEquals(10.0, ms.distance(10.0), 1e-9)
        val a = Settings(accelUnit = AccelUnit.MetresPerSecondSquared).accel(doubleArrayOf(1.0, Double.NaN))
        assertEquals(9.80665, a[0], 1e-9)
        assertTrue(a[1].isNaN())
        val same = doubleArrayOf(1.0)
        assertTrue("km/h and g leave the array alone", Settings().speed(same) === same && Settings().accel(same) === same)
    }
}
