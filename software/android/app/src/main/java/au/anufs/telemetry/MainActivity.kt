package au.anufs.telemetry

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.lifecycle.viewmodel.compose.viewModel
import au.anufs.telemetry.core.ThemeMode
import au.anufs.telemetry.ui.AppViewModel
import au.anufs.telemetry.ui.LocalSettings
import au.anufs.telemetry.ui.SettingsScreen
import au.anufs.telemetry.ui.ExportKind
import au.anufs.telemetry.ui.HomeScreen
import au.anufs.telemetry.ui.SessionScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            val vm: AppViewModel = viewModel()
            if (BuildConfig.DEBUG) {
                androidx.compose.runtime.LaunchedEffect(Unit) {
                    intent?.getStringExtra("open_path")?.let { vm.openPath(it, intent.getIntExtra("open_session", -1).takeIf { n -> n >= 0 }) }
                }
            }
            val settings = vm.settings
            val dark = when (settings.theme) {
                ThemeMode.System -> isSystemInDarkTheme()
                ThemeMode.Light -> false
                ThemeMode.Dark -> true
            }
            MaterialTheme(colorScheme = if (dark) darkColorScheme() else lightColorScheme()) {
                CompositionLocalProvider(LocalSettings provides settings) {
                    val pickImage = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
                        if (uri != null) vm.openImage(uri)
                    }
                    var pending by remember { mutableStateOf<ExportKind?>(null) }
                    val saveGpx = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(ExportKind.GPX.mime)) { uri ->
                        if (uri != null) vm.export(pending ?: ExportKind.GPX, uri)
                    }
                    val saveCsv = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(ExportKind.CSV.mime)) { uri ->
                        if (uri != null) vm.export(ExportKind.CSV, uri)
                    }
                    var showSettings by rememberSaveable { mutableStateOf(false) }
                    val open = vm.open
                    if (showSettings) {
                        BackHandler { showSettings = false }
                        SettingsScreen(settings, vm::updateSettings, onBack = { showSettings = false })
                    } else if (open == null) {
                        HomeScreen(vm, onPickImage = { pickImage.launch(arrayOf("*/*")) }, onSettings = { showSettings = true })
                    } else {
                        BackHandler { vm.closeSession() }
                        SessionScreen(
                            vm, open,
                            onBack = { vm.closeSession() },
                            onSettings = { showSettings = true },
                            onExport = { kind ->
                                pending = kind
                                when (kind) {
                                    ExportKind.GPX, ExportKind.GPX_ELLIPSE -> saveGpx.launch(vm.exportName(kind))
                                    ExportKind.CSV -> saveCsv.launch(vm.exportName(kind))
                                }
                            },
                        )
                    }
                }
            }
        }
    }
}
