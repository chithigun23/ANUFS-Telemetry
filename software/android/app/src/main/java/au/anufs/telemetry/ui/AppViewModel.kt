package au.anufs.telemetry.ui

import android.app.Application
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import au.anufs.telemetry.core.CanData
import au.anufs.telemetry.core.EllipseView
import au.anufs.telemetry.core.GeoLine
import au.anufs.telemetry.core.rememberLine
import au.anufs.telemetry.core.ellipseGpx
import au.anufs.telemetry.core.CardScan
import au.anufs.telemetry.core.LogReadException
import au.anufs.telemetry.core.Native
import au.anufs.telemetry.core.Samples
import au.anufs.telemetry.core.Settings
import au.anufs.telemetry.core.Session
import au.anufs.telemetry.core.parseCanSummary
import au.anufs.telemetry.core.parseExportRows
import au.anufs.telemetry.core.parseSignalsCsv
import au.anufs.telemetry.core.parseSamplesCsv
import au.anufs.telemetry.core.parseScan
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.FileInputStream
import java.io.IOException

/** A session whose samples are loaded for viewing. */
class OpenSession(val session: Session, val samples: Samples)

/** What can be exported from an open session. */
enum class ExportKind(val native: String, val extension: String, val mime: String) {
    GPX("gpx", "gpx", "application/gpx+xml"),
    GPX_ELLIPSE("gpx-ellipse", "gpx", "application/gpx+xml"),
    CSV("samples", "csv", "text/csv"),
}

class AppViewModel(app: Application) : AndroidViewModel(app) {
    var busy by mutableStateOf(false)
        private set
    var message by mutableStateOf<String?>(null)
        private set
    var sourceName by mutableStateOf<String?>(null)
        private set
    var card by mutableStateOf<CardScan?>(null)
        private set
    var open by mutableStateOf<OpenSession?>(null)
        private set

    /** CAN signals and per-id statistics of the open session; loaded when the CAN tab is first shown. */
    var can by mutableStateOf<CanData?>(null)
        private set
    var canBusy by mutableStateOf(false)
        private set

    private val prefs = app.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE)

    /** User settings, loaded from and saved to the phone. */
    var settings by mutableStateOf(Settings.from { prefs.getString(it, null) })
        private set

    /** Change settings and save the result. */
    fun updateSettings(change: (Settings) -> Settings) {
        settings = change(settings)
        prefs.edit().apply { settings.toMap().forEach { (k, v) -> putString(k, v) } }.apply()
    }

    /** Start/finish lines the user has set, newest first; a session whose track crosses one gets laps without asking. */
    var savedLines by mutableStateOf(
        (prefs.getString("sf_lines", "") ?: "").split(';').mapNotNull { GeoLine.parse(it) },
    )
        private set

    private fun persistLines() {
        prefs.edit().putString("sf_lines", savedLines.joinToString(";") { it.encode() }).apply()
    }

    fun saveLine(line: GeoLine) {
        savedLines = rememberLine(savedLines, line)
        persistLines()
    }

    fun forgetLine(line: GeoLine) {
        savedLines = savedLines.filter { it != line }
        persistLines()
    }

    private var pfd: ParcelFileDescriptor? = null
    private var imagePath: String? = null

    /** Debug builds only: forget saved settings and lines, so a test starts from nothing. */
    fun resetState() {
        prefs.edit().clear().apply()
        savedLines = emptyList()
        settings = Settings()
    }

    /** Show [text] in the snackbar. */
    fun notify(text: String) {
        message = text
    }

    fun dismissMessage() {
        message = null
    }

    fun closeSession() {
        open = null
        can = null
    }

    /** Load the CAN data of the open session (once). */
    fun loadCan() {
        val path = imagePath ?: return
        val s = open?.session ?: return
        if (can != null || canBusy) return
        viewModelScope.launch {
            canBusy = true
            try {
                can = withContext(Dispatchers.IO) {
                    val dir = getApplication<Application>().cacheDir
                    val sig = File(dir, "signals_${s.number}.csv")
                    val sum = File(dir, "can_summary_${s.number}.json")
                    parseExportRows(Native.export(path, s.number, "signals", sig.path))
                    parseExportRows(Native.export(path, s.number, "can-summary", sum.path))
                    CanData(sig.bufferedReader().use(::parseSignalsCsv), parseCanSummary(sum.readText()))
                }
            } catch (e: Exception) {
                message = e.message ?: e.toString()
            } finally {
                canBusy = false
            }
        }
    }

    /** Open a card image the user picked and list its sessions. */
    fun openImage(uri: Uri) {
        viewModelScope.launch {
            busy = true
            try {
                val (path, name) = withContext(Dispatchers.IO) { resolveImage(uri) }
                card = withContext(Dispatchers.IO) { parseScan(Native.scan(path)) }
                imagePath = path
                sourceName = name
                open = null
                can = null
                message = null
            } catch (e: Exception) {
                message = e.message ?: e.toString()
            } finally {
                busy = false
            }
        }
    }

    /** Debug builds only: open a card image by plain path (`adb shell am start --es open_path ...`). */
    fun openPath(path: String, session: Int? = null) {
        viewModelScope.launch {
            busy = true
            try {
                val scan = withContext(Dispatchers.IO) { parseScan(Native.scan(path)) }
                card = scan
                imagePath = path
                sourceName = File(path).name
                open = null
                can = null
                scan.sessions.firstOrNull { session == null || it.number == session }?.let { openSession(it) }
            } catch (e: Exception) {
                message = e.message ?: e.toString()
            } finally {
                busy = false
            }
        }
    }

    /** Load the samples of [session] for the viewer. */
    fun openSession(session: Session) {
        val path = imagePath ?: return
        viewModelScope.launch {
            busy = true
            try {
                can = null
                open = withContext(Dispatchers.IO) {
                    val out = File(getApplication<Application>().cacheDir, "session_${session.number}.csv")
                    parseExportRows(Native.export(path, session.number, "samples", out.path))
                    OpenSession(session, out.bufferedReader().use(::parseSamplesCsv))
                }
            } catch (e: Exception) {
                message = e.message ?: e.toString()
            } finally {
                busy = false
            }
        }
    }

    /** Suggested file name for an export of the open session. */
    fun exportName(kind: ExportKind): String {
        val s = open?.session ?: return "telemetry.${kind.extension}"
        val base = sourceName?.substringBeforeLast('.') ?: "telemetry"
        val tag = if (kind == ExportKind.GPX_ELLIPSE) "_ellipse" else ""
        return "${base}_session${s.number}$tag.${kind.extension}"
    }

    /** Write the open session as [kind] to the document [dest] the user chose. */
    fun export(kind: ExportKind, dest: Uri) {
        val path = imagePath ?: return
        val s0 = open ?: return
        val s = s0.session
        viewModelScope.launch {
            busy = true
            try {
                val rows = withContext(Dispatchers.IO) {
                    val app = getApplication<Application>()
                    val out = app.contentResolver.openOutputStream(dest, "wt")
                        ?: throw LogReadException("cannot write to the chosen file")
                    if (kind == ExportKind.GPX_ELLIPSE) {
                        // Built here from the logged Ellipse signals, not by the Rust exporter.
                        val signals = can?.signals ?: throw LogReadException("the CAN data is still loading, try again")
                        val doc = ellipseGpx(s0.samples, EllipseView(s0.samples, signals), s.number)
                        out.use { it.write(doc.xml.toByteArray(Charsets.UTF_8)) }
                        doc.points.toLong()
                    } else {
                        val tmp = File(app.cacheDir, "export_${s.number}.${kind.extension}")
                        val n = parseExportRows(Native.export(path, s.number, kind.native, tmp.path))
                        out.use { o -> tmp.inputStream().use { it.copyTo(o) } }
                        tmp.delete()
                        n
                    }
                }
                message = "Saved $rows ${if (kind == ExportKind.GPX) "track points" else "rows"}"
            } catch (e: Exception) {
                message = e.message ?: e.toString()
            } finally {
                busy = false
            }
        }
    }

    /**
     * The reader opens a path. A picked file is reachable as /proc/self/fd/N without a copy;
     * if that path cannot be opened (the provider hands out a pipe), copy the file to the cache.
     */
    private fun resolveImage(uri: Uri): Pair<String, String> {
        val app = getApplication<Application>()
        val name = app.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            ?.use { if (it.moveToFirst()) it.getString(0) else null } ?: "card image"
        pfd?.close()
        val fd = app.contentResolver.openFileDescriptor(uri, "r")
            ?: throw LogReadException("cannot open $name")
        pfd = fd
        val direct = "/proc/self/fd/${fd.fd}"
        try {
            FileInputStream(direct).use { it.channel.size() }
            return direct to name
        } catch (_: IOException) {
            // fall through to a copy
        }
        val copy = File(app.cacheDir, "image.img")
        FileInputStream(fd.fileDescriptor).use { i -> copy.outputStream().use { o -> i.copyTo(o) } }
        return copy.path to name
    }

    override fun onCleared() {
        pfd?.close()
    }
}
