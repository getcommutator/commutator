package dev.freethinkel.commutator.android.ui

import android.net.Uri
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.PlainTooltip
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TooltipAnchorPosition
import androidx.compose.material3.TooltipBox
import androidx.compose.material3.TooltipDefaults
import androidx.compose.material3.rememberTooltipState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isMetaPressed
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.isAltPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.unit.dp
import androidx.navigation.NavHostController
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import kotlinx.coroutines.launch
import dev.freethinkel.commutator.android.core.AppModel
import dev.freethinkel.commutator.android.design.ZIcon
import dev.freethinkel.commutator.android.design.ZIcons
import dev.freethinkel.commutator.android.design.CommutatorTheme

@Composable
fun CommutatorRoot(model: AppModel) {
    val appearance by model.appearance.collectAsState()
    CommutatorTheme(appearance) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            val client by model.client.collectAsState()
            val account by model.account.collectAsState()
            // Signed out → sign in; signed in, no machine → pick one; else the shell.
            val stage = when {
                client != null -> 2
                account != null -> 1
                else -> 0
            }
            Box(Modifier.fillMaxSize()) {
                AnimatedContent(stage, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "root") { s ->
                    when (s) {
                        2 -> MainNav(model)
                        1 -> MachinePickerScreen(model)
                        else -> SignInScreen(model)
                    }
                }
            }
        }
    }
}

object Routes {
    const val HOME = "home"
    const val CHAT = "chat/{id}"
    const val NEW = "new"
    const val SEARCH = "search"
    const val SETTINGS = "settings"
    const val ARCHIVED = "archived"
    const val PULL_REQUESTS = "pull-requests"
    const val APPEARANCE = "appearance"
    const val TERMINAL = "terminal/{chat}?id={id}"
    const val SUBAGENT = "subagent/{parent}/{doc}?title={title}&running={running}"
    fun chat(id: String) = "chat/$id"
    fun terminal(chat: String, id: String?) = "terminal/${Uri.encode(chat)}" + (id?.let { "?id=${Uri.encode(it)}" } ?: "")
    fun subagent(parent: String, doc: String, title: String, running: Boolean) =
        "subagent/${Uri.encode(parent)}/${Uri.encode(doc)}?title=${Uri.encode(title)}&running=$running"
}

@Composable
private fun MainNav(model: AppModel) {
    val nav = rememberNavController()
    val context = androidx.compose.ui.platform.LocalContext.current
    // Large screens (tablets, unfolded foldables): sessions beside the open chat.
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize()) {
    // (Width alone would split a phone turned sideways.)
    // From about an iPad mini in portrait up (iPad shows its split there too).
    val wide = maxWidth >= 700.dp && maxHeight >= 480.dp
    val roomy = maxWidth >= 1100.dp
    val sidebarWidth = if (maxWidth < 1000.dp) 320.dp else 380.dp
    val shell = remember { TabletShellState() }
    val open: (String) -> Unit = { id -> if (wide) shell.select(id) else nav.navigate(Routes.chat(id)) }
    // Folding back to a phone keeps the open chat, full screen; unfolding brings it beside the list.
    LaunchedEffect(wide) {
        if (!wide) {
            val id = shell.selected ?: return@LaunchedEffect
            shell.select(null)
            nav.navigate(Routes.chat(id))
        } else {
            val entry = nav.currentBackStackEntry ?: return@LaunchedEffect
            if (entry.destination.route == Routes.CHAT) {
                val id = entry.arguments?.getString("id")
                nav.popBackStack(Routes.HOME, inclusive = false)
                shell.select(id)
            }
        }
    }
    LaunchedEffect(Unit) {
        val route = model.launch.route ?: return@LaunchedEffect
        val parts = route.split(':')
        when (parts[0]) {
            "new" -> if (wide) shell.newSession() else nav.navigate(Routes.NEW)
            "search" -> if (wide) shell.searching = true else nav.navigate(Routes.SEARCH)
            "settings", "more" -> if (wide) shell.settings = Routes.SETTINGS else nav.navigate(Routes.SETTINGS)
            "archived" -> nav.navigate(Routes.ARCHIVED)
            "chat" -> open(route.removePrefix("chat:"))
        }
    }
    // A debug `companion` extra opens over the launch route's chat, once.
    var companionPending by rememberSaveable { mutableStateOf(model.launch.companion) }
    // And a rig's `send-on-open` is sent there, once.
    var sendPending by rememberSaveable { mutableStateOf(model.launch.sendOnOpen) }

    /** A session page with its companions wired to the nav stack (phone) or the inspector (large screens). */
    @Composable
    fun Session(id: String, onBack: () -> Unit, beside: Boolean = false) {
        val launchChat = model.launch.route?.removePrefix("chat:")
        SessionScreen(
            model,
            id,
            onBack = onBack,
            onOpenSideChat = { side -> if (beside) shell.push(id, InspectorPage.SideChat(side)) else nav.navigate(Routes.chat(side)) },
            // On the phone the sender's chat goes over this one, so Back returns
            // here; beside the sidebar it takes the main column (as on iPad).
            onOpenChat = { chat -> if (beside) shell.select(chat) else nav.navigate(Routes.chat(chat)) },
            onOpenTerminal = { chat, terminal -> if (beside) shell.push(chat, InspectorPage.Terminal(terminal)) else nav.navigate(Routes.terminal(chat, terminal)) },
            onOpenSubagent = { parent, doc, title, running ->
                if (beside) shell.push(id, InspectorPage.Subagent(parent, doc, title, running)) else nav.navigate(Routes.subagent(parent, doc, title, running))
            },
            openCompanion = companionPending?.takeIf { id == launchChat }?.also { companionPending = null },
            sendOnOpen = sendPending?.takeIf { id == launchChat }?.also { sendPending = null },
            onShowCompanions = if (beside) ({ section -> shell.toggleCompanions(id, section) }) else null,
        )
    }

    Box(
        Modifier.fillMaxSize().onPreviewKeyEvent { e ->
            // Hardware keyboards (iPad's): Ctrl+N new session, Ctrl+Shift+F search, Ctrl+B sidebar,
            // Ctrl+J terminal, Ctrl+Alt+0 agents, side chats and terminals.
            if (e.type != KeyEventType.KeyDown || !(e.isCtrlPressed || e.isMetaPressed)) return@onPreviewKeyEvent false
            val chat = if (wide) shell.selected else nav.currentBackStackEntry?.takeIf { it.destination.route == Routes.CHAT }?.arguments?.getString("id")
            when {
                e.key == Key.N -> true.also { if (wide) shell.newSession() else nav.navigate(Routes.NEW) }
                e.key == Key.F && e.isShiftPressed -> true.also { if (wide) shell.searching = true else nav.navigate(Routes.SEARCH) }
                e.key == Key.B && wide -> true.also { shell.sidebar = !shell.sidebar }
                e.key == Key.J && chat != null -> true.also {
                    val live = model.terminals.tabs(chat).firstOrNull { it.isLive }?.ref?.id
                    if (wide) shell.push(chat, InspectorPage.Terminal(live)) else nav.navigate(Routes.terminal(chat, live))
                }
                e.key == Key.Zero && e.isAltPressed && chat != null && wide -> true.also { shell.toggleCompanions(chat, null) }
                else -> false
            }
        },
    ) {
    NavHost(nav, startDestination = Routes.HOME) {
        composable(Routes.HOME) {
            if (!wide) {
                PhoneTabs(
                    model,
                    open,
                    onSearch = { nav.navigate(Routes.SEARCH) },
                    onNew = { nav.navigate(Routes.NEW) },
                    onArchived = { nav.navigate(Routes.ARCHIVED) },
                    onPullRequests = { nav.navigate(Routes.PULL_REQUESTS) },
                    onAppearance = { nav.navigate(Routes.APPEARANCE) },
                )
            } else {
                TabletShell(
                    model,
                    shell,
                    roomy = roomy,
                    // About a phone's width; narrower windows hold it at its minimum.
                    sidebarWidth = sidebarWidth,
                    sidebar = {
                        if (shell.searching) {
                            SearchScreen(model, onBack = { shell.searching = false }, onOpen = { id ->
                                shell.searching = false
                                open(id)
                            })
                        } else {
                            Home(
                                model,
                                open,
                                selected = shell.selected,
                                onSettings = { shell.settings = Routes.SETTINGS },
                                onSearch = { shell.searching = true },
                                onNew = { shell.newSession() },
                                onArchived = { shell.settings = Routes.ARCHIVED },
                                onPullRequests = { shell.settings = Routes.PULL_REQUESTS },
                            )
                        }
                    },
                    session = { id -> Session(id, onBack = { shell.select(null) }, beside = true) },
                    draft = {
                        NewSessionScreen(
                            model,
                            onClose = null,
                            onCreated = { id -> shell.select(id) },
                            focusOnAppear = shell.draftToken > 0,
                        )
                    },
                    page = { chat, page -> InspectorPageContent(model, shell, chat, page) },
                )
            }
        }
        settingsGraph(nav, model, openChat = { id ->
            nav.popBackStack(Routes.HOME, inclusive = false)
            open(id)
        })
        composable(Routes.CHAT) { entry ->
            val id = entry.arguments?.getString("id") ?: return@composable
            Session(id, onBack = { nav.popBackStack() })
        }
        composable(
            Routes.TERMINAL,
            arguments = listOf(navArgument("chat") { type = NavType.StringType }, navArgument("id") { type = NavType.StringType; nullable = true }),
        ) { entry ->
            val chat = entry.arguments?.getString("chat") ?: return@composable
            TerminalScreen(
                model,
                chat,
                terminalId = entry.arguments?.getString("id"),
                onBack = { nav.popBackStack() },
                script = if (model.launch.terminalDemo) TERMINAL_DEMO else emptyList(),
            )
        }
        composable(
            Routes.SUBAGENT,
            arguments = listOf(
                navArgument("parent") { type = NavType.StringType },
                navArgument("doc") { type = NavType.StringType },
                navArgument("title") { type = NavType.StringType; defaultValue = "Subagent" },
                navArgument("running") { type = NavType.BoolType; defaultValue = false },
            ),
        ) { entry ->
            val args = entry.arguments ?: return@composable
            SubagentScreen(
                model,
                parentId = args.getString("parent") ?: return@composable,
                docId = args.getString("doc") ?: return@composable,
                title = args.getString("title") ?: "Subagent",
                running = args.getBoolean("running"),
                onBack = { nav.popBackStack() },
                onOpenSubagent = { parent, doc, title, running -> nav.navigate(Routes.subagent(parent, doc, title, running)) },
            )
        }
        composable(Routes.NEW) {
            NewSessionScreen(
                model,
                onClose = { nav.popBackStack() },
                onCreated = { id ->
                    nav.popBackStack()
                    open(id)
                },
            )
        }
        composable(Routes.SEARCH) {
            SearchScreen(model, onBack = { nav.popBackStack() }, onOpen = { id ->
                if (wide) nav.popBackStack()
                open(id)
            })
        }
    }
    // Settings on a large screen: a dialog over the shell with its own pages (iPad's form sheet).
    shell.settings?.let { start ->
        SettingsDialog(model, start, onDismiss = { shell.settings = null }, openChat = { id ->
            shell.settings = null
            open(id)
        })
    }
    Toasts(model, nav)
    }
    }
}

private val TERMINAL_DEMO = listOf("git log --oneline -5", "cargo test -p commutator-veil")

/**
 * Settings and the pages it leads to (Archived, Pull requests, Appearance): on the phone's stack, or in the large-screen
 * Settings dialog's own. `onClose` ends the dialog from its first page.
 */
private fun androidx.navigation.NavGraphBuilder.settingsGraph(
    nav: NavHostController,
    model: AppModel,
    openChat: (String) -> Unit,
    onClose: (() -> Unit)? = null,
) {
    fun back() {
        if (!nav.popBackStack()) onClose?.invoke()
    }
    composable(Routes.SETTINGS) {
        Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
            SettingsScreen(
                model,
                onArchived = { nav.navigate(Routes.ARCHIVED) },
                onAppearance = { nav.navigate(Routes.APPEARANCE) },
                onBack = ::back,
            )
        }
    }
    composable(Routes.ARCHIVED) {
        ArchivedScreen(model, onBack = ::back, onOpen = openChat)
    }
    composable(Routes.PULL_REQUESTS) {
        PullRequestsScreen(model, onBack = ::back)
    }
    composable(Routes.APPEARANCE) {
        AppearanceScreen(model, onBack = ::back)
    }
}

/** Large screens: Settings over the shell as a dialog with its own stack of pages. */
@Composable
private fun SettingsDialog(model: AppModel, start: String, onDismiss: () -> Unit, openChat: (String) -> Unit) {
    androidx.compose.ui.window.Dialog(
        onDismissRequest = onDismiss,
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false),
    ) {
        val nav = rememberNavController()
        Surface(
            shape = androidx.compose.foundation.shape.RoundedCornerShape(28.dp),
            color = MaterialTheme.colorScheme.background,
            modifier = Modifier.fillMaxWidth(0.9f).widthIn(max = 720.dp).fillMaxHeight(0.92f),
        ) {
            NavHost(nav, startDestination = start) {
                settingsGraph(nav, model, openChat = openChat, onClose = onDismiss)
            }
        }
    }
}

/** App-wide snackbars ("Archived · Undo"), just above whatever bottom chrome the page has. */
@Composable
private fun androidx.compose.foundation.layout.BoxScope.Toasts(model: AppModel, nav: NavHostController) {
    val host = remember { SnackbarHostState() }
    LaunchedEffect(Unit) {
        model.toasts.collect { toast ->
            // One at a time: a new toast replaces the one showing.
            host.currentSnackbarData?.dismiss()
            launch {
                val result = host.showSnackbar(toast.message, actionLabel = toast.action, withDismissAction = toast.action != null, duration = SnackbarDuration.Long)
                if (result == SnackbarResult.ActionPerformed) toast.run?.invoke()
            }
        }
    }
    val entry by nav.currentBackStackEntryAsState()
    val inset = when (entry?.destination?.route) {
        Routes.HOME -> 168.dp
        Routes.CHAT, Routes.NEW -> 96.dp
        Routes.TERMINAL, Routes.SUBAGENT -> 72.dp
        else -> 16.dp
    }
    SnackbarHost(host, Modifier.align(Alignment.BottomCenter).imePadding().navigationBarsPadding().padding(bottom = inset))
}

/**
 * The front page: the Sessions list over a Material 3 bottom app bar —
 * Settings and Search as its actions, New session as its FAB (iOS's
 * bottom bar: Settings · Search · New session). The bar slides away
 * while the list scrolls down and comes back on the way up.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun Home(
    model: AppModel,
    open: (String) -> Unit,
    selected: String? = null,
    onSettings: () -> Unit,
    onSearch: () -> Unit,
    onNew: () -> Unit,
    onArchived: () -> Unit,
    onPullRequests: () -> Unit,
) {
    // The FAB shows its label at rest and tucks to its icon while the list scrolls down (Gmail's Compose).
    var fabExpanded by rememberSaveable { mutableStateOf(true) }
    val fabScroll = remember {
        object : androidx.compose.ui.input.nestedscroll.NestedScrollConnection {
            override fun onPreScroll(available: androidx.compose.ui.geometry.Offset, source: androidx.compose.ui.input.nestedscroll.NestedScrollSource): androidx.compose.ui.geometry.Offset {
                if (available.y < -4f) fabExpanded = false else if (available.y > 4f) fabExpanded = true
                return androidx.compose.ui.geometry.Offset.Zero
            }
        }
    }
    // A checkout opened from Projects (`tree id` to `checkout id`): its chats over the list.
    var checkout by rememberSaveable { mutableStateOf<List<String>?>(null) }
    checkout?.let { (tree, id) ->
        val actions = remember(model, open, selected) { RowActions(model, open, { model.archive(it.id) }, emptyList(), {}, selected) }
        return ProjectChatsScreen(model, tree, id, actions, onBack = { checkout = null }, onNew = onNew)
    }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Box(Modifier.fillMaxSize().nestedScroll(fabScroll)) {
            SessionsScreen(
                model,
                onOpen = open,
                onArchived = onArchived,
                onPullRequests = onPullRequests,
                selected = selected,
                onSearch = onSearch,
                onSettings = onSettings,
                onNew = onNew,
                onOpenCheckout = { tree, c -> checkout = listOf(tree.id, c.id) },
            )
        }
        ExtendedFloatingActionButton(
            text = { Text("New session") },
            icon = { ZIcon(ZIcons.NewSession, null, Modifier.size(24.dp)) },
            onClick = onNew,
            expanded = fabExpanded,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .navigationBarsPadding()
                .padding(16.dp),
        )
    }
}

/** Phone front page: Chats · Pull requests · Settings in a bottom navigation bar. */
@Composable
private fun PhoneTabs(
    model: AppModel,
    open: (String) -> Unit,
    onSearch: () -> Unit,
    onNew: () -> Unit,
    onArchived: () -> Unit,
    onPullRequests: () -> Unit,
    onAppearance: () -> Unit,
) {
    var tab by rememberSaveable { mutableStateOf(0) }
    androidx.compose.material3.Scaffold(
        contentWindowInsets = androidx.compose.foundation.layout.WindowInsets(0),
        bottomBar = {
            androidx.compose.material3.NavigationBar {
                listOf("Chats" to ZIcons.TabSessions, "Pull requests" to ZIcons.PullRequest, "Settings" to ZIcons.TabSettings)
                    .forEachIndexed { i, (label, icon) ->
                        NavigationBarItem(
                            selected = tab == i,
                            onClick = { tab = i },
                            icon = { ZIcon(icon, null, Modifier.size(24.dp)) },
                            label = { Text(label) },
                        )
                    }
            }
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding)) {
            when (tab) {
                0 -> Home(model, open, onSettings = { tab = 2 }, onSearch = onSearch, onNew = onNew, onArchived = onArchived, onPullRequests = onPullRequests)
                1 -> PullRequestsScreen()
                else -> Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
                    SettingsScreen(model, onArchived = onArchived, onAppearance = onAppearance)
                }
            }
        }
    }
}

/** The Pull requests tab. ponytail: empty until the server serves PRs. */
@Composable
private fun PullRequestsScreen() {
    Column(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(32.dp),
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        ZIcon(ZIcons.PullRequest, null, Modifier.size(40.dp))
        Text("No pull requests", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 12.dp))
        Text(
            "Pull requests from your sessions will show up here.",
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            modifier = Modifier.padding(top = 4.dp),
        )
    }
}

/** A long-press label for an icon-only control. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun Tip(text: String, content: @Composable () -> Unit) {
    TooltipBox(
        positionProvider = TooltipDefaults.rememberTooltipPositionProvider(TooltipAnchorPosition.Above),
        tooltip = { PlainTooltip { Text(text) } },
        state = rememberTooltipState(),
        content = content,
    )
}
