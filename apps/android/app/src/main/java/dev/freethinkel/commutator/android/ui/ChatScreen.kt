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
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.core.MathSettings
import dev.freethinkel.commutator.android.core.WallpaperStore
import dev.freethinkel.commutator.android.design.ZIcons
import kotlinx.coroutines.launch

/** The Settings row's summary: "Fira Math · 100%", or that math shows as source. */
fun chatSummary(math: MathSettings.State): String =
    if (math.render) "${MathSettings.label(math.face)} · ${math.size}%" else "Math as LaTeX source"

/** Settings › Chat: how transcripts show math, and the wallpaper behind them. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ChatScreen(model: AppModel, onBack: () -> Unit) {
    LazyColumn(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background),
        contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 48.dp),
    ) {
        item {
            Box(Modifier.padding(start = 12.dp, top = 4.dp)) {
                TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            }
        }
        item { ScreenHeader("Chat", null) }
        section("Math")
        item { MathSettingsRows(model.math) }
        section("Wallpaper")
        item { WallpaperSettings(model) }
    }
}

/** "Render Math" and, while it's on, the face and size pickers (iOS's rows). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun MathSettingsRows(store: MathSettings) {
    val math by store.state.collectAsState()
    var fonts by remember { mutableStateOf(false) }
    var sizes by remember { mutableStateOf(false) }
    val rows = if (math.render) 3 else 1
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
        SegmentedListItem(
            onClick = { store.setRender(!math.render) },
            shapes = segmentedShapes(0, rows),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { IconTile(ZIcons.Text) },
            supportingContent = { Text("Off shows LaTeX as its source") },
            trailingContent = { Switch(math.render, { store.setRender(it) }) },
        ) { Text("Render Math") }
        if (math.render) {
            Box {
                SegmentedListItem(
                    onClick = { fonts = true },
                    shapes = segmentedShapes(1, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Rename) },
                    supportingContent = { Text("${MathSettings.label(math.face)} — ${MathSettings.detail(math.face)}") },
                ) { Text("Math Font") }
                ChoiceMenu(fonts, { fonts = false }, listOf(MenuSection("Math Font", MathSettings.faces.map { f ->
                    MenuChoice(MathSettings.label(f), f == math.face, supporting = MathSettings.detail(f)) { store.setFace(f) }
                })))
            }
            Box {
                SegmentedListItem(
                    onClick = { sizes = true },
                    shapes = segmentedShapes(2, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.RowsExpand) },
                    supportingContent = { Text("${math.size}% of the text around it") },
                ) { Text("Math Size") }
                ChoiceMenu(sizes, { sizes = false }, listOf(MenuSection("Math Size", MathSettings.sizes.map { size ->
                    MenuChoice("$size%", size == math.size) { store.setSize(size) }
                })))
            }
        }
    }
}

/** Wallpaper: shown behind new sessions and the sessions list (iOS parity). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
internal fun WallpaperSettings(model: AppModel) {
    val store = model.wallpaper
    val state by store.state.collectAsState()
    val scope = rememberCoroutineScope()
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(
        androidx.activity.result.contract.ActivityResultContracts.PickVisualMedia(),
    ) { uri -> if (uri != null) scope.launch { store.set(uri, "Photo") } }
    var effects by remember { mutableStateOf(false) }
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
                    supportingContent = { Text("${WallpaperStore.label(state.effect)} — ${WallpaperStore.detail(state.effect)}") },
                ) { Text("Effect") }
                ChoiceMenu(effects, { effects = false }, listOf(MenuSection("Effect", WallpaperStore.effects.map { e ->
                    MenuChoice(WallpaperStore.label(e), e == state.effect) { store.setEffect(e) }
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
