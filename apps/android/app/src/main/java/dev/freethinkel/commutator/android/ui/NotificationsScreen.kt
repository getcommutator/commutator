package dev.freethinkel.commutator.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.design.ZIcons

/** Settings › Notifications: session notifications and the agents' live update. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun NotificationsScreen(model: AppModel, onBack: () -> Unit) {
    LazyColumn(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background),
        contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 48.dp),
    ) {
        item {
            Box(Modifier.padding(start = 12.dp, top = 4.dp)) {
                TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            }
        }
        item { ScreenHeader("Notifications", null) }
        section("Sessions")
        item { NotificationSettings(model) }
        section("Live updates")
        item {
            val on by model.push.activity.collectAsState()
            SegmentedListItem(
                onClick = { model.push.setActivity(!on) },
                shapes = segmentedShapes(0, 1),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Alarm) },
                supportingContent = { Text("Your agents in an ongoing notification, after you send work from here") },
                trailingContent = { Switch(on, { model.push.setActivity(it) }) },
                modifier = Modifier.padding(horizontal = 16.dp),
            ) { Text("Agent activity") }
        }
    }
}

/** The master switch and, while it's on and Android allows them, one per kind (as on iOS). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun NotificationSettings(model: AppModel) {
    val push = model.push
    val enabled by push.enabled.collectAsState()
    val kinds by push.kinds.collectAsState()
    val context = androidx.compose.ui.platform.LocalContext.current
    // Re-read when Settings comes back to the front (allowed in Android's settings meanwhile).
    var allowed by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(push.allowed()) }
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current
    androidx.compose.runtime.DisposableEffect(lifecycle) {
        val observer = androidx.lifecycle.LifecycleEventObserver { _, e ->
            if (e == androidx.lifecycle.Lifecycle.Event.ON_RESUME) allowed = push.allowed()
        }
        lifecycle.lifecycle.addObserver(observer)
        onDispose { lifecycle.lifecycle.removeObserver(observer) }
    }
    val asked by push.askPermission.collectAsState()
    LaunchedEffect(asked) { if (!asked) allowed = push.allowed() }
    val on = enabled && allowed
    fun toggle(want: Boolean) {
        when {
            !want -> push.setEnabled(false)
            // Turned off for the app in Android's settings: only there can it come back.
            !allowed && enabled -> context.startActivity(
                android.content.Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, context.packageName),
            )
            else -> push.setEnabled(true)
        }
    }
    val rows = if (on) 1 + dev.freethinkel.commutator.android.core.Push.Kind.entries.size else 1
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = { toggle(!on) },
            shapes = segmentedShapes(0, rows),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { IconTile(ZIcons.Bell) },
            supportingContent = {
                Text(if (enabled && !allowed) "Turned off in Android settings" else "When a session finishes, needs you or fails")
            },
            trailingContent = { Switch(on, { toggle(it) }) },
        ) { Text("Session notifications") }
        if (on) {
            dev.freethinkel.commutator.android.core.Push.Kind.entries.forEachIndexed { i, kind ->
                val kindOn = kinds[kind] != false
                SegmentedListItem(
                    onClick = { push.set(kind, !kindOn) },
                    shapes = segmentedShapes(i + 1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = {
                        IconTile(
                            when (kind) {
                                dev.freethinkel.commutator.android.core.Push.Kind.Done -> ZIcons.Check
                                dev.freethinkel.commutator.android.core.Push.Kind.Input -> ZIcons.Question
                                dev.freethinkel.commutator.android.core.Push.Kind.Failed -> ZIcons.Warning
                            },
                        )
                    },
                    trailingContent = { Switch(kindOn, { push.set(kind, it) }) },
                ) { Text(kind.title) }
            }
        }
    }
}
