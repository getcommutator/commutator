package dev.freethinkel.commutator.android.core

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.commutator_core.ChatIndicator
import uniffi.commutator_core.ProjectRef
import uniffi.commutator_core.ProjectView
import uniffi.commutator_core.SessionRow

class ProjectsTest {
    private fun chat(id: String, at: Long, indicator: ChatIndicator = ChatIndicator.IDLE, cwd: String? = null, unseen: Boolean = false, woke: Long? = null) = SessionRow(
        id = id, revision = 0u, title = id, hasTitle = true, preview = null, project = ProjectRef("p1", "app", 0u),
        deviceId = "mac", deviceName = null, deviceOnline = true, harness = null, harnessLabel = null, model = null,
        modelLabel = null, reasoning = null, branch = null, cwd = cwd, indicator = indicator, hostIndicator = indicator,
        workingSinceMs = null, lastActivityMs = at, timeLabel = "", createdAtMs = 0, unseen = unseen, archived = false,
        pinned = false, sectionId = null, pullRequest = null, sendState = null, parentChatId = null, roomGen = 0u,
        sleepUntilMs = null, wokeAtMs = woke, waitsOnProcess = false,
    )

    private fun project(sessions: List<SessionRow>) = ProjectView(
        id = "p1", name = "app", path = "/src/app/", colorIndex = 0u, deviceId = "mac", deviceName = null,
        deviceOnline = true, gitDetected = true, createdAtMs = 0, groupKey = "g", groupName = "app",
        indicator = ChatIndicator.IDLE, unseenCount = 0u, sessions = sessions,
    )

    @Test
    fun treeSplitsChatsByFolderAndAddsHostWorktrees() {
        val trees = projectTree(
            listOf(project(listOf(chat("a", 1), chat("b", 2, cwd = "/src/app-wt1")))),
            gitWorktrees = mapOf("mac:/src/app" to listOf("/src/app-wt2")),
            names = mapOf("checkout:g|mac:/src/app-wt1" to "Feature"),
        )
        val t = trees.single()
        assertEquals(listOf("a"), t.main.chats.map { it.id })
        // In use first, then the host's own worktree with no chats.
        assertEquals(listOf("Feature", "app-wt2"), t.worktrees.map { it.name })
        assertEquals(listOf("/src/app-wt1", "/src/app-wt2"), t.worktrees.map { it.path })
        assertEquals("p1", t.mainSpace)
        assertEquals(null, t.projectIds[t.worktrees[1].id])
    }

    @Test
    fun aWorktreeTwoProjectsShareHasAnIdInEach() {
        // Two repositories (groups) with chats in one worktree folder: the
        // list keys its rows by id, so each group's row needs its own.
        val shared = "/src/shared-wt"
        val other = project(listOf(chat("b", 2, cwd = shared))).copy(id = "p2", path = "/src/other", groupKey = "h", groupName = "other")
        val trees = projectTree(listOf(project(listOf(chat("a", 1, cwd = shared))), other))
        val ids = trees.flatMap { t -> t.checkouts.map { it.id } }
        assertEquals(ids.size, ids.toSet().size)
    }

    @Test
    fun needsYouFirstThenDoneThenWorkingThenRest() {
        val rows = listOf(
            chat("idle", 9),
            chat("working", 8, ChatIndicator.WORKING),
            chat("done", 7, ChatIndicator.COMPLETED, unseen = true),
            chat("seen", 10, ChatIndicator.COMPLETED),
            chat("input", 1, ChatIndicator.AWAITING_INPUT),
            chat("woke", 2, woke = 11),
        )
        assertEquals(listOf("input", "woke", "done", "working", "seen", "idle"), needsYouFirst(rows).map { it.id })
        assertEquals(ChatStatus.Awaiting, urgentStatus(rows))
        assertEquals(ChatStatus.Done, urgentStatus(rows.filter { it.id != "input" }))
        assertEquals(null, urgentStatus(listOf(chat("idle", 1))))
    }
}
