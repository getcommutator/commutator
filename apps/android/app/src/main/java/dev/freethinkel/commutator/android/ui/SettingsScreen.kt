package dev.freethinkel.commutator.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.core.AppUpdate
import dev.freethinkel.commutator.android.design.ZIcon
import dev.freethinkel.commutator.android.design.ZIcons
import uniffi.commutator_core.coreVersion
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SettingsScreen(
    model: AppModel,
    onArchived: () -> Unit,
    onAppearance: () -> Unit = {},
    onBack: (() -> Unit)? = null,
) {
    val appearance by model.appearance.collectAsState()
    val groupByProject by model.groupByProject.collectAsState()
    var confirmSignOut by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    val workspace by model.workspace.collectAsState()
    val devices = workspace?.devices.orEmpty()
    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    val context = androidx.compose.ui.platform.LocalContext.current
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    LaunchedEffect(Unit) { AppUpdate.check(context) }
    Box(Modifier.fillMaxSize()) {
    LazyColumn(
        Modifier.fillMaxSize(),
        state = list,
        contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 48.dp),
    ) {
        if (onBack != null) {
            item {
                Box(Modifier.padding(start = 12.dp, top = 4.dp)) {
                    TonalCircleButton(dev.freethinkel.commutator.android.design.ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                }
            }
        }
        item { ScreenHeader("Settings", null) }
        item {
            // Account: a tonal hero card with a shaped monogram.
            Surface(
                shape = RoundedCornerShape(32.dp),
                color = MaterialTheme.colorScheme.primaryContainer,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
            ) {
                Row(Modifier.padding(20.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(
                        Modifier.size(64.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.primary),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(model.accountName.take(1).uppercase(), style = MaterialTheme.typography.headlineSmallEmphasized, color = MaterialTheme.colorScheme.onPrimary)
                    }
                    Spacer(Modifier.width(16.dp))
                    Column {
                        Text(model.accountName, style = MaterialTheme.typography.titleLargeEmphasized, color = MaterialTheme.colorScheme.onPrimaryContainer)
                        Text(model.accountDetail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onPrimaryContainer.copy(alpha = 0.8f))
                    }
                }
            }
        }
        // The machine this phone drives; the account's others are a tap away.
        model.host?.takeIf { !model.isDemo }?.let { host ->
            section("Machine")
            item {
                SegmentedListItem(
                    onClick = { model.leaveMachine() },
                    shapes = segmentedShapes(0, 1),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Laptop) },
                    supportingContent = { Text("Switch to another machine") },
                    modifier = Modifier.padding(horizontal = 16.dp),
                ) { Text(host.name) }
            }
        }
        section("Display")
        item {
            SegmentedListItem(
                onClick = onAppearance,
                shapes = segmentedShapes(0, 1),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Magic) },
                supportingContent = { Text(appearanceSummary(appearance, dev.freethinkel.commutator.android.design.LocalDarkTheme.current)) },
                trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.outline) },
                modifier = Modifier.padding(horizontal = 16.dp),
            ) { Text("Appearance") }
        }
        section("Sessions")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                SegmentedListItem(
                    onClick = { model.setGroupByProject(!groupByProject) },
                    shapes = segmentedShapes(0, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Project) },
                    supportingContent = { Text("Recent sessions grouped by repository") },
                    trailingContent = { Switch(groupByProject, { model.setGroupByProject(it) }) },
                ) { Text("Group by project") }
                SegmentedListItem(
                    onClick = onArchived,
                    shapes = segmentedShapes(1, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Archive) },
                    supportingContent = { (workspace?.archived?.size ?: 0).takeIf { it > 0 }?.let { Text("$it sessions") } },
                    trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.outline) },
                ) { Text("Archived sessions") }
            }
        }
        if (devices.isNotEmpty()) {
            section("Devices")
            item {
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                    devices.forEachIndexed { i, device ->
                        SegmentedListItem(
                            onClick = {},
                            shapes = segmentedShapes(i, devices.size),
                            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                            leadingContent = {
                                IconTile(
                                    when {
                                        !device.isExecutionHost -> ZIcons.Phone
                                        device.platform == "linux" -> ZIcons.Server
                                        else -> ZIcons.Laptop
                                    },
                                )
                            },
                            supportingContent = {
                                Text(
                                    listOfNotNull(
                                        if (device.isSelf) "This device" else if (device.online) "Online" else "Offline",
                                        device.version?.let { "v$it" },
                                        if (device.sessionCount > 0u) "${device.sessionCount} sessions" else null,
                                    ).joinToString(" · "),
                                )
                            },
                            trailingContent = {
                                Box(
                                    Modifier.size(10.dp).clip(CircleShape).background(
                                        if (device.online || device.isSelf) successColor() else MaterialTheme.colorScheme.outlineVariant,
                                    ),
                                )
                            },
                        ) { Text(device.name) }
                    }
                }
            }
        }
        section("About")
        item {
            val update = AppUpdate.available
            val rows = if (update != null) 3 else 2
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                if (update != null) {
                    val progress = AppUpdate.progress
                    SegmentedListItem(
                        onClick = { scope.launch { AppUpdate.install(context) } },
                        shapes = segmentedShapes(0, rows),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(ZIcons.ArrowDown, MaterialTheme.colorScheme.primaryContainer, MaterialTheme.colorScheme.onPrimaryContainer) },
                        supportingContent = {
                            Text(
                                AppUpdate.error ?: progress?.let { "Downloading · ${(it * 100).toInt()}%" } ?: "Download and install",
                                color = if (AppUpdate.error != null) MaterialTheme.colorScheme.error else androidx.compose.ui.graphics.Color.Unspecified,
                            )
                        },
                    ) { Text("Update to ${update.name}") }
                }
                SegmentedListItem(
                    onClick = {},
                    shapes = segmentedShapes(rows - 2, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Info) },
                    supportingContent = { Text("Commutator for Android · core ${coreVersion()}") },
                ) { Text("Version") }
                SegmentedListItem(
                    onClick = { if (model.isDemo) model.signOut() else confirmSignOut = true },
                    shapes = segmentedShapes(rows - 1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Logout, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
                ) { Text(if (model.isDemo) "Leave demo" else "Sign out", color = MaterialTheme.colorScheme.error) }
            }
        }
    }
    StatusBarScrim(list)
    }
    if (confirmSignOut) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirmSignOut = false },
            icon = { ZIcon(ZIcons.Logout, null) },
            title = { Text("Sign out?") },
            text = { Text("Local drafts stay on this device.") },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    confirmSignOut = false
                    model.signOut()
                }) { Text("Sign out", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { confirmSignOut = false }) { Text("Cancel") } },
        )
    }
}

internal fun LazyListScope.section(title: String) {
    item {
        Text(
            title,
            style = MaterialTheme.typography.titleSmallEmphasized,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
        )
    }
}

/** A leading glyph on a tonal rounded tile. */
@Composable
fun IconTile(
    icon: Int,
    container: Color = MaterialTheme.colorScheme.secondaryContainer,
    content: Color = MaterialTheme.colorScheme.onSecondaryContainer,
) {
    Box(Modifier.size(40.dp).clip(RoundedCornerShape(14.dp)).background(container), contentAlignment = Alignment.Center) {
        ZIcon(icon, null, Modifier.size(22.dp), tint = content)
    }
}

/** Wallpaper: shown behind new sessions and the sessions list (iOS parity). */
@Composable
internal fun WallpaperSettings(model: AppModel) {
    val store = model.wallpaper
    val state by store.state.collectAsState()
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia(),
    ) { uri -> if (uri != null) scope.launch { store.set(uri, "Photo") } }
    var effects by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    val rows = if (state.set) 3 else 1
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = {
                picker.launch(androidx.activity.result.PickVisualMediaRequest(androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia.ImageOnly))
            },
            shapes = segmentedShapes(0, rows),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { IconTile(ZIcons.Image) },
            supportingContent = { Text(if (state.set) state.name ?: "Photo" else "Shown behind new sessions and the sessions list") },
        ) { Text(if (state.set) "Change wallpaper" else "Choose wallpaper") }
        if (state.set) {
            Box {
                SegmentedListItem(
                    onClick = { effects = true },
                    shapes = segmentedShapes(1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("${dev.freethinkel.commutator.android.core.WallpaperStore.label(state.effect)} — ${dev.freethinkel.commutator.android.core.WallpaperStore.detail(state.effect)}") },
                ) { Text("Effect") }
                ChoiceMenu(effects, { effects = false }, listOf(MenuSection("Effect", dev.freethinkel.commutator.android.core.WallpaperStore.effects.map { e ->
                    MenuChoice(dev.freethinkel.commutator.android.core.WallpaperStore.label(e), e == state.effect) { store.setEffect(e) }
                })))
            }
            SegmentedListItem(
                onClick = { store.remove() },
                shapes = segmentedShapes(2, rows),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Delete, MaterialTheme.colorScheme.errorContainer, MaterialTheme.colorScheme.onErrorContainer) },
            ) { Text("Remove wallpaper", color = MaterialTheme.colorScheme.error) }
        }
    }
}
