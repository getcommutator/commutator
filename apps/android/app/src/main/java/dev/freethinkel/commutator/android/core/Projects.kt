package dev.freethinkel.commutator.android.core

import android.graphics.BitmapFactory
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import uniffi.commutator_core.ChatIndicator
import uniffi.commutator_core.CoreClient
import uniffi.commutator_core.ProjectView
import uniffi.commutator_core.SendState
import uniffi.commutator_core.SessionRow

/** One checkout of a repository: its main one or a worktree, with the chats that run in it. */
data class Checkout(
    /** `checkout:<device>:<path>`. */
    val id: String,
    val name: String,
    /** Most recent first. */
    val chats: List<SessionRow>,
    /** A worktree's folder on its main checkout's device (a new chat there runs in it). */
    val path: String? = null,
)

/**
 * One repository in the Projects view (iOS's `ProjectTreeVM`): its main
 * checkout, then its worktrees, the ones in use most recently first.
 */
data class ProjectTree(
    /** `project:<group key>`. */
    val id: String,
    val name: String,
    val colorIndex: Int,
    val main: Checkout,
    val worktrees: List<Checkout>,
    /** The project a checkout stands for (a new chat goes there); none for a worktree known only from where a chat runs. */
    val projectIds: Map<String, String>,
    /** Where the main checkout lives (its artwork is read from there). */
    val deviceId: String,
    val path: String,
) {
    val checkouts: List<Checkout> get() = listOf(main) + worktrees
    val mainSpace: String? get() = projectIds[main.id]
}

/** A chat's state as the lists rank and draw it. */
enum class ChatStatus { Awaiting, Errored, Done, Woke, Working, Idle }

fun SessionRow.status(): ChatStatus = when {
    sendState == SendState.FAILED -> ChatStatus.Errored
    indicator == ChatIndicator.AWAITING_INPUT -> ChatStatus.Awaiting
    indicator == ChatIndicator.ERRORED -> ChatStatus.Errored
    indicator == ChatIndicator.WORKING || indicator == ChatIndicator.WAITING -> ChatStatus.Working
    // Woken from sleep and not looked at since.
    wokeAtMs != null -> ChatStatus.Woke
    indicator == ChatIndicator.COMPLETED && unseen -> ChatStatus.Done
    else -> ChatStatus.Idle
}

/** Its last move: activity, or waking from sleep. */
fun SessionRow.recency(): Long = maxOf(lastActivityMs, wokeAtMs ?: 0L)

/**
 * The order chats run in, as the desktop's Agents: what waits on you (input,
 * a failure), then finished and not seen (or woke), then work under way,
 * then the rest; the most recent first within each.
 */
fun needsYouFirst(rows: List<SessionRow>): List<SessionRow> = rows.sortedWith(
    compareBy<SessionRow> {
        when (it.status()) {
            ChatStatus.Awaiting, ChatStatus.Errored -> 0
            ChatStatus.Done, ChatStatus.Woke -> 1
            ChatStatus.Working -> 2
            ChatStatus.Idle -> 3
        }
    }.thenByDescending { it.recency() },
)

/** A group's status: its most pressing chat's (a question, done and not seen, woke, working, a failure); null when none is. */
fun urgentStatus(rows: List<SessionRow>): ChatStatus? =
    rows.map { it.status() }.filter { it != ChatStatus.Idle }.minByOrNull {
        when (it) {
            ChatStatus.Awaiting -> 0
            ChatStatus.Done -> 1
            ChatStatus.Woke -> 2
            ChatStatus.Working -> 3
            else -> 4
        }
    }

fun trimPath(path: String): String {
    var p = path.trim()
    while (p.length > 1 && (p.endsWith("/") || p.endsWith("\\"))) p = p.dropLast(1)
    return p
}

/**
 * Repositories (checkouts sharing a `groupKey`), most recently active first.
 * A checkout's chats are the ones running in its folder; a chat in another
 * folder (a worktree beside it) makes a worktree row, and so does each of
 * the host's own worktrees ([gitWorktrees], `device:main path` → paths).
 * [names]: worktree names given on this phone, by checkout id.
 */
fun projectTree(
    projects: List<ProjectView>,
    gitWorktrees: Map<String, List<String>> = emptyMap(),
    names: Map<String, String> = emptyMap(),
): List<ProjectTree> {
    val groups = LinkedHashMap<String, MutableList<ProjectView>>()
    for (p in projects) groups.getOrPut(p.groupKey) { mutableListOf() }.add(p)
    val out = mutableListOf<Pair<ProjectTree, Long>>()
    for ((key, checkouts) in groups) {
        val main = checkouts.firstOrNull { it.name == it.groupName } ?: checkouts.first()
        val byFolder = LinkedHashMap<String, MutableList<SessionRow>>()
        val checkoutOf = HashMap<String, ProjectView>()
        for (c in checkouts) {
            val home = c.deviceId + ":" + trimPath(c.path)
            checkoutOf[home] = c
            byFolder.getOrPut(home) { mutableListOf() }
        }
        for (c in checkouts) {
            for (chat in c.sessions) {
                val cwd = chat.cwd?.let(::trimPath)?.takeIf { it.isNotEmpty() }
                byFolder.getOrPut(c.deviceId + ":" + (cwd ?: trimPath(c.path))) { mutableListOf() }.add(chat)
            }
        }
        val mainHome = main.deviceId + ":" + trimPath(main.path)
        for (path in gitWorktrees[mainHome].orEmpty()) byFolder.getOrPut(main.deviceId + ":" + path) { mutableListOf() }
        var mainRow: Checkout? = null
        val worktrees = mutableListOf<Checkout>()
        val projectIds = HashMap<String, String>()
        for ((folder, list) in byFolder) {
            // Per repository: one folder can belong to two groups (a worktree
            // two projects run chats in), and list keys must be unique.
            val id = "checkout:$key|$folder"
            val chats = list.sortedByDescending { it.lastActivityMs }
            checkoutOf[folder]?.let { projectIds[id] = it.id }
            val name = checkoutOf[folder]?.name ?: folder.split('/', '\\').last { it.isNotEmpty() }
            if (folder == mainHome) {
                mainRow = Checkout(id, name, chats)
            } else {
                val path = folder.takeIf { it.startsWith(main.deviceId + ":") }?.removePrefix(main.deviceId + ":")
                worktrees.add(Checkout(id, names[id] ?: name, chats, path))
            }
        }
        // Worktrees in use most recently first; ones without chats last.
        worktrees.sortByDescending { it.chats.firstOrNull()?.lastActivityMs ?: Long.MIN_VALUE }
        val last = checkouts.flatMap { it.sessions }.maxOfOrNull { it.lastActivityMs } ?: main.createdAtMs
        out += ProjectTree(
            "project:$key", main.groupName, main.colorIndex.toInt(), mainRow!!, worktrees, projectIds,
            main.deviceId, trimPath(main.path),
        ) to last
    }
    return out.sortedByDescending { it.second }.map { it.first }
}

/** Each chat's repository (by its checkouts' chats). */
fun projectByChat(trees: List<ProjectTree>): Map<String, ProjectTree> {
    val out = HashMap<String, ProjectTree>()
    for (t in trees) for (c in t.checkouts) for (chat in c.chats) out[chat.id] = t
    return out
}

/**
 * Activity grouped by project: every chat by its repository, the chats by
 * [recency] and the projects in the order their latest chat came back;
 * chats in none under "No Project" (a null tree).
 */
fun activityGroups(rows: List<SessionRow>, owner: Map<String, ProjectTree>): List<Pair<ProjectTree?, List<SessionRow>>> {
    val groups = LinkedHashMap<String?, MutableList<SessionRow>>()
    for (row in rows.distinctBy { it.id }.sortedByDescending { it.recency() }) {
        groups.getOrPut(owner[row.id]?.id) { mutableListOf() }.add(row)
    }
    return groups.map { (id, list) -> (id?.let { owner[list.first().id] }) to list }
}

/**
 * The desktop's repository artwork (`project_icon.rs` ICON_PATHS), read
 * over the attachment route, which serves files under any chat's folder.
 * A project with no chats yet, or on a device out of reach, keeps its
 * monogram until the next try, five minutes on. Main thread only.
 */
object ProjectArt {
    // ponytail: no SVG (BitmapFactory can't decode it); add a renderer when a project needs one.
    private val paths = listOf(
        "public/apple-touch-icon.png", "apple-touch-icon.png", "public/favicon.png",
        "public/icon.png", "public/logo.png", "favicon.png", "app/icon.png", "src/app/icon.png",
        "public/favicon.ico", "favicon.ico", "app/favicon.ico", "static/favicon.ico",
        "src-tauri/icons/icon.png", "assets/icon.png", "src/assets/icon.png",
    )

    /** Loaded artwork by `device:root` (a state map: rows redraw when one lands). */
    private val art = mutableStateMapOf<String, ImageBitmap>()
    private val tried = HashMap<String, Long>()
    /** Each checkout's project id → its repository's artwork key. */
    private val keyBySpace = HashMap<String, String>()

    fun of(tree: ProjectTree): ImageBitmap? = art[tree.deviceId + ":" + tree.path]

    /** A chat's project's artwork, by its `ProjectRef.id`. */
    fun forSpace(spaceId: String?): ImageBitmap? = spaceId?.let { keyBySpace[it] }?.let { art[it] }

    fun link(tree: ProjectTree) {
        for (spaceId in tree.projectIds.values) keyBySpace[spaceId] = tree.deviceId + ":" + tree.path
    }

    fun load(scope: CoroutineScope, client: CoreClient, tree: ProjectTree) {
        val key = tree.deviceId + ":" + tree.path
        val now = System.currentTimeMillis()
        if (art.containsKey(key) || tree.path.isEmpty() || now - (tried[key] ?: 0L) < 300_000) return
        tried[key] = now
        scope.launch {
            for (p in paths) {
                val bytes = runCatching { client.readAttachment(tree.deviceId, tree.path + "/" + p) }.getOrNull() ?: continue
                // As the desktop: the first file found decides, even a corrupt one.
                val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return@launch
                art[key] = bitmap.asImageBitmap()
                tried.remove(key)
                return@launch
            }
        }
    }
}
