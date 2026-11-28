package dev.freethinkel.commutator.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.RadioButton
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ToggleButtonDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.design.Appearance
import dev.freethinkel.commutator.android.design.LocalDarkTheme
import dev.freethinkel.commutator.android.design.ThemeMode
import dev.freethinkel.commutator.android.design.ZIcon
import dev.freethinkel.commutator.android.design.ZIcons
import uniffi.commutator_core.ThemeFamilyInfo
import uniffi.commutator_core.ThemeVariantInfo
import uniffi.commutator_core.themeFamilies

/** The built-in themes (desktop order), resolved once. */
private val families: List<ThemeFamilyInfo> by lazy { themeFamilies() }

private fun variant(id: String): ThemeVariantInfo? = families.flatMap { it.variants }.firstOrNull { it.id == id }

/** "Dark · Dracula": the mode and the family of the theme on screen now (the Settings row). */
fun appearanceSummary(appearance: Appearance, dark: Boolean): String {
    val id = if (dark) appearance.darkTheme else appearance.lightTheme
    val family = families.firstOrNull { f -> f.variants.any { it.id == id } }?.name ?: "Commutator"
    return "${appearance.mode.name} · $family"
}

/** Settings → Appearance: light/dark mode, the light and the dark theme. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun AppearanceScreen(model: AppModel, onBack: () -> Unit) {
    val appearance by model.appearance.collectAsState()
    // null: no picker; true: the dark theme's; false: the light theme's.
    var picking by remember { mutableStateOf<Boolean?>(null) }
    LazyColumn(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background),
        contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 48.dp),
    ) {
        item {
            Box(Modifier.padding(start = 12.dp, top = 4.dp)) {
                TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            }
        }
        item { ScreenHeader("Appearance", null) }
        section("Mode")
        item {
            Surface(Modifier.padding(horizontal = 16.dp), shape = segmentedShapes(0, 1).shape, color = cardColor()) {
                Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
                    val modes = listOf(
                        Triple(ThemeMode.System, "System", ZIcons.Monitor),
                        Triple(ThemeMode.Light, "Light", ZIcons.Sun),
                        Triple(ThemeMode.Dark, "Dark", ZIcons.Moon),
                    )
                    modes.forEachIndexed { index, (mode, label, icon) ->
                        ToggleButton(
                            checked = appearance.mode == mode,
                            onCheckedChange = { model.setAppearance(appearance.copy(mode = mode)) },
                            modifier = Modifier.weight(1f).semantics { role = Role.RadioButton },
                            shapes = when (index) {
                                0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                                modes.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                                else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                            },
                        ) {
                            ZIcon(icon, null, Modifier.size(ToggleButtonDefaults.IconSize))
                            Spacer(Modifier.size(ToggleButtonDefaults.IconSpacing))
                            Text(label)
                        }
                    }
                }
            }
        }
        section("Theme")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                listOf(false to appearance.lightTheme, true to appearance.darkTheme).forEachIndexed { index, (dark, id) ->
                    val v = variant(id)
                    SegmentedListItem(
                        onClick = { picking = dark },
                        shapes = segmentedShapes(index, 3),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(if (dark) ZIcons.Moon else ZIcons.Sun) },
                        supportingContent = { Text(v?.name ?: id) },
                        trailingContent = { v?.let { Swatches(it.swatches) } },
                    ) { Text(if (dark) "Dark theme" else "Light theme") }
                }
                SegmentedListItem(
                    onClick = { model.setAppearance(appearance.copy(dynamicColor = !appearance.dynamicColor)) },
                    shapes = segmentedShapes(2, 3),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Magic) },
                    supportingContent = { Text("Tint Commutator with your wallpaper's palette") },
                    trailingContent = { Switch(appearance.dynamicColor, { model.setAppearance(appearance.copy(dynamicColor = it)) }) },
                ) { Text("Wallpaper colors") }
            }
        }
    }
    picking?.let { dark ->
        ThemePicker(
            dark = dark,
            current = if (dark) appearance.darkTheme else appearance.lightTheme,
            onPick = { id -> model.setAppearance(if (dark) appearance.copy(darkTheme = id) else appearance.copy(lightTheme = id)) },
            onDismiss = { picking = null },
        )
    }
}

/** The built-in themes of one appearance, by family, with their colours. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ThemePicker(dark: Boolean, current: String, onPick: (String) -> Unit, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        LazyColumn(contentPadding = PaddingValues(bottom = 32.dp)) {
            item {
                Text(
                    if (dark) "Dark theme" else "Light theme",
                    style = MaterialTheme.typography.titleLarge,
                    modifier = Modifier.padding(start = 24.dp, bottom = 8.dp),
                )
            }
            for (family in families) {
                val variants = family.variants.filter { it.dark == dark }
                if (variants.isEmpty()) continue
                item {
                    Text(
                        family.name,
                        style = MaterialTheme.typography.titleSmall,
                        color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.padding(start = 24.dp, top = 16.dp, bottom = 4.dp),
                    )
                }
                items(variants, key = { it.id }) { v ->
                    Row(
                        Modifier.fillMaxWidth().clickable { onPick(v.id) }.padding(horizontal = 16.dp, vertical = 4.dp),
                        verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
                    ) {
                        RadioButton(selected = v.id == current, onClick = { onPick(v.id) })
                        Text(v.name, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                        Swatches(v.swatches)
                    }
                }
            }
        }
    }
}

/** A theme's background, text, accent and success as overlapping dots. */
@Composable
private fun Swatches(colors: List<UInt>) {
    val ring = if (LocalDarkTheme.current) Color.White.copy(alpha = 0.25f) else Color.Black.copy(alpha = 0.2f)
    Box(Modifier.size(width = (16 + 10 * (colors.size - 1)).dp, height = 16.dp)) {
        colors.forEachIndexed { i, argb ->
            Box(
                Modifier.offset(x = (10 * i).dp).size(16.dp).clip(CircleShape)
                    .background(Color(argb.toInt())).border(1.dp, ring, CircleShape),
            )
        }
    }
}
