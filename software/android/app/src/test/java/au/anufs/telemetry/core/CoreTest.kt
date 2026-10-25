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
            defaultSource = DefaultSource.LastUsed, lastSource = "Both", minLapSeconds = 20, weighting = Weighting.PreferEllipse, useImu = false,
        )
        val saved = s.toMap()
        assertEquals(s, Settings.from { saved[it] })
        assertEquals(Settings(), Settings.from { null })
    }

    @Test
    fun unknownOrOldSavedValuesFallBackToDefaults() {
        val saved = mapOf("theme" to "Sepia", "speedUnit" to "KilometresPerHour", "lastSource" to "Nonsense", "minLapSeconds" to "7")
        val s = Settings.from { saved[it] }
        assertEquals(ThemeMode.System, s.theme)
        assertEquals("Fused", s.lastSource)
        assertEquals(AccelUnit.G, s.accelUnit)
        assertEquals(10, s.minLapSeconds)
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

class LapsTest {
    private val lat0 = -35.28
    private val lon0 = 149.13
    private val r = 50.0
    private val speed = 10.0 // m/s, so a lap of the circle takes 2*pi*50/10 = 31.4 s
    private val mLat = 111_320.0
    private val mLon = 111_320.0 * Math.cos(Math.toRadians(lat0))

    /** A car going anticlockwise round a circle, starting [startDeg] past east, for [seconds] at 10 Hz. */
    private fun circle(seconds: Double, startDeg: Double = 10.0): Triple<DoubleArray, DoubleArray, DoubleArray> {
        val n = (seconds * 10).toInt() + 1
        val t = DoubleArray(n) { it / 10.0 }
        val w = speed / r
        val lat = DoubleArray(n) { lat0 + r * Math.sin(Math.toRadians(startDeg) + w * t[it]) / mLat }
        val lon = DoubleArray(n) { lon0 + r * Math.cos(Math.toRadians(startDeg) + w * t[it]) / mLon }
        return Triple(t, lat, lon)
    }

    private fun speeds(n: Int) = DoubleArray(n) { speed * 3.6 }

    /** A short line across the track at the east point (angle 0), drawn by hand: direction unknown. */
    private val eastLine = GeoLine(lat0 - 1e-5 * 0, lon0 + (r - 8) / mLon, lat0, lon0 + (r + 8) / mLon, 0)
        .let { GeoLine(lat0, lon0 + (r - 8) / mLon, lat0, lon0 + (r + 8) / mLon, 0) }

    @Test
    fun countsLapsOfACircleAndTimesThemFromInterpolatedCrossings() {
        val (t, lat, lon) = circle(100.0) // 3.2 laps, first crossing at ~ (360-10)/ (w in deg/s) s
        val res = detectLaps(t, lat, lon, speeds(t.size), eastLine)
        assertEquals(3, res.crossings.size)
        assertEquals(2, res.laps.size)
        val lapTime = 2 * Math.PI * r / speed
        res.laps.forEach { assertEquals(lapTime, it.timeS, 0.02) }
        assertEquals(1, res.laps[0].number)
        assertEquals(2 * Math.PI * r, res.laps[0].distanceMetres, 3.0) // the steps at the two ends are not counted
        assertEquals(36.0, res.laps[0].maxKmh, 1e-9)
        assertEquals(36.0, res.laps[0].avgKmh, 0.2)
        // out lap: from the start (10 degrees past the line) round to the line
        assertEquals(lapTime * 350 / 360, res.outLapS, 0.02)
        assertEquals(100.0 - res.crossings.last(), res.inLapS, 1e-9)
    }

    @Test
    fun theLinesDirectionAndTheShortestLapDecideWhatCounts() {
        val (t, lat, lon) = circle(100.0)
        val sp = speeds(t.size)
        fun withDir(d: Int) = GeoLine(eastLine.lat1, eastLine.lon1, eastLine.lat2, eastLine.lon2, dir = d)
        // the line runs west to east and the car crosses it going north, which is dir +1
        assertEquals(3, detectLaps(t, lat, lon, sp, withDir(1)).crossings.size)
        assertEquals(0, detectLaps(t, lat, lon, sp, withDir(-1)).crossings.size)
        // a hand-drawn line (dir 0) takes the direction most crossings go
        assertEquals(3, detectLaps(t, lat, lon, sp, eastLine).crossings.size)
        // crossings 31 s apart are two laps; a 70 s shortest lap keeps only the first crossing
        assertEquals(1, detectLaps(t, lat, lon, sp, eastLine, minLapSeconds = 70.0).crossings.size)
        assertEquals(0, detectLaps(t, lat, lon, sp, eastLine, minLapSeconds = 70.0).laps.size)
    }

    @Test
    fun aLineMadeAtASampleCountsTheCarsOwnDirectionOnly() {
        val (t, lat, lon) = circle(100.0)
        val i = 200 // 20 s in
        val line = lineAtSample(lat, lon, i, halfLengthMetres = 10.0)!!
        assertEquals(1, line.dir)
        assertEquals(20.0, line.lengthMetres, 0.1)
        val res = detectLaps(t, lat, lon, speeds(t.size), line)
        // the car was on the line at 20 s, and passes it again every lap
        val lapTime = 2 * Math.PI * r / speed
        assertEquals(20.0, res.crossings[0], 0.15)
        assertEquals(20.0 + lapTime, res.crossings[1], 0.15)
        // the same car driving the other way never crosses it in the line's direction
        val back = detectLaps(t, lat.reversedArray(), lon.reversedArray(), speeds(t.size), line)
        assertEquals(0, back.crossings.size)
    }

    @Test
    fun gapsInTheFixesDoNotMakeLapsOrBreakTheCount() {
        val (t, lat, lon) = circle(100.0)
        // lose the fix for five seconds around the second crossing (about 63 s)
        for (i in 600..660) { lat[i] = Double.NaN; lon[i] = Double.NaN }
        val res = detectLaps(t, lat, lon, speeds(t.size), eastLine)
        assertEquals("the crossing at ~63 s fell in the gap", 2, res.crossings.size)
        assertEquals(1, res.laps.size)
    }

    @Test
    fun savedLinesAreChosenByTheLapsTheyFindAndKeptTidy() {
        val (t, lat, lon) = circle(100.0)
        val farAway = GeoLine(lat0 + 0.01, lon0, lat0 + 0.01, lon0 + 0.0001, 0)
        assertEquals(eastLine, bestSavedLine(t, lat, lon, speeds(t.size), listOf(farAway, eastLine)))
        assertEquals(null, bestSavedLine(t, lat, lon, speeds(t.size), listOf(farAway)))
        // a line 10 m from an old one replaces it; 100 m away is kept alongside
        val near = GeoLine(eastLine.lat1 + 5e-5, eastLine.lon1, eastLine.lat2 + 5e-5, eastLine.lon2, 0)
        val far = GeoLine(eastLine.lat1 + 1e-3, eastLine.lon1, eastLine.lat2 + 1e-3, eastLine.lon2, 0)
        assertEquals(listOf(near), rememberLine(listOf(eastLine), near))
        assertEquals(listOf(far, eastLine), rememberLine(listOf(eastLine), far))
        assertEquals(20, (1..40).fold(emptyList<GeoLine>()) { acc, k -> rememberLine(acc, GeoLine(lat0 + k * 1e-3, lon0, lat0 + k * 1e-3, lon0 + 1e-4, 0)) }.size)
    }

    @Test
    fun linesRoundTripThroughTheSavedForm() {
        val l = GeoLine(-35.1234567, 149.7654321, -35.1234000, 149.7655000, 1)
        assertEquals(l, GeoLine.parse(l.encode()))
        assertEquals(null, GeoLine.parse("1,2,3"))
        assertEquals(null, GeoLine.parse("a,b,c,d,1"))
    }
}

class FusedExportTest {
    @Test
    fun fusedGpxCarriesTheFusedTrackAndSplitsWhereItHasNoPosition() {
        val csv = "t_s,lat_deg,lon_deg,sog_mps,utc\n" +
            "5.0,-35.0,149.0,1.0,2025-10-23T14:25:00.000Z\n" +
            "5.1,-35.0,149.0,1.0,2025-10-23T14:25:00.100Z\n" +
            "5.2,-35.0,149.0,1.0,2025-10-23T14:25:00.200Z\n"
        val s = parseSamplesCsv(csv.reader().buffered())
        val f = FusedTrack(
            lat = doubleArrayOf(-35.0, Double.NaN, -35.0002), lon = doubleArrayOf(149.0, Double.NaN, 149.0002),
            altM = doubleArrayOf(584.0, Double.NaN, 585.0), speedKmh = doubleArrayOf(36.0, Double.NaN, 72.0),
            courseDeg = doubleArrayOf(90.0, Double.NaN, 180.0), sigmaM = doubleArrayOf(0.5, Double.NaN, 0.5), validCount = 2,
        )
        val doc = fusedGpx(s, f, 7)
        assertEquals(2, doc.points)
        assertEquals("a missing position ends the segment", 2, Regex("<trkseg>").findAll(doc.xml).count())
        assertTrue(doc.xml.contains("(fused best estimate)"))
        assertTrue(doc.xml.contains("<tel:speed>10.000</tel:speed><tel:course>90.00</tel:course>"))
        assertTrue(doc.xml.contains("<ele>585.000</ele>"))
    }

    @Test
    fun fuseSessionReadsTheColumnsAndFusesTheOnboardTrackAlone() {
        val rows = (0 until 50).joinToString("\n") { i ->
            val lat = -35.0 + i * 1e-5 // 1.11 m north per 0.1 s, so 11.1 m/s
            "${5.0 + i * 0.1},$lat,149.0,11.1,0.0,0.8,1,584.0"
        }
        val csv = "t_s,lat_deg,lon_deg,sog_mps,cog_deg,hdop,fix_quality,alt_m\n$rows\n"
        val s = parseSamplesCsv(csv.reader().buffered())
        val f = fuseSession(s, null)
        assertEquals(50, f.validCount)
        assertEquals(11.1 * 3.6, f.speedKmh[25], 0.5)
        assertEquals(0.0, (f.courseDeg[25] + 180) % 360 - 180, 1.0)
        assertEquals(584.0, f.altM[25], 1e-9)
    }
}
