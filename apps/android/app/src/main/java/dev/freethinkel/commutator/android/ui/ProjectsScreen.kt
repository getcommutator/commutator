package dev.freethinkel.commutator.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.core.ChatStatus
import dev.freethinkel.commutator.android.core.Checkout
import dev.freethinkel.commutator.android.core.ProjectArt
import dev.freethinkel.commutator.android.core.ProjectTree
import dev.freethinkel.commutator.android.core.needsYouFirst
import dev.freethinkel.commutator.android.core.status
import dev.freethinkel.commutator.android.core.urgentStatus
import dev.freethinkel.commutator.android.design.ZIcon
import dev.freethinkel.commutator.android.design.ZIcons
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.commutator_core.SessionRow

/** A status at the end of a row: the question, done check, alarm, spinner, failure dot; null, a quiet dot. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun StatusGlyph(status: ChatStatus?, size: Dp = 14.dp) {
    Box(Modifier.size(size + 4.dp), contentAlignment = Alignment.Center) {
        when (status) {
            ChatStatus.Awaiting -> ZIcon(ZIcons.Question, "Needs input", Modifier.size(size), tint = MaterialTheme.colorScheme.error)
            ChatStatus.Done -> ZIcon(ZIcons.Check, "Done", Modifier.size(size), tint = successColor())
            ChatStatus.Woke -> ZIcon(ZIcons.Alarm, "Woke", Modifier.size(size), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            ChatStatus.Working -> LoadingIndicator(Modifier.size(size + 4.dp))
            ChatStatus.Errored -> Box(Modifier.size(7.dp).clip(CircleShape).background(MaterialTheme.colorScheme.error))
            ChatStatus.Idle, null -> Box(Modifier.size(7.dp).clip(CircleShape).background(MaterialTheme.colorScheme.onSurface.copy(alpha = 0.14f)))
        }
    }
}

/** A repository's artwork, or its monogram until (unless) that loads. */
@Composable
fun ProjectIcon(tree: ProjectTree?, size: Dp) {
    val art = tree?.let { ProjectArt.of(it) }
    if (art != null) {
        Image(art, tree.name, Modifier.size(size).clip(RoundedCornerShape(size * 0.23f)), contentScale = ContentScale.Crop)
    } else {
        ProjectTile(tree?.name ?: "No Project", tree?.colorIndex ?: 0, size)
    }
}

/** A chat's project tile: the repository's artwork once loaded, else its monogram. */
@Composable
fun ChatProjectTile(project: uniffi.commutator_core.ProjectRef, size: Dp) {
    val art = ProjectArt.forSpace(project.id)
    if (art != null) {
        Image(art, null, Modifier.size(size).clip(RoundedCornerShape(size * 0.23f)), contentScale = ContentScale.Crop)
    } else {
        ProjectTile(project.name, project.colorIndex.toInt(), size)
    }
}

/** A host call; a failure shows as a toast. */
private fun CoroutineScope.run(model: AppModel, failure: String, work: suspend () -> Unit) = launch {
    try {
        work()
    } catch (e: Exception) {
        model.toast("$failure: ${e.message ?: e.javaClass.simpleName}")
    }
}

@Composable
private fun ConfirmDialog(title: String, message: String, action: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(message) },
        confirmButton = {
            TextButton(onClick = { onDismiss(); onConfirm() }) { Text(action, color = MaterialTheme.colorScheme.error) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Delete a chat for good, after asking. */
@Composable
fun ConfirmDeleteChat(model: AppModel, id: String, onDismiss: () -> Unit) =
    ConfirmDialog("Delete this session?", "It's removed for good, on every device.", "Delete", onDismiss) { model.delete(id) }

/** A chat's own actions (every list): Rename, Stop Waiting, Archive (not while it works), Delete. */
fun chatMenu(row: SessionRow, actions: RowActions, rename: () -> Unit, delete: () -> Unit): List<MenuAction> = listOfNotNull(
    MenuAction("Rename…", ZIcons.Rename, onClick = rename),
    if (row.waitsOnProcess) MenuAction("Stop Waiting", ZIcons.Close) { actions.model.stopWaiting(row.id) } else null,
    MenuAction("Archive", ZIcons.Archive, enabled = row.status() != ChatStatus.Working) { actions.archive(row) },
    MenuAction("Delete", ZIcons.Delete, destructive = true, onClick = delete),
)

/**
 * One row of the Projects list, drawn as the Chats rows below it: its own
 * segmented card, 44 dp, a 26 dp icon and 15 sp text, a gap between rows.
 */
@Composable
private fun CardRow(
    index: Int,
    count: Int,
    text: String,
    icon: @Composable () -> Unit,
    onClick: () -> Unit,
    indent: Dp = 0.dp,
    color: Color = MaterialTheme.colorScheme.onSurface,
    onLongClick: (() -> Unit)? = null,
    trailing: @Composable () -> Unit = {},
) {
    val haptics = LocalHapticFeedback.current
    Surface(
        shape = segmentedShapes(index, count).shape,
        color = cardColor(),
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = ListItemDefaults.SegmentedGap).height(44.dp),
    ) {
        Row(
            Modifier
                .fillMaxSize()
                .combinedClickable(
                    onClick = onClick,
                    onLongClick = onLongClick?.let { long -> { haptics.performHapticFeedback(HapticFeedbackType.LongPress); long() } },
                )
                .padding(start = 16.dp + indent, end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(26.dp), contentAlignment = Alignment.Center) { icon() }
            Spacer(Modifier.width(10.dp))
            Text(text, fontSize = 15.sp, color = color, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            trailing()
        }
    }
}

/**
 * The Projects view, as Shepherd's phone app lays out its workspaces: a
 * Projects card (each repository, its worktrees folded under it, New
 * project) and below it every chat, what needs you first. A repository
 * opens its own checkout's chats, a worktree its own; with none yet, a new
 * chat there.
 */
fun LazyListScope.projectsContent(
    model: AppModel,
    trees: List<ProjectTree>,
    chats: List<SessionRow>,
    folded: Set<String>,
    actions: RowActions,
    scope: CoroutineScope,
    onOpenCheckout: (ProjectTree, Checkout) -> Unit,
    onNew: () -> Unit,
    onNewProject: () -> Unit,
) {
    fun open(tree: ProjectTree, checkout: Checkout) {
        if (checkout.chats.isEmpty() && model.prepareNewChat(tree, checkout)) onNew() else onOpenCheckout(tree, checkout)
    }
    item("projects-h") { SectionHeader("Projects", Modifier.padding(start = 8.dp)) }
    val rows = trees.flatMap { t -> listOf(t to null) + (if (FOLD + t.id in folded) emptyList() else t.worktrees.map { t to it }) }
    val count = rows.size + 1
    rows.forEachIndexed { i, (tree, worktree) ->
        if (worktree == null) {
            item("p/${tree.id}") {
                var menu by remember { mutableStateOf(false) }
                var removing by remember { mutableStateOf(false) }
                Box(Modifier.animateItem()) {
                    CardRow(
                        i, count, tree.name,
                        icon = { ProjectIcon(tree, 26.dp) },
                        onClick = { open(tree, tree.main) },
                        onLongClick = { menu = true },
                    ) {
                        StatusGlyph(urgentStatus(tree.main.chats))
                        if (tree.worktrees.isNotEmpty()) {
                            val shut = FOLD + tree.id in folded
                            IconButton(onClick = { model.setGroupCollapsed(FOLD + tree.id, !shut) }, modifier = Modifier.size(28.dp)) {
                                ZIcon(if (shut) ZIcons.ChevronRight else ZIcons.ChevronDown, if (shut) "Show worktrees" else "Hide worktrees", Modifier.size(16.dp), tint = MaterialTheme.colorScheme.outline)
                            }
                        }
                    }
                    ActionMenu(
                        menu, { menu = false },
                        listOf(
                            MenuAction("New Worktree", ZIcons.Worktree) { scope.run(model, "Couldn't create the worktree") { model.createWorktree(tree) } },
                            MenuAction("Remove Project", ZIcons.Delete, destructive = true) { removing = true },
                        ),
                    )
                }
                if (removing) {
                    ConfirmDialog("Remove ${tree.name}?", "It leaves Commutator; its folder stays on disk.", "Remove", { removing = false }) {
                        scope.run(model, "Couldn't remove the project") { model.removeProject(tree) }
                    }
                }
            }
        } else {
            item("w/${worktree.id}") {
                var menu by remember { mutableStateOf(false) }
                var renaming by remember { mutableStateOf(false) }
                var deleting by remember { mutableStateOf(false) }
                // No chats yet: dimmed, as the desktop draws it.
                val alpha = if (worktree.chats.isEmpty()) 0.45f else 1f
                Box(Modifier.animateItem()) {
                    CardRow(
                        i, count, worktree.name,
                        indent = 26.dp,
                        color = MaterialTheme.colorScheme.onSurface.copy(alpha = alpha),
                        icon = { ZIcon(ZIcons.Branch, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = alpha)) },
                        onClick = { open(tree, worktree) },
                        onLongClick = { menu = true },
                    ) {
                        if (worktree.chats.isNotEmpty()) StatusGlyph(urgentStatus(worktree.chats))
                    }
                    ActionMenu(
                        menu, { menu = false },
                        listOfNotNull(
                            MenuAction("Rename", ZIcons.Rename) { renaming = true },
                            worktree.path?.let { MenuAction("Delete Worktree", ZIcons.Delete, destructive = true) { deleting = true } },
                        ),
                    )
                }
                if (renaming) {
                    TextPromptDialog("Rename worktree", worktree.name, "Rename", ZIcons.Rename, onDismiss = { renaming = false }) {
                        model.renameWorktree(worktree.id, it)
                    }
                }
                if (deleting) {
                    ConfirmDialog("Delete ${worktree.name}?", "Its folder is removed from disk (git worktree remove).", "Delete", { deleting = false }) {
                        scope.run(model, "Couldn't delete the worktree") { model.deleteWorktree(tree, worktree) }
                    }
                }
            }
        }
    }
    item("new-project") {
        val accent = MaterialTheme.colorScheme.primary
        CardRow(
            count - 1, count, "New project",
            color = accent,
            icon = { ZIcon(ZIcons.Plus, null, Modifier.size(20.dp), tint = accent) },
            onClick = onNewProject,
        )
    }
    group("projects.chats", "Chats", needsYouFirst(chats), actions)
}

private const val FOLD = "projects.fold:"

/**
 * One checkout's chats, opened from Projects: the repository's own (main,
 * develop…) or a worktree's, what needs you first. New chat runs there;
 * the toggle beside it switches Compact (shared with Chats).
 */
@Composable
fun ProjectChatsScreen(
    model: AppModel,
    treeId: String,
    checkoutId: String,
    actions: RowActions,
    onBack: () -> Unit,
    onNew: () -> Unit,
) {
    val trees by model.projects.collectAsState()
    val compact by model.compactRows.collectAsState()
    val tree = trees.firstOrNull { it.id == treeId }
    val checkout = tree?.checkouts?.firstOrNull { it.id == checkoutId }
    BackHandler(onBack = onBack)
    // Removed (or its worktree deleted) meanwhile: back to Projects.
    LaunchedEffect(trees.isNotEmpty() && checkout == null) { if (trees.isNotEmpty() && checkout == null) onBack() }
    if (tree == null || checkout == null) return
    val rows = needsYouFirst(checkout.chats)
    val list = rememberLazyListState()
    val local = remember(actions, compact) { RowActions(actions.model, actions.onOpen, actions.archive, actions.selected, activity = false, compact = compact) }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        LazyColumn(state = list, contentPadding = PaddingValues(bottom = 32.dp), modifier = Modifier.fillMaxSize()) {
            item("header") {
                Column(Modifier.statusBarsPadding()) {
                    Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        TonalCircleButton(ZIcons.Back, "Back", onClick = onBack)
                        Spacer(Modifier.weight(1f))
                        TonalCircleButton(ZIcons.NewSession, "New chat", onClick = { if (model.prepareNewChat(tree, checkout)) onNew() })
                        Spacer(Modifier.width(8.dp))
                        // Its icon says what a tap does.
                        TonalCircleButton(if (compact) ZIcons.RowsExpand else ZIcons.RowsCompress, if (compact) "Compact: on" else "Compact: off", onClick = { model.setCompactRows(!compact) })
                    }
                    ScreenHeader(if (checkout.id == tree.main.id) tree.name else checkout.name, if (rows.isEmpty()) null else "${rows.size} sessions")
                }
            }
            group("checkout", null, rows, local)
        }
        StatusBarScrim(list)
    }
}
