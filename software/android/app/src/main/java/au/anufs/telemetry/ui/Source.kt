package au.anufs.telemetry.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/**
 * Which receiver and IMU a view draws. The board's own LC29H pair and ASM330 and the SBG
 * Ellipse on CAN are independent sources of the same quantities; the app does not rank them.
 */
enum class Source(val label: String) {
    Onboard("Onboard"),
    Ellipse("Ellipse"),
    Both("Both"),
}

val Source.showsOnboard: Boolean get() = this != Source.Ellipse
val Source.showsEllipse: Boolean get() = this != Source.Onboard

/** Source picker; shown only when the session has Ellipse data. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SourceBar(source: Source, onSource: (Source) -> Unit) {
    Row(
        Modifier.padding(horizontal = 16.dp, vertical = 2.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("Source", style = MaterialTheme.typography.labelMedium)
        Source.entries.forEach { s ->
            FilterChip(selected = source == s, onClick = { onSource(s) }, label = { Text(s.label) })
        }
    }
}

/** Legend line for charts that draw both sources. */
@Composable
fun BothLegend() {
    Text(
        "Solid: onboard · dashed: Ellipse",
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = 16.dp),
    )
}
