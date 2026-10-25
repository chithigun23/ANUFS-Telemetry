package au.anufs.telemetry.core

/**
 * Fuse a session's onboard receiver and, when there is one, the Ellipse into one track. The fusion
 * itself is [fuse]; this reads the columns it needs out of the session.
 */
fun fuseSession(s: Samples, e: EllipseView?, tuning: FusionTuning = FusionTuning()): FusedTrack {
    val sog = s["sog_mps"]
    val cog = s["cog_deg"]
    val inputs = FusionInputs(
        t = s.t,
        onLat = s.lat, onLon = s.lon,
        onSog = sog, onCog = cog,
        onHdop = s["hdop"], onFixQuality = s["fix_quality"], onAlt = s["alt_m"],
        elLat = e?.lat, elLon = e?.lon, elVelN = e?.velN, elVelE = e?.velE, elAlt = e?.altM,
        imus = buildList {
            // The board's IMU, turned by the dual-antenna heading; the Ellipse's, by its own yaw. The mounting of
            // neither is assumed: fuse() fits it from the GNSS motion and drops an IMU that does not match.
            val g = 9.80665
            val ax = s["accel_mean_x_g"]
            val ay = s["accel_mean_y_g"]
            val h = s["heading_deg"]
            add(ImuInput("Onboard IMU", DoubleArray(s.size) { ax[it] * g }, DoubleArray(s.size) { ay[it] * g }, h))
            if (e != null) add(ImuInput("Ellipse IMU", DoubleArray(s.size) { e.accelXg[it] * g }, DoubleArray(s.size) { e.accelYg[it] * g }, e.yawDeg))
        },
    )
    return fuse(inputs, tuning)
}

/** The [p] quantile (0 to 1) of the values that are numbers, or NaN if there are none. */
fun percentile(values: DoubleArray, p: Double): Double {
    val v = values.filter { !it.isNaN() }.sorted()
    if (v.isEmpty()) return Double.NaN
    val pos = p.coerceIn(0.0, 1.0) * (v.size - 1)
    val lo = pos.toInt()
    val hi = minOf(lo + 1, v.size - 1)
    return v[lo] + (v[hi] - v[lo]) * (pos - lo)
}

/** Root mean square of the values that are numbers, or NaN. */
fun rms(values: DoubleArray): Double {
    val v = values.filter { !it.isNaN() }
    return if (v.isEmpty()) Double.NaN else kotlin.math.sqrt(v.sumOf { it * it } / v.size)
}

/** Distance in metres between two tracks, sample by sample (NaN where either has no position). */
fun trackGap(latA: DoubleArray, lonA: DoubleArray, latB: DoubleArray, lonB: DoubleArray): DoubleArray =
    DoubleArray(latA.size) {
        if (latA[it].isNaN() || lonA[it].isNaN() || latB[it].isNaN() || lonB[it].isNaN()) Double.NaN
        else haversine(latA[it], lonA[it], latB[it], lonB[it])
    }

/** Share of values that are numbers, 0 to 1. */
fun presence(values: DoubleArray): Double = if (values.isEmpty()) 0.0 else values.count { !it.isNaN() }.toDouble() / values.size

/** Length of a track in metres, over the samples that have a position. */
fun trackLengthMetres(lat: DoubleArray, lon: DoubleArray): Double {
    var d = 0.0
    var prev = -1
    for (i in lat.indices) {
        if (lat[i].isNaN() || lon[i].isNaN()) continue
        if (prev >= 0) d += haversine(lat[prev], lon[prev], lat[i], lon[i])
        prev = i
    }
    return d
}
