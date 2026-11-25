package dev.freethinkel.commutator.android.core

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import dev.freethinkel.commutator.android.design.Appearance
import dev.freethinkel.commutator.android.design.ThemeMode
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.commutator_core.ConnectivityState
import uniffi.commutator_core.ProjectView
import uniffi.commutator_core.SendState
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import uniffi.commutator_core.ChatConfig
import uniffi.commutator_core.ClientEvent
import uniffi.commutator_core.ClientListener
import uniffi.commutator_core.Connectivity
import uniffi.commutator_core.CoreClient
import uniffi.commutator_core.CoreConfig
import uniffi.commutator_core.Credentials
import uniffi.commutator_core.DemoFixture
import uniffi.commutator_core.DemoOptions
import uniffi.commutator_core.NewSession
import uniffi.commutator_core.SandboxLevel
import uniffi.commutator_core.SendRequest
import uniffi.commutator_core.BusyPolicy
import uniffi.commutator_core.SessionRow
import uniffi.commutator_core.SessionTarget
import uniffi.commutator_core.StreamSpeed
import uniffi.commutator_core.TranscriptScale
import uniffi.commutator_core.WorkspaceSnapshot
import uniffi.commutator_core.WorktreeSpec
import uniffi.commutator_core.ServerDevice
import uniffi.commutator_core.ServerSignIn
import uniffi.commutator_core.serverDevices
import uniffi.commutator_core.serverSignOut
import java.io.File
import java.util.UUID
import uniffi.commutator_core.setThemeSelection

/** How the app was launched (adb extras mirror the iOS launch arguments). */
data class LaunchOptions(
    val demo: Boolean = false,
    val fast: Boolean = false,
    val longReply: Boolean = false,
    val big: Boolean = false,
    val huge: Boolean = false,
    val noProjects: Boolean = false,
    val signedOut: Boolean = false,
    val route: String? = null,
    /** `--es companion companions|terminal|side-chat:<id>|subagent:<tool id>`: open it over the routed chat (screenshots). */
    val companion: String? = null,
    /** `--ez terminal-demo true`: the opened terminal types a short script. */
    val terminalDemo: Boolean = false,
    val wallpaper: String? = null,
    val wallpaperEffect: String? = null,
    /**
     * `--es account "<server url> <token> <device id>"` (spaces or commas) `[--es host <machine id>]`: a
     * device token minted on a local dev server (Google can't sign in a
     * server on localhost; debuggable builds). See crates/client/examples/remote_stack.rs.
     */
    val account: String? = null,
    val host: String? = null,
    /** `--es send-on-open <text>`: with `route chat:<id>`, send this once the session is open, for rigs (debuggable builds). */
    val sendOnOpen: String? = null,
)

/**
 * App-wide state owner: holds the Rust [CoreClient], republishes its
 * snapshots as flows, and fans session events out to screens. Core events
 * arrive on Rust threads and hop to the main thread here.
 */
class AppModel(private val app: Application) {
    private val scope = MainScope()
    private val main = Handler(Looper.getMainLooper())
    val credentials = CredentialStore(app)
    val wallpaper = WallpaperStore(app)

    private val _client = MutableStateFlow<CoreClient?>(null)
    val client: StateFlow<CoreClient?> = _client.asStateFlow()

    private val _workspace = MutableStateFlow<WorkspaceSnapshot?>(null)
    val workspace: StateFlow<WorkspaceSnapshot?> = _workspace.asStateFlow()

    private val _connectivity = MutableStateFlow<Connectivity?>(null)
    val connectivity: StateFlow<Connectivity?> = _connectivity.asStateFlow()

    /** Chat ids whose session or composer changed. */
    private val _sessionEvents = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val sessionEvents: SharedFlow<String> = _sessionEvents

    val signInError = MutableStateFlow<String?>(null)

    /** Signed in to the server (the machine picker shows until a host is picked). */
    private val _account = MutableStateFlow(credentials.account())
    val account: StateFlow<CredentialStore.Account?> = _account.asStateFlow()

    /** App-wide snackbars ("Archived · Undo", open failures) shown by the shell. */
    val toasts = MutableSharedFlow<Toast>(extraBufferCapacity = 8)

    fun toast(message: String, action: String? = null, run: (() -> Unit)? = null) {
        toasts.tryEmit(Toast(message, action, run))
    }

    private val settings = app.getSharedPreferences("settings", 0)
    private val _appearance = MutableStateFlow(
        Appearance(
            runCatching { ThemeMode.valueOf(settings.getString("theme", "System")!!) }.getOrDefault(ThemeMode.System),
            settings.getBoolean("dynamicColor", false),
            settings.getString("themeLight", null) ?: Appearance().lightTheme,
            settings.getString("themeDark", null) ?: Appearance().darkTheme,
        ).also { setThemeSelection(it.lightTheme, it.darkTheme) },
    )
    val appearance: StateFlow<Appearance> = _appearance.asStateFlow()

    fun setAppearance(value: Appearance) {
        _appearance.value = value
        // The core paints its own visuals and terminal cells with the selection too.
        setThemeSelection(value.lightTheme, value.darkTheme)
        settings.edit().putString("theme", value.mode.name).putBoolean("dynamicColor", value.dynamicColor)
            .putString("themeLight", value.lightTheme).putString("themeDark", value.darkTheme).apply()
    }

    /** Sessions grouped by project in place of the flat Recent list. */
    private val _groupByProject = MutableStateFlow(settings.getBoolean("groupByProject", false))
    val groupByProject: StateFlow<Boolean> = _groupByProject.asStateFlow()

    fun setGroupByProject(on: Boolean) {
        _groupByProject.value = on
        settings.edit().putBoolean("groupByProject", on).apply()
    }

    /** The front page's view: Projects (repositories and their worktrees) or Chats. Kept on this device. */
    private val _projectsView = MutableStateFlow(settings.getString("sessionsView", "chats") == "projects")
    val projectsView: StateFlow<Boolean> = _projectsView.asStateFlow()

    fun setProjectsView(on: Boolean) {
        _projectsView.value = on
        settings.edit().putString("sessionsView", if (on) "projects" else "chats").apply()
    }

    /** Chats' project filter (a [ProjectTree.id]); null, every project. */
    private val _activityProject = MutableStateFlow(settings.getString("activity.project", null))
    val activityProject: StateFlow<String?> = _activityProject.asStateFlow()

    fun setActivityProject(id: String?) {
        _activityProject.value = id
        settings.edit().putString("activity.project", id).apply()
    }

    /** One-line chat rows (the desktop sidebar's Compact): Chats and a project's chats. */
    private val _compactRows = MutableStateFlow(settings.getBoolean("compactRows", false))
    val compactRows: StateFlow<Boolean> = _compactRows.asStateFlow()

    fun setCompactRows(on: Boolean) {
        _compactRows.value = on
        settings.edit().putBoolean("compactRows", on).apply()
    }

    /** Folded sidebar groups (pinned, recent, project groups), kept on this device. */
    private val _collapsed = MutableStateFlow(settings.getStringSet("collapsedGroups", emptySet())!!.toSet())
    val collapsed: StateFlow<Set<String>> = _collapsed.asStateFlow()

    fun setGroupCollapsed(id: String, collapsed: Boolean) {
        _collapsed.value = if (collapsed) _collapsed.value + id else _collapsed.value - id
        settings.edit().putStringSet("collapsedGroups", _collapsed.value).apply()
    }

    /** Unsent composer text per chat (and the new-session page), kept across launches. */
    val drafts = Drafts(app)

    /** The demo keeps the new-session page in memory only: one run's leftovers must not seed the next. */
    private val persistsNewSession get() = !launch.demo
    private var volatileNewSessionText = ""

    /** What was typed on the new-session page when it was closed unsent. */
    var newSessionText: String
        get() = if (persistsNewSession) drafts.load(Drafts.NEW_SESSION) else volatileNewSessionText
        set(value) {
            if (persistsNewSession) drafts.save(Drafts.NEW_SESSION, value) else volatileNewSessionText = value
        }

    /** Images staged there (this launch only). */
    var newSessionImages: List<dev.freethinkel.commutator.android.ui.StagedImage> = emptyList()

    /** The new-session page's options (project, host, branch, model, effort), kept across launches. */
    var lastDraft: NewSessionDraft = storedDraft()
        set(value) {
            field = value
            if (persistsNewSession) {
                settings.edit()
                    .putString("draft.project", value.projectId)
                    .putString("draft.host", value.hostId)
                    .putString("draft.harness", value.harness)
                    .putString("draft.model", value.model)
                    .putString("draft.effort", value.effort)
                    .apply()
            }
        }

    private fun storedDraft() = NewSessionDraft(
        projectId = settings.getString("draft.project", null),
        hostId = settings.getString("draft.host", null),
        harness = settings.getString("draft.harness", null) ?: "claude-code",
        model = settings.getString("draft.model", null),
        effort = settings.getString("draft.effort", null),
    )

    var launch = LaunchOptions()
        private set

    val isDemo: Boolean get() = _client.value?.isDemo() == true
    private var refreshScheduled = false
    /** The sign-in under way (its PKCE verifier lives in the core object). */
    private var pendingSignIn: ServerSignIn? = null

    private val listener = object : ClientListener {
        override fun onEvent(event: ClientEvent) {
            main.post { handle(event) }
        }
    }

    fun boot(options: LaunchOptions) {
        launch = options
        if (_client.value != null) return
        if (options.signedOut) credentials.clear()
        options.account?.split(' ', ',')?.filter { it.isNotEmpty() }?.takeIf { it.size == 3 }?.let { (url, token, id) ->
            credentials.store(CredentialStore.Account(url, token, id, "dev@localhost", "Local dev"))
            options.host?.let { credentials.host = CredentialStore.Host(it, it) }
        }
        _account.value = credentials.account()
        // The demo starts from the in-memory defaults, never a real account's options.
        if (options.demo) lastDraft = NewSessionDraft()
        watchNetwork()
        // `wallpaper <path>` / `wallpaper none` and `wallpaper-effect <name>`:
        // set the wallpaper at launch (screenshots, tests).
        options.wallpaper?.let { path ->
            if (path == "none") wallpaper.remove() else scope.launch { wallpaper.set(File(path)) }
        }
        options.wallpaperEffect?.let { name ->
            WallpaperStore.effects.firstOrNull { it.name.equals(name, ignoreCase = true) }?.let(wallpaper::setEffect)
        }
        val stored = _account.value
        val host = credentials.host
        when {
            // A `route` with nobody signed in opens on the demo (screenshots, tests).
            options.demo || options.route != null && stored == null -> start(Credentials.Demo(demoOptions()))
            stored != null && host != null -> start(Credentials.Remote(stored.serverUrl, stored.token, host.id))
        }
        // Relative times ("4m") and staleness age without events.
        scope.launch {
            while (true) {
                delay(30_000)
                // Nothing is looking, and this process may not be suspended for a long time.
                if (inForeground) refreshWorkspace()
            }
        }
    }

    fun demoOptions() = DemoOptions(
        fixture = if (launch.noProjects) DemoFixture.NO_PROJECTS else DemoFixture.STANDARD,
        transcriptScale = when {
            launch.huge -> TranscriptScale.Huge
            launch.big -> TranscriptScale.Big
            else -> TranscriptScale.Normal
        },
        streamSpeed = if (launch.fast) StreamSpeed.FAST else StreamSpeed.REALISTIC,
        longReply = launch.longReply,
    )

    private val coreDir get() = File(app.filesDir, "core")

    /** Local docs belong to one sign-in: another account starts empty. Each machine mirrors into its own folder. */
    private fun claimCoreDir(account: CredentialStore.Account) {
        val owner = "${account.serverUrl}|${account.deviceId}"
        val marker = File(coreDir, ".owner")
        if (runCatching { marker.readText() }.getOrNull() != owner) coreDir.deleteRecursively()
        coreDir.mkdirs()
        marker.writeText(owner)
    }

    private fun start(credentials: Credentials): Boolean {
        val account = _account.value
        var dir = File(app.filesDir, "demo")
        if (credentials is Credentials.Remote && account != null) {
            claimCoreDir(account)
            dir = File(File(coreDir, "hosts"), credentials.hostDeviceId)
        }
        dir.mkdirs()
        val config = CoreConfig(
            dataDir = dir.path,
            // The server's id for this phone: what the engine sees it as.
            deviceId = if (credentials is Credentials.Remote) account?.deviceId ?: deviceId else deviceId,
            deviceName = deviceName(),
            platform = "android",
            appVersion = app.packageManager.getPackageInfo(app.packageName, 0).versionName ?: "0",
        )
        return try {
            val client = CoreClient(config, credentials, listener)
            _client.value = client
            client.setNetworkOnline(networkPath.online)
            refreshWorkspace()
            client.preloadSessions()
            true
        } catch (e: Exception) {
            Log.e("Commutator", "core start failed", e)
            false
        }
    }

    fun startDemo() {
        start(Credentials.Demo(demoOptions()))
    }

    private val deviceId: String
        get() {
            val prefs = app.getSharedPreferences("device", 0)
            prefs.getString("deviceId", null)?.let { return it }
            val id = "android-" + UUID.randomUUID().toString().take(8)
            prefs.edit().putString("deviceId", id).apply()
            return id
        }

    private fun deviceName(): String =
        Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL

    // ── sign-in ────────────────────────────────────────────────────────────

    /** The server's Google sign-in URL to open in a Custom Tab (null: a bad address). */
    fun beginSignIn(): String? = try {
        signInError.value = null
        ServerSignIn(SERVER_URL, deviceName()).also { pendingSignIn = it }.url()
    } catch (e: Exception) {
        signInError.value = "That server address doesn't look right."
        null
    }

    /** `commutator://auth?code=…`: redeem it for this phone's token, then pick a machine. */
    fun handleCallback(url: String) {
        val signIn = pendingSignIn ?: run {
            signInError.value = "Sign-in was interrupted. Try again."
            return
        }
        scope.launch {
            try {
                val a = signIn.finish(url)
                pendingSignIn = null
                val saved = CredentialStore.Account(a.serverUrl, a.token, a.deviceId, a.email, a.name)
                credentials.store(saved)
                _account.value = saved
            } catch (e: Exception) {
                Log.w("Commutator", "sign-in failed", e)
                signInError.value = "Sign-in didn't complete. Try again."
            }
        }
    }

    /** The account's machines (the picker's list). */
    suspend fun machines(): List<ServerDevice> {
        val a = _account.value ?: return emptyList()
        return serverDevices(a.serverUrl, a.token)
    }

    /** Drive `machine`'s engine from now on. */
    fun choose(machine: ServerDevice) {
        val a = _account.value ?: return
        credentials.host = CredentialStore.Host(machine.id, machine.name)
        start(Credentials.Remote(a.serverUrl, a.token, machine.id))
    }

    /** Settings › Devices: drive another of the account's machines. */
    fun switchMachine(machine: ServerDevice) {
        leaveMachine()
        choose(machine)
    }

    /** The machine driven now. */
    val host: CredentialStore.Host? get() = credentials.host

    /** Back to the machine picker: the account stays. */
    fun leaveMachine() {
        terminals.signOut()
        _client.value?.shutdown()
        _client.value = null
        _workspace.value = null
        _connectivity.value = null
        credentials.host = null
    }

    /** The user signing out: this device forgets the account's local docs. */
    fun signOut() {
        // The server forgets this phone too; signed out here either way.
        _account.value?.let { a -> scope.launch { runCatching { serverSignOut(a.serverUrl, a.token, a.deviceId) } } }
        signOutLocally(forget = true)
    }

    /**
     * Drop the client. Only a sign-out the user asked for deletes the store:
     * an expired session keeps it (the outbox may hold unsent messages), and
     * the `.owner` marker still keeps a different account out of it.
     */
    private fun signOutLocally(forget: Boolean) {
        val wasDemo = isDemo
        endDeliveryHold()
        lastDeliveryActivity = 0L
        terminals.signOut()
        _client.value?.shutdown()
        _client.value = null
        _workspace.value = null
        _connectivity.value = null
        if (!wasDemo) {
            credentials.clear()
            _account.value = null
            if (forget) coreDir.deleteRecursively()
        }
        // Nothing of the account stays in memory (or seeds the next one's new session).
        lastDraft = NewSessionDraft()
        newSessionText = ""
        newSessionImages = emptyList()
    }

    val accountName: String
        get() = when {
            isDemo -> "Demo"
            else -> _account.value?.let { it.name ?: it.email } ?: "Signed out"
        }

    val accountDetail: String
        get() = if (isDemo) "Offline demo workspace" else _account.value?.let { a ->
            val server = runCatching { android.net.Uri.parse(a.serverUrl).host }.getOrNull() ?: a.serverUrl
            listOfNotNull(if (a.name != null) a.email else null, server).joinToString(" · ")
        } ?: "Commutator account"

    // ── events ─────────────────────────────────────────────────────────────

    private fun handle(event: ClientEvent) {
        when (event) {
            is ClientEvent.WorkspaceChanged -> scheduleRefresh()
            is ClientEvent.SessionChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ComposerChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ConnectivityChanged -> _connectivity.value = event.connectivity
        }
    }

    /** Coalesce bursts of registry frames into one rebuild per loop turn. */
    private fun scheduleRefresh() {
        if (refreshScheduled) return
        refreshScheduled = true
        main.post {
            refreshScheduled = false
            refreshWorkspace()
        }
    }

    fun refreshWorkspace() {
        val client = _client.value ?: return
        val ws = client.workspace()
        _workspace.value = ws
        sweepFreshSideChats()
        val trees = projectTree(ws.projects, gitWorktrees, worktreeNames)
        _projects.value = trees
        loadWorktrees(client, ws.projects)
        // Each repository's artwork, for every row and header that names it.
        for (tree in trees) {
            ProjectArt.link(tree)
            ProjectArt.load(scope, client, tree)
        }
    }

    // ── projects ───────────────────────────────────────────────────────────

    /** Repositories and their worktrees (the Projects view), most recently active first. */
    private val _projects = MutableStateFlow<List<ProjectTree>>(emptyList())
    val projects: StateFlow<List<ProjectTree>> = _projects.asStateFlow()

    /** Each repository's linked worktrees (`device:main path` → paths), as the desktop lists them with no chats yet. */
    private var gitWorktrees: Map<String, List<String>> = emptyMap()
    private val worktreesAsked = HashMap<String, Long>()

    /** Ask each git checkout's host for its worktrees, at most every 15 s; a changed answer rebuilds the tree. */
    private fun loadWorktrees(client: CoreClient, projects: List<ProjectView>) {
        val now = SystemClock.elapsedRealtime()
        for (p in projects) {
            if (!p.gitDetected || p.name != p.groupName) continue
            val main = trimPath(p.path)
            val home = p.deviceId + ":" + main
            if (now - (worktreesAsked[home] ?: -15_001L) <= 15_000) continue
            worktreesAsked[home] = now
            scope.launch {
                val refs = runCatching { client.listRefs(p.deviceId, p.path) }.getOrNull() ?: return@launch
                val paths = refs.mapNotNull { it.worktreePath?.let(::trimPath) }.filter { it != main }.distinct()
                if (gitWorktrees[home] == paths) return@launch
                gitWorktrees = gitWorktrees + (home to paths)
                refreshWorkspace()
            }
        }
    }

    /** Worktree names given on this phone (Projects' Rename), by checkout id. */
    private var worktreeNames: Map<String, String> = settings.getStringSet("worktreeNames", emptySet())!!
        .mapNotNull { it.split('\t', limit = 2).takeIf { p -> p.size == 2 }?.let { (k, v) -> k to v } }.toMap()

    fun renameWorktree(checkoutId: String, name: String) {
        worktreeNames = if (name.isBlank()) worktreeNames - checkoutId else worktreeNames + (checkoutId to name.trim())
        settings.edit().putStringSet("worktreeNames", worktreeNames.map { (k, v) -> "$k\t$v" }.toSet()).apply()
        refreshWorkspace()
    }

    /** A new worktree off the main checkout's HEAD. */
    suspend fun createWorktree(tree: ProjectTree) {
        val client = _client.value ?: return
        val spaceId = tree.mainSpace ?: return
        client.createWorktree(tree.deviceId, spaceId, tree.path, "HEAD")
        reloadWorktrees(tree)
    }

    /** `git worktree remove` on its host, and its project entry if it had one. */
    suspend fun deleteWorktree(tree: ProjectTree, checkout: Checkout) {
        val client = _client.value ?: return
        val path = checkout.path ?: return
        client.deleteWorktree(tree.deviceId, tree.path, path)
        tree.projectIds[checkout.id]?.let { attempt("delete project") { client.deleteProject(it) } }
        reloadWorktrees(tree)
    }

    /** Every checkout of the repository leaves the list (folders stay on disk). */
    fun removeProject(tree: ProjectTree) {
        val client = _client.value ?: return
        for (spaceId in tree.projectIds.values.toSet()) client.deleteProject(spaceId)
        refreshWorkspace()
    }

    private fun reloadWorktrees(tree: ProjectTree) {
        worktreesAsked.remove(tree.deviceId + ":" + tree.path)
        refreshWorkspace()
    }

    /**
     * The worktree the next new chat in this project runs in (`projectId` to
     * its folder: a worktree that isn't a project of its own). Read once by
     * [createSession]; the new-session page clears it when it closes.
     */
    var newChatIn: Pair<String, String>? = null

    /** Set up the new-session page for a chat in this checkout: its project, or the repository's run in its folder. */
    fun prepareNewChat(tree: ProjectTree, checkout: Checkout): Boolean {
        val own = tree.projectIds[checkout.id]
        val projectId = own ?: tree.mainSpace ?: return false
        lastDraft = lastDraft.copy(projectId = projectId, hostId = null, branch = null, worktree = false)
        newChatIn = if (own == null) checkout.path?.let { projectId to it } else null
        return true
    }

    fun stopWaiting(id: String) {
        val client = _client.value ?: return
        scope.launch {
            runCatching { client.stopWaiting(id) }.onFailure {
                Log.w("Commutator", "stop waiting failed", it)
                toast("Couldn't stop waiting")
            }
        }
    }

    private var inForeground = true

    /**
     * iOS suspends a backgrounded app, which is what stops its sockets. Android may not for
     * hours (before 14, a process just stays running with its keepalives waking the radio),
     * so once delivery has had its time the core is told to let go of them.
     */
    private val rest = object : Runnable {
        override fun run() {
            if (inForeground) return
            _client.value?.onBackgroundIdle()
        }
    }

    fun onForeground() {
        inForeground = true
        main.removeCallbacks(rest)
        endDeliveryHold()
        _client.value?.onForeground()
        refreshWorkspace()
    }

    /** When the user last sent something (elapsed realtime). */
    private var lastDeliveryActivity = 0L

    fun noteDeliveryActivity() {
        lastDeliveryActivity = SystemClock.elapsedRealtime()
    }

    fun onBackground() {
        inForeground = false
        // Give the already-durable room outbox and host wake time to reach
        // the edge when the user leaves right after tapping Send.
        endDeliveryHold()
        val recent = lastDeliveryActivity != 0L && SystemClock.elapsedRealtime() - lastDeliveryActivity < 120_000
        val pending = _workspace.value?.let { ws ->
            (ws.front.pinned + ws.front.recent)
                .any { it.sendState == SendState.SENDING || it.sendState == SendState.QUEUED }
        } == true
        if (_client.value != null && !isDemo && (recent || pending)) DeliveryService.hold(app, 25_000)
        _client.value?.onBackground()
        main.removeCallbacks(rest)
        main.postDelayed(rest, REST_AFTER_MS)
    }

    private fun endDeliveryHold() = DeliveryService.release(app)

    /**
     * Pull to refresh: redial the registry and rooms and re-probe health (the
     * same resync as returning to the foreground), then return once the
     * workspace has updated — at least [minimum] ms so the spinner doesn't
     * flicker, at most [maximum] if nothing changes.
     */
    suspend fun refresh(minimum: Long = 600, maximum: Long = 3_000) {
        val started = SystemClock.elapsedRealtime()
        val before = _workspace.value
        onForeground()
        withTimeoutOrNull(maximum) {
            // A reconnecting/offline blip from the redial isn't the resync landing.
            combine(_workspace, _connectivity) { ws, c -> ws to c }.first { (ws, c) ->
                ws != before && c?.state != ConnectivityState.RECONNECTING && c?.state != ConnectivityState.OFFLINE
            }
        }
        val wait = minimum - (SystemClock.elapsedRealtime() - started)
        if (wait > 0) delay(wait)
    }

    private val networkPath = NetworkPathTracker(
        setOnline = {
            Log.i("Commutator", "network: ${if (it) "online" else "no path"}")
            _client.value?.setNetworkOnline(it)
        },
        pathChanged = {},
    )

    private fun watchNetwork() {
        val cm = app.getSystemService(ConnectivityManager::class.java) ?: return
        networkPath.seed(cm.activeNetwork?.let { n -> cm.getNetworkCapabilities(n)?.let { networkShape(n, it) } })
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            // onAvailable says nothing here that this doesn't: it always follows immediately.
            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                val shape = networkShape(network, capabilities)
                main.post { networkPath.update(shape) }
            }

            override fun onBlockedStatusChanged(network: Network, blocked: Boolean) {
                main.post { networkPath.setBlocked(blocked) }
            }

            override fun onLost(network: Network) {
                main.post { networkPath.update(null) }
            }
        })
    }

    // ── writes ─────────────────────────────────────────────────────────────

    private inline fun attempt(what: String, body: () -> Unit) {
        try {
            body()
        } catch (e: Exception) {
            Log.w("Commutator", "$what failed", e)
        }
    }

    fun setPinned(id: String, pinned: Boolean) = attempt("pin") {
        val c = _client.value ?: return
        if (pinned) c.pinSession(id) else c.unpinSession(id)
    }

    /** Archive from anywhere, with an Undo snackbar. */
    fun archive(id: String) {
        attempt("archive") { _client.value?.archiveSession(id) }
        toast("Archived", "Undo") { unarchive(id) }
    }

    fun unarchive(id: String) = attempt("unarchive") { _client.value?.unarchiveSession(id) }
    fun rename(id: String, title: String) = attempt("rename") { _client.value?.renameSession(id, title) }
    fun markSeen(id: String) = attempt("seen") { _client.value?.markSeen(id) }
    fun movePin(id: String, after: String?, before: String?) = attempt("move pin") { _client.value?.movePin(id, after, before) }

    fun row(id: String): SessionRow? = _client.value?.sessionRow(id)

    /** Delete for good (the desktop's "Delete session"). */
    fun delete(id: String) = attempt("delete") {
        _client.value?.deleteSession(id)
        drafts.save(id, "")
        refreshWorkspace()
    }

    /** A device's name ("MacBook Pro"), or a stand-in. */
    fun deviceName(deviceId: String): String =
        _workspace.value?.devices?.firstOrNull { it.id == deviceId }?.name ?: "its computer"

    // ── companions (side chats, subagents, terminals) ──────────────────────

    /** Every chat's terminals (shells on their hosts). */
    val terminals = TerminalStore(app) { _client.value }

    /** A session's side chats, newest first. */
    fun sideChats(chatId: String): List<SessionRow> =
        _client.value?.childSessions(chatId).orEmpty().filter { !it.archived }

    /** The subagents a session's agent spawned (its open transcript). */
    fun sessionSubagents(chatId: String): List<uniffi.commutator_core.SubagentRow> {
        val client = _client.value ?: return emptyList()
        if ("--sub--" in chatId) return emptyList()
        val handle = client.session(chatId) ?: runCatching { client.openSession(chatId) }.getOrNull()
        return handle?.subagents().orEmpty()
    }

    /** Processes the session's agent left running (a dev server, a watcher), oldest first. */
    fun sessionBackground(chatId: String): List<uniffi.commutator_core.BackgroundTaskRow> {
        if ("--sub--" in chatId) return emptyList()
        return _client.value?.session(chatId)?.composer()?.background.orEmpty()
    }

    /** Ask the session's host to stop one; it leaves the list when the agent reports it gone. */
    suspend fun stopBackgroundTask(chatId: String, id: String): Boolean =
        _client.value?.session(chatId)?.stopBackgroundTask(id) ?: false

    /** The tail of one's output; null once it isn't listed. */
    suspend fun backgroundOutput(chatId: String, id: String): uniffi.commutator_core.BackgroundOutput? =
        _client.value?.session(chatId)?.backgroundTaskOutput(id)

    /**
     * Side chats started here that nothing has been sent in yet: closed
     * unused, they go (the desktop only writes one at its first send). Kept
     * across launches, so one left open when the app was killed goes too.
     */
    private var freshSideChats: Set<String> = settings.getStringSet("freshSideChats", emptySet())!!.toSet()
        set(value) {
            field = value
            settings.edit().putStringSet("freshSideChats", value).apply()
        }

    fun isFreshSideChat(id: String) = id in freshSideChats
    fun forgetFreshSideChat(id: String) { freshSideChats = freshSideChats - id }

    /** Chats with a session screen up (the sweep leaves those alone). */
    val openChats = HashSet<String>()

    /** Fresh side chats nobody has open: empty a minute on, they go; one that got a message is just a side chat now. */
    private fun sweepFreshSideChats() {
        if (freshSideChats.isEmpty()) return
        val now = System.currentTimeMillis()
        for (id in freshSideChats) {
            val row = row(id) ?: continue
            if (row.preview != null) {
                forgetFreshSideChat(id)
            } else if (id !in openChats && now - row.createdAtMs > 60_000) {
                forgetFreshSideChat(id)
                delete(id)
            }
        }
    }

    /** A fresh side chat under `parentId`; null when it couldn't be made. */
    fun createSideChat(parentId: String): String? = try {
        _client.value?.createSideChat(parentId)?.also {
            freshSideChats = freshSideChats + it
            refreshWorkspace()
        }
    } catch (e: Exception) {
        Log.w("Commutator", "side chat failed", e)
        null
    }

    /** Fork `sourceId` into a side chat (its host copies the conversation). */
    suspend fun forkSideChat(sourceId: String): String {
        val client = _client.value ?: throw uniffi.commutator_core.CoreException.Closed()
        return client.forkSideChat(sourceId).also { refreshWorkspace() }
    }

    /** Open composers by chat: text handed to a chat (terminal output) lands in its draft. */
    val composerSinks = HashMap<String, (String) -> Unit>()

    /** Text into a chat's composer, after what's there (or its saved draft when it isn't open). */
    fun appendToComposer(chatId: String, text: String) {
        composerSinks[chatId]?.let { return it(text) }
        val draft = drafts.load(chatId)
        drafts.save(chatId, if (draft.isEmpty()) text else draft + "\n" + text)
    }

    /** Create the chat and send its first message. */
    fun createSession(draft: NewSessionDraft, text: String, attachments: List<uniffi.commutator_core.OutgoingAttachment> = emptyList()): String? {
        val client = _client.value ?: return null
        val target = when {
            draft.projectId != null -> SessionTarget.Project(draft.projectId)
            draft.hostId != null -> SessionTarget.Projectless(draft.hostId)
            else -> return null
        }
        val config = ChatConfig(draft.harness, draft.model, draft.effort, emptyMap(), SandboxLevel.WORKSPACE_WRITE)
        return try {
            // An existing worktree picked from Projects runs the chat there, not in a new one.
            val cwd = newChatIn?.takeIf { it.first == draft.projectId }?.second
            newChatIn = null
            val project = _workspace.value?.projects?.firstOrNull { it.id == draft.projectId }
            val worktree = if (cwd == null && draft.worktree && project != null) WorktreeSpec(project.path, draft.branch ?: "HEAD", project.id) else null
            val branch = if (worktree == null && cwd == null && !draft.worktree) draft.branch else null
            val chatId = client.createSession(NewSession(target, config, branch, cwd, null))
            val handle = client.openSession(chatId)
            handle.send(SendRequest(text, attachments, worktree, BusyPolicy.QUEUE))
            noteDeliveryActivity()
            refreshWorkspace()
            chatId
        } catch (e: Exception) {
            Log.w("Commutator", "create session failed", e)
            null
        }
    }
}

/** Past [DeliveryService]'s 25 s, so a message sent just before leaving goes first. */
private const val REST_AFTER_MS = 30_000L

/** The hosted Commutator server every app signs in to. */
private const val SERVER_URL = "https://commutator.freethinkel.dev"

/** A snackbar raised from anywhere in the app. */
class Toast(val message: String, val action: String? = null, val run: (() -> Unit)? = null)

/** The new-session page's options (kept across launches). */
data class NewSessionDraft(
    val projectId: String? = null,
    val hostId: String? = null,
    val harness: String = "claude-code",
    val model: String? = null,
    val effort: String? = null,
    val branch: String? = null,
    val worktree: Boolean = false,
)
