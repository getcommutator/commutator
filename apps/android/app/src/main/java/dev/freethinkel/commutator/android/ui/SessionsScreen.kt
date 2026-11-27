package dev.freethinkel.commutator.android.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.clickable
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.ui.focus.focusRequester
import dev.freethinkel.commutator.android.core.Checkout
import dev.freethinkel.commutator.android.core.ChatStatus
import dev.freethinkel.commutator.android.core.ProjectTree
import dev.freethinkel.commutator.android.core.activityGroups
import dev.freethinkel.commutator.android.core.projectByChat
import dev.freethinkel.commutator.android.core.status
import dev.freethinkel.commutator.android.core.urgentStatus
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.design.HarnessMark
import dev.freethinkel.commutator.android.design.LocalDarkTheme
import dev.freethinkel.commutator.android.design.ProjectColors
import dev.freethinkel.commutator.android.design.ZIcon
import dev.freethinkel.commutator.android.design.ZIcons
import uniffi.commutator_core.ChatIndicator
import uniffi.commutator_core.ConnectivityState
import uniffi.commutator_core.PullRequestState
import uniffi.commutator_core.SessionRow
import uniffi.commutator_core.WorkspaceSnapshot

private val SEARCH_BAR_HEIGHT = 56.dp

/**
 * The Material 3 search bar on top of the list, as Gmail has it: the list's
 * options behind the leading menu, "Search sessions" opening search, and the
 * account's avatar opening Settings.
 */
@Composable
private fun SessionsSearchBar(
    onSearch: () -> Unit,
    onSettings: () -> Unit,
    account: String,
    actions: @Composable RowScope.() -> Unit = {},
    menu: @Composable (open: Boolean, dismiss: () -> Unit) -> Unit,
    modifier: Modifier = Modifier,
) {
    var open by remember { mutableStateOf(false) }
    Surface(
        onClick = onSearch,
        shape = CircleShape,
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        modifier = modifier
            .statusBarsPadding()
            .padding(start = 16.dp, end = 16.dp, top = 8.dp)
            .fillMaxWidth()
            .height(SEARCH_BAR_HEIGHT)
            .semantics { contentDescription = "Search sessions" },
    ) {
        Row(Modifier.padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Box {
                IconButton(onClick = { open = true }) { ZIcon(ZIcons.Menu, "Session options", Modifier.size(24.dp)) }
                menu(open) { open = false }
            }
            Text(
                "Search sessions",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                modifier = Modifier.weight(1f).padding(start = 4.dp),
            )
            actions()
            // The account, Settings behind it (Google apps' avatar).
            Box(
                Modifier
                    .padding(start = 4.dp, end = 8.dp)
                    .size(32.dp)
                    .clip(CircleShape)
                    .background(MaterialTheme.colorScheme.primary)
                    .clickable(onClickLabel = "Settings", onClick = onSettings)
                    .semantics { contentDescription = "Settings, $account" },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    account.trim().take(1).uppercase().ifEmpty { "Z" },
                    style = MaterialTheme.typography.titleSmall,
                    color = MaterialTheme.colorScheme.onPrimary,
                )
            }
        }
    }
}

private enum class Filter(val label: String) { All("All"), NeedsYou("Needs you"), Working("Working"), Pinned("Pinned") }

/** "2 working · 1 needs you" — the live summary beside New session. */
fun liveSummary(ws: WorkspaceSnapshot): String? {
    val (working, awaiting) = liveCounts(ws)
    val parts = listOfNotNull(
        if (working > 0) "$working working" else null,
        if (awaiting > 0) "$awaiting need${if (awaiting == 1) "s" else ""} you" else null,
    )
    return parts.joinToString(" · ").ifEmpty { null }
}

private fun frontRows(ws: WorkspaceSnapshot): List<SessionRow> {
    val seen = HashSet<String>()
    return (ws.front.pinned + ws.front.recent).filter { seen.add(it.id) }
}

private fun liveCounts(ws: WorkspaceSnapshot): Pair<Int, Int> {
    val rows = frontRows(ws)
    return rows.count { it.indicator == ChatIndicator.WORKING || it.indicator == ChatIndicator.WAITING } to rows.count { it.indicator == ChatIndicator.AWAITING_INPUT }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SessionsScreen(
    model: AppModel,
    onOpen: (String) -> Unit,
    onArchived: () -> Unit,
    onPullRequests: () -> Unit = {},
    selected: String? = null,
    onSearch: () -> Unit = {},
    onSettings: () -> Unit = {},
    onNew: () -> Unit = {},
    onOpenCheckout: (ProjectTree, Checkout) -> Unit = { _, _ -> },
) {
    val workspace by model.workspace.collectAsState()
    val trees by model.projects.collectAsState()
    val projectsView by model.projectsView.collectAsState()
    val activityProject by model.activityProject.collectAsState()
    val compact by model.compactRows.collectAsState()
    var picker by remember { mutableStateOf<String?>(null) }
    var newProject by remember { mutableStateOf(false) }
    val owner = remember(trees) { projectByChat(trees) }
    // A filter on a project that's gone shows everything.
    val filterTree = trees.firstOrNull { it.id == activityProject }
    val keep: (SessionRow) -> Boolean = { row -> filterTree == null || owner[row.id]?.id == filterTree.id }
    val connectivity by model.connectivity.collectAsState()
    val groupByProject by model.groupByProject.collectAsState()
    val collapsedGroups by model.collapsed.collectAsState()
    var filter by rememberSaveable { mutableStateOf(Filter.All) }
    val scope = rememberCoroutineScope()
    var reorder by remember { mutableStateOf(false) }
    var refreshing by remember { mutableStateOf(false) }
    val pull = rememberPullToRefreshState()
    val ws = workspace
    val counts = remember(ws) { ws?.let { liveCounts(it) } ?: (0 to 0) }

    val subtitle = when {
        connectivity?.state == ConnectivityState.OFFLINE -> "Offline"
        connectivity?.state == ConnectivityState.RECONNECTING -> "Reconnecting…"
        model.isDemo -> "Demo workspace"
        // Who's signed in belongs in Settings, not over the list.
        else -> null
    }

    // Remembered: every row takes `actions`, so a fresh instance per
    // recomposition re-composes every visible row.
    val actions = remember(model, onOpen, selected, compact) {
        RowActions(
            model = model,
            onOpen = onOpen,
            archive = { row -> model.archive(row.id) },
            selected = selected,
            compact = compact,
        )
    }
    // The Projects view's Chats card: compact rows, no Pin.
    val projectActions = remember(actions) {
        RowActions(actions.model, actions.onOpen, actions.archive, actions.selected, activity = false, compact = true)
    }

    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    val density = androidx.compose.ui.platform.LocalDensity.current
    // The search bar slides away while the list scrolls down and comes back on the way up (Gmail's).
    val barSpace = with(density) { (SEARCH_BAR_HEIGHT + 16.dp).toPx() + WindowInsets.statusBars.asPaddingValues().calculateTopPadding().toPx() }
    var barHidden by remember { mutableFloatStateOf(0f) }
    val barScroll = remember(barSpace) {
        object : androidx.compose.ui.input.nestedscroll.NestedScrollConnection {
            override fun onPreScroll(available: androidx.compose.ui.geometry.Offset, source: androidx.compose.ui.input.nestedscroll.NestedScrollSource): androidx.compose.ui.geometry.Offset {
                barHidden = (barHidden - available.y).coerceIn(0f, barSpace)
                return androidx.compose.ui.geometry.Offset.Zero
            }
        }
    }
    // Back at the top it's always there.
    val atTop by remember { derivedStateOf { list.firstVisibleItemIndex == 0 && list.firstVisibleItemScrollOffset < 8 } }
    LaunchedEffect(atTop) { if (atTop) barHidden = 0f }
    Box(Modifier.fillMaxSize().nestedScroll(barScroll)) {
        dev.freethinkel.commutator.android.design.WallpaperHero(
            model.wallpaper,
            scrollFade = {
                if (list.firstVisibleItemIndex > 0) 0f
                else 1f - list.firstVisibleItemScrollOffset / (with(density) { 400.dp.toPx() })
            },
        )
        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                refreshing = true
                scope.launch {
                    model.refresh()
                    refreshing = false
                }
            },
            state = pull,
            modifier = Modifier.fillMaxSize(),
            indicator = {
                PullToRefreshDefaults.LoadingIndicator(
                    state = pull,
                    isRefreshing = refreshing,
                    modifier = Modifier.align(Alignment.TopCenter).padding(WindowInsets.statusBars.asPaddingValues()),
                )
            },
        ) {
            LazyColumn(
                state = list,
                contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + SEARCH_BAR_HEIGHT + 16.dp, bottom = 112.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                // Only when there's something to say: offline, reconnecting, the demo.
                if (subtitle != null) {
                    item("status") {
                        val offline = connectivity?.state == ConnectivityState.OFFLINE
                        Row(
                            Modifier
                                .padding(start = 28.dp, end = 16.dp, top = 4.dp)
                                .then(if (offline) Modifier.clickable(onClickLabel = "Try again") { scope.launch { model.refresh() } } else Modifier),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            if (offline) {
                                ZIcon(ZIcons.Offline, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.error)
                                Spacer(Modifier.width(6.dp))
                            }
                            Text(
                                if (offline) "$subtitle · Tap to retry" else subtitle,
                                style = MaterialTheme.typography.labelLarge,
                                color = if (offline) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
                if (projectsView) {
                    if (ws != null) {
                        projectsContent(
                            model, trees, frontRows(ws), collapsedGroups, projectActions, scope,
                            onOpenCheckout = onOpenCheckout, onNew = onNew, onNewProject = { newProject = true },
                        )
                    }
                    return@LazyColumn
                }
                item("filters") {
                    Row(
                        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        for (f in Filter.entries) {
                            Pill(
                                f.label,
                                filter == f,
                                onClick = { filter = f },
                                count = when (f) {
                                    Filter.NeedsYou -> counts.second
                                    Filter.Working -> counts.first
                                    else -> null
                                },
                            )
                        }
                    }
                }
                // Before the registry first arrives an empty list means
                // "not loaded yet", not "no chats".
                if (ws == null || !ws.synced) {
                    item("loading") {
                        Box(Modifier.fillMaxWidth().padding(48.dp), contentAlignment = Alignment.Center) { LoadingIndicator() }
                    }
                    return@LazyColumn
                }
                val front = ws.front
                val pinned = front.pinned.filter(keep)
                val recent = front.recent.filter(keep)
                val all = frontRows(ws).filter(keep)
                when {
                    filter == Filter.All && groupByProject -> {
                        // One list per project, Pinned and all, no shelves.
                        for ((tree, rows) in activityGroups(all, owner)) {
                            val id = "activity.project." + (tree?.id ?: "none")
                            group(
                                id, tree?.name ?: "No Project", rows, actions,
                                collapsed = id in collapsedGroups,
                                onToggle = { model.setGroupCollapsed(id, id !in collapsedGroups) },
                                icon = if (tree != null) ({ ProjectIcon(tree, 18.dp) }) else null,
                            )
                        }
                    }
                    filter == Filter.All -> {
                        if (pinned.isNotEmpty()) {
                            group(
                                "pinned", "Pinned", pinned, actions,
                                collapsed = "pinned" in collapsedGroups,
                                onToggle = { model.setGroupCollapsed("pinned", "pinned" !in collapsedGroups) },
                                menu = listOf(MenuAction("Reorder…", ZIcons.Sort) { reorder = true }),
                            )
                        }
                        if (recent.isNotEmpty()) {
                            group(
                                "recent", "Recent", recent, actions,
                                collapsed = "recent" in collapsedGroups,
                                onToggle = { model.setGroupCollapsed("recent", "recent" !in collapsedGroups) },
                            )
                        }
                    }
                    filter == Filter.NeedsYou -> group("f", null, all.filter { it.indicator == ChatIndicator.AWAITING_INPUT }, actions)
                    filter == Filter.Working -> group("f", null, all.filter { it.indicator == ChatIndicator.WORKING || it.indicator == ChatIndicator.WAITING }, actions)
                    else -> group("f", null, pinned, actions)
                }
                val empty = when (filter) {
                    Filter.All -> all.isEmpty()
                    Filter.NeedsYou -> all.none { it.indicator == ChatIndicator.AWAITING_INPUT }
                    Filter.Working -> all.none { it.indicator == ChatIndicator.WORKING || it.indicator == ChatIndicator.WAITING }
                    Filter.Pinned -> pinned.isEmpty()
                }
                if (empty) item("empty") { EmptyState(filter) }
            }
        }
        StatusBarScrim(list)
        SessionsSearchBar(
            onSearch = onSearch,
            onSettings = onSettings,
            account = model.accountName,
            actions = { IconButton(onClick = onPullRequests) { ZIcon(ZIcons.PullRequest, "Pull requests", Modifier.size(24.dp)) } },
            menu = { open, dismiss ->
                val views = listOf(
                    MenuAction("Chats", ZIcons.Chat, checked = !projectsView) { model.setProjectsView(false) },
                    MenuAction("Projects", ZIcons.Folder, checked = projectsView) { model.setProjectsView(true) },
                )
                val chats = if (projectsView) emptyList() else listOf(
                    MenuAction("Project: ${filterTree?.name ?: "All Projects"}", ZIcons.Project) { picker = "project" },
                    MenuAction("Group by: ${if (groupByProject) "Project" else "None"}", ZIcons.FolderTree) { picker = "group" },
                    MenuAction("Compact", ZIcons.RowsCompress, checked = compact) { model.setCompactRows(!compact) },
                )
                val more = listOfNotNull(
                    if (projectsView) null else MenuAction("Collapse all", ZIcons.Collapse) {
                        if (ws == null) return@MenuAction
                        val local = listOf("pinned", "recent") +
                            activityGroups(frontRows(ws), owner).map { "activity.project." + (it.first?.id ?: "none") }
                        local.forEach { model.setGroupCollapsed(it, true) }
                    },
                    MenuAction("Archived", ZIcons.Archive, onClick = onArchived),
                )
                GroupedActionMenu(open, dismiss, listOf(views, chats, more))
                ChoiceMenu(
                    picker == "project", { picker = null },
                    listOf(
                        MenuSection("Project", listOf(
                            MenuChoice("All Projects", filterTree == null, leading = { ZIcon(ZIcons.Companions, null, Modifier.size(20.dp)) }) { model.setActivityProject(null) },
                        ) + trees.map { t ->
                            MenuChoice(t.name, filterTree?.id == t.id, leading = { ProjectIcon(t, 20.dp) }) { model.setActivityProject(t.id) }
                        }),
                    ),
                )
                ChoiceMenu(
                    picker == "group", { picker = null },
                    listOf(
                        MenuSection("Group by", listOf(
                            MenuChoice("None", !groupByProject, leading = { ZIcon(ZIcons.Sort, null, Modifier.size(20.dp)) }) { model.setGroupByProject(false) },
                            MenuChoice("Project", groupByProject, leading = { ZIcon(ZIcons.Folder, null, Modifier.size(20.dp)) }) { model.setGroupByProject(true) },
                        )),
                    ),
                )
            },
            modifier = Modifier
                .align(Alignment.TopCenter)
                .graphicsLayer { translationY = -barHidden },
        )
    }

    if (reorder && ws != null) ReorderPinsSheet(model, ws.front.pinned) { reorder = false }
    if (newProject) NewProjectScreen(model, onDismiss = { newProject = false }) { newProject = false }
}

/** What a session row can do, shared by every group on the page. */
class RowActions(
    val model: AppModel,
    val onOpen: (String) -> Unit,
    val archive: (SessionRow) -> Unit,
    /** The chat open beside the list (large screens). */
    val selected: String? = null,
    /** Pin (menu and swipe): the Chats view's, off in a project's chats. */
    val activity: Boolean = true,
    /** One-line rows (the desktop sidebar's Compact). */
    val compact: Boolean = false,
)

internal fun LazyListScope.group(
    id: String,
    title: String?,
    rows: List<SessionRow>,
    actions: RowActions,
    collapsed: Boolean = false,
    onToggle: (() -> Unit)? = null,
    menu: List<MenuAction> = emptyList(),
    /** A project group's artwork (or monogram), before the title. */
    icon: (@Composable () -> Unit)? = null,
) {
    if (title != null) {
        item("h-$id") {
            var open by remember { mutableStateOf(false) }
            Box(Modifier.animateItem()) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .combinedClickable(
                            onClick = { onToggle?.invoke() },
                            onLongClick = if (menu.isNotEmpty()) ({ open = true }) else null,
                        )
                        .padding(start = 28.dp, end = 12.dp, top = 20.dp, bottom = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                        if (icon != null) {
                            icon()
                            Spacer(Modifier.width(8.dp))
                        }
                        Text(title, style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.primary, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                        Spacer(Modifier.width(8.dp))
                        Text("${rows.size}", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        urgentStatus(rows)?.let {
                            Spacer(Modifier.width(6.dp))
                            StatusGlyph(it, 12.dp)
                        }
                    }
                    if (menu.isNotEmpty()) {
                        IconButton(onClick = { open = true }, modifier = Modifier.size(32.dp)) {
                            ZIcon(ZIcons.More, "$title options", Modifier.size(18.dp))
                        }
                    }
                    if (onToggle != null) {
                        IconButton(onClick = onToggle, modifier = Modifier.size(32.dp)) {
                            ZIcon(if (collapsed) ZIcons.ChevronDown else ZIcons.ChevronUp, if (collapsed) "Expand" else "Collapse", Modifier.size(18.dp))
                        }
                    }
                }
                Box(Modifier.align(Alignment.TopEnd).padding(end = 12.dp, top = 20.dp)) { ActionMenu(open, { open = false }, menu) }
            }
        }
    } else {
        item("h-$id") { Spacer(Modifier.height(8.dp)) }
    }
    if (collapsed) return
    itemsIndexed(rows, key = { _, r -> "$id/${r.id}" }) { i, row ->
        Box(Modifier.animateItem().padding(horizontal = 16.dp).padding(bottom = ListItemDefaults.SegmentedGap)) {
            SwipeableSessionRow(row, i, rows.size, actions)
        }
    }
}

/** Swipe right to pin (or unpin), left to archive. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SwipeableSessionRow(row: SessionRow, index: Int, count: Int, actions: RowActions) {
    val state = rememberSwipeToDismissBoxState()
    val scope = rememberCoroutineScope()
    val shape = segmentedShapes(index, count).shape
    SwipeToDismissBox(
        state = state,
        enableDismissFromStartToEnd = actions.activity,
        onDismiss = {
            when (it) {
                SwipeToDismissBoxValue.EndToStart -> actions.archive(row)
                SwipeToDismissBoxValue.StartToEnd -> {
                    actions.model.setPinned(row.id, !row.pinned)
                    scope.launch { state.reset() }
                }
                else -> Unit
            }
        },
        backgroundContent = {
            val pinning = state.dismissDirection == SwipeToDismissBoxValue.StartToEnd
            Surface(
                Modifier.fillMaxSize(),
                shape = shape,
                color = if (pinning) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.tertiaryContainer,
            ) {
                Box(
                    Modifier.padding(horizontal = 28.dp),
                    contentAlignment = if (pinning) Alignment.CenterStart else Alignment.CenterEnd,
                ) {
                    if (pinning) {
                        ZIcon(ZIcons.Pin, if (row.pinned) "Unpin" else "Pin", tint = MaterialTheme.colorScheme.onSecondaryContainer)
                    } else {
                        ZIcon(ZIcons.Archive, "Archive", tint = MaterialTheme.colorScheme.onTertiaryContainer)
                    }
                }
            }
        },
    ) {
        SessionItem(row, index, count, actions)
    }
}

/** The list card colour: paper-white in light, raised in dark. */
@Composable
fun cardColor() = if (LocalDarkTheme.current) MaterialTheme.colorScheme.surfaceContainer else MaterialTheme.colorScheme.surfaceContainerLowest

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SessionItem(row: SessionRow, index: Int, count: Int, actions: RowActions) {
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var deleting by remember { mutableStateOf(false) }
    val haptics = LocalHapticFeedback.current
    val model = actions.model
    Box {
        if (actions.compact) {
            CompactSessionItem(row, index, count, actions) {
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                menu = true
            }
        } else SegmentedListItem(
            onClick = { actions.onOpen(row.id) },
            onLongClick = {
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                menu = true
            },
            shapes = segmentedShapes(index, count),
            colors = ListItemDefaults.segmentedColors(
                containerColor = if (row.id == actions.selected) MaterialTheme.colorScheme.secondaryContainer else cardColor(),
            ),
            leadingContent = { HarnessTile(row) },
            supportingContent = { Subline(row) },
            trailingContent = {
                Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    StatusLabel(row)
                    row.pullRequest?.let { PrBadge(it.number, it.state) }
                }
            },
        ) {
            Text(
                row.title,
                style = if (row.unseen) MaterialTheme.typography.titleMediumEmphasized else MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        ActionMenu(
            menu,
            { menu = false },
            (if (actions.activity) {
                listOf(
                    MenuAction(if (row.pinned) "Unpin" else "Pin", ZIcons.Pin) { model.setPinned(row.id, !row.pinned) },
                )
            } else {
                emptyList()
            }) + chatMenu(row, actions, rename = { renaming = true }, delete = { deleting = true }),
        )
    }
    if (renaming) RenameDialog(row.title, onDismiss = { renaming = false }) { model.rename(row.id, it) }
    if (deleting) ConfirmDeleteChat(model, row.id) { deleting = false }
}

/** Compact: the title line alone, 44 dp (the Projects page's rows). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun CompactSessionItem(row: SessionRow, index: Int, count: Int, actions: RowActions, onLongClick: () -> Unit) {
    Surface(
        shape = segmentedShapes(index, count).shape,
        color = if (row.id == actions.selected) MaterialTheme.colorScheme.secondaryContainer else cardColor(),
        modifier = Modifier.fillMaxWidth().height(44.dp),
    ) {
        Row(
            Modifier.fillMaxSize().combinedClickable(onClick = { actions.onOpen(row.id) }, onLongClick = onLongClick).padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(26.dp), contentAlignment = Alignment.Center) { HarnessMark(row.harness, 20.dp, tint = MaterialTheme.colorScheme.onSurface) }
            Spacer(Modifier.width(10.dp))
            Text(row.title, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            val status = row.status()
            if (status == ChatStatus.Idle) {
                Text(row.timeLabel, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.outline)
            } else {
                StatusGlyph(status)
            }
        }
    }
}

/** The harness mark on a tile toned by the session's project. */
@Composable
fun HarnessTile(row: SessionRow) {
    val tone = ProjectColors.color(row.colorIndex(), LocalDarkTheme.current)
    Box(
        Modifier.size(48.dp).clip(RoundedCornerShape(16.dp)).background(tone.copy(alpha = if (LocalDarkTheme.current) 0.18f else 0.12f)),
        contentAlignment = Alignment.Center,
    ) { HarnessMark(row.harness, 24.dp, tint = MaterialTheme.colorScheme.onSurface) }
}

/** Project monogram + name, then the branch — the desktop sidebar subline. */
@Composable
private fun Subline(row: SessionRow) {
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    Row(verticalAlignment = Alignment.CenterVertically) {
        val project = row.project
        if (project != null) {
            ChatProjectTile(project, 16.dp)
            Spacer(Modifier.width(6.dp))
        }
        Text(
            project?.name ?: row.deviceName ?: "No project",
            style = MaterialTheme.typography.bodyMedium,
            color = muted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        row.branch?.takeIf { it.isNotEmpty() }?.let { branch ->
            Spacer(Modifier.width(10.dp))
            ZIcon(ZIcons.Branch, null, Modifier.size(14.dp), tint = muted.copy(alpha = 0.7f))
            Spacer(Modifier.width(3.dp))
            Text(branch, style = MaterialTheme.typography.bodyMedium, color = muted.copy(alpha = 0.7f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
fun PrBadge(number: ULong, state: PullRequestState) {
    val tone = when (state) {
        PullRequestState.OPEN -> successColor()
        PullRequestState.MERGED -> MaterialTheme.colorScheme.primary
        PullRequestState.CLOSED -> MaterialTheme.colorScheme.error
    }
    Surface(shape = RoundedCornerShape(8.dp), color = tone.copy(alpha = 0.12f), contentColor = tone) {
        Row(Modifier.padding(horizontal = 6.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            ZIcon(ZIcons.PullRequest, null, Modifier.size(13.dp))
            Spacer(Modifier.width(3.dp))
            Text("$number", style = MaterialTheme.typography.labelMediumEmphasized)
        }
    }
}

@Composable
fun RenameDialog(current: String, onDismiss: () -> Unit, onRename: (String) -> Unit) =
    TextPromptDialog("Rename session", current, "Rename", ZIcons.Rename, onDismiss, onRename)

/** A one-field dialog (rename a session). */
@Composable
fun TextPromptDialog(title: String, current: String, confirm: String, icon: Int, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var text by remember { mutableStateOf(current) }
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    androidx.compose.runtime.LaunchedEffect(Unit) { focus.requestFocus() }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(icon, null) },
        title = { Text(title) },
        text = {
            OutlinedTextField(
                text, { text = it },
                singleLine = true,
                shape = RoundedCornerShape(16.dp),
                modifier = Modifier.focusRequester(focus),
            )
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(text.trim()); onDismiss() }, enabled = text.isNotBlank()) { Text(confirm) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Pins are an ordered list (synced to the desktop): move them up and down. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ReorderPinsSheet(model: AppModel, pinned: List<SessionRow>, onDismiss: () -> Unit) {
    val sheet = androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true)
    androidx.compose.material3.ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        Text("Reorder pinned", style = MaterialTheme.typography.headlineSmallEmphasized, modifier = Modifier.padding(start = 24.dp, bottom = 16.dp))
        LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp)) {
            itemsIndexed(pinned, key = { _, r -> r.id }) { i, row ->
                SegmentedListItem(
                    onClick = {},
                    shapes = segmentedShapes(i, pinned.size),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { HarnessTile(row) },
                    trailingContent = {
                        Row {
                            IconButton(onClick = { model.movePin(row.id, pinned.getOrNull(i - 2)?.id, pinned[i - 1].id) }, enabled = i > 0) {
                                ZIcon(ZIcons.ChevronUp, "Move up", Modifier.size(20.dp))
                            }
                            IconButton(onClick = { model.movePin(row.id, pinned[i + 1].id, pinned.getOrNull(i + 2)?.id) }, enabled = i < pinned.size - 1) {
                                ZIcon(ZIcons.ChevronDown, "Move down", Modifier.size(20.dp))
                            }
                        }
                    },
                    modifier = Modifier.animateItem().padding(bottom = ListItemDefaults.SegmentedGap),
                ) { Text(row.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            }
        }
    }
}

@Composable
private fun EmptyState(filter: Filter) {
    Column(Modifier.fillMaxWidth().padding(48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        ZIcon(ZIcons.Chat, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.outline)
        Spacer(Modifier.height(12.dp))
        Text(
            when (filter) {
                Filter.All -> "No sessions yet"
                Filter.NeedsYou -> "Nothing needs you"
                Filter.Working -> "No agents working"
                Filter.Pinned -> "Nothing pinned"
            },
            style = MaterialTheme.typography.titleLarge,
        )
    }
}
