package dev.freethinkel.vardoger.android.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.freethinkel.vardoger.android.core.AppModel
import uniffi.vardoger_core.CoreException
import uniffi.vardoger_core.ServerDevice
import uniffi.vardoger_core.relativeTimeLabel

/** Signed in, no machine picked yet: the account's machines, online first. */
@Composable
fun MachinePickerScreen(model: AppModel) {
    var machines by remember { mutableStateOf<List<ServerDevice>?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    LaunchedEffect(reload) {
        try {
            machines = model.machines().filter { it.isMachine }.sortedWith(compareBy({ !it.online }, { it.name }))
            failure = null
        } catch (e: CoreException.Auth) {
            // The server no longer knows this phone (removed there).
            model.signOut()
        } catch (e: Exception) {
            failure = "Couldn't reach your server."
        }
    }
    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        ScreenHeader("Machines", "Pick the computer to work on") {
            TextButton(onClick = { reload++ }) { Text("Refresh") }
            TextButton(onClick = { model.signOut() }) { Text("Sign out") }
        }
        val now = System.currentTimeMillis()
        LazyColumn(Modifier.padding(horizontal = 8.dp)) {
            items(machines.orEmpty(), key = { it.id }) { m ->
                ListItem(
                    headlineContent = { Text(m.name) },
                    supportingContent = {
                        Text(if (m.online) "Online" else "Offline · last seen ${relativeTimeLabel(m.lastSeen * 1000, now)}")
                    },
                    modifier = Modifier.clickable { model.choose(m) },
                )
            }
            item {
                val note = failure ?: when {
                    machines == null -> "Loading…"
                    machines!!.isEmpty() -> "No machines yet. Run `vardoger login` on a computer to add it to ${model.accountDetail}."
                    else -> "Signed in as ${model.accountName}."
                }
                Text(note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(16.dp))
            }
        }
    }
}
