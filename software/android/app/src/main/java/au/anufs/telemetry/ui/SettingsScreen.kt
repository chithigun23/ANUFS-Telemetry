package au.anufs.telemetry.ui

import android.content.Context
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import au.anufs.telemetry.BuildConfig
import au.anufs.telemetry.core.AccelUnit
import au.anufs.telemetry.core.DefaultSource
import au.anufs.telemetry.core.Settings
import au.anufs.telemetry.core.SpeedUnit
import au.anufs.telemetry.core.ThemeMode
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File
import java.util.Locale

/** The settings in force, for any composable that formats a number. */
val LocalSettings = androidx.compose.runtime.compositionLocalOf { Settings() }

private fun tileCacheDir(context: Context) = File(context.cacheDir, "osmdroid")

private fun dirSize(f: File): Long = if (f.isFile) f.length() else f.listFiles()?.sumOf { dirSize(it) } ?: 0L

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(settings: Settings, onChange: ((Settings) -> Settings) -> Unit, onBack: () -> Unit) {
    val context = LocalContext.current
    var cacheVersion by remember { mutableIntStateOf(0) }
    val cacheBytes by produceState(0L, cacheVersion) { value = withContext(Dispatchers.IO) { dirSize(tileCacheDir(context)) } }
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Settings") },
                navigationIcon = { TextButton(onClick = onBack) { Text("‹ Back") } },
            )
        },
    ) { pad ->
        Column(Modifier.padding(pad).fillMaxSize().verticalScroll(rememberScrollState())) {
            Group("Display")
            Choice("Theme", ThemeMode.entries, settings.theme, { it.label }) { v -> onChange { it.copy(theme = v) } }
            Choice("Speed and distance", SpeedUnit.entries, settings.speedUnit, { it.label }) { v -> onChange { it.copy(speedUnit = v) } }
            Choice("Acceleration", AccelUnit.entries, settings.accelUnit, { it.label }) { v -> onChange { it.copy(accelUnit = v) } }
            Hint("The g-g diagram always uses g.")
            Choice("Source to open on, when a session has Ellipse data", DefaultSource.entries, settings.defaultSource, { it.label }) { v ->
                onChange { it.copy(defaultSource = v) }
            }

            Group("Map")
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Map tile cache", style = MaterialTheme.typography.bodyLarge)
                    Text(
                        String.format(Locale.US, "%.1f MB of OpenStreetMap tiles kept on the phone", cacheBytes / 1e6),
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                TextButton(enabled = cacheBytes > 0, onClick = { tileCacheDir(context).deleteRecursively(); cacheVersion++ }) { Text("Clear") }
            }

            Group("About")
            Hint("ANUFS Telemetry ${BuildConfig.VERSION_NAME}. Reads logs with the same Rust code as the board's firmware and tel-decode.")
            Hint("SBG CAN database: MIT licence, sbgECom. Maps: OpenStreetMap contributors.")
        }
    }
}

@Composable
private fun Group(title: String) {
    HorizontalDivider(Modifier.padding(top = 8.dp))
    Text(title, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 16.dp, top = 12.dp, bottom = 4.dp))
}

@Composable
private fun Hint(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 16.dp, vertical = 2.dp))
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun <T> Choice(title: String, options: List<T>, selected: T, label: (T) -> String, onSelect: (T) -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) {
        Text(title, style = MaterialTheme.typography.bodyLarge)
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            options.forEach { o -> FilterChip(selected = o == selected, onClick = { onSelect(o) }, label = { Text(label(o)) }) }
        }
    }
}
