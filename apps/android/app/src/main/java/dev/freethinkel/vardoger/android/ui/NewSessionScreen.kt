package dev.freethinkel.vardoger.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.background
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import dev.freethinkel.vardoger.android.core.AppModel
import dev.freethinkel.vardoger.android.design.HarnessMark
import dev.freethinkel.vardoger.android.design.ZIcon
import dev.freethinkel.vardoger.android.design.ZIcons
import uniffi.vardoger_core.ModelInfo
import uniffi.vardoger_core.fallbackHarnesses
import uniffi.vardoger_core.fallbackModels
import uniffi.vardoger_core.modelLabel
import uniffi.vardoger_core.harnessLabel
import uniffi.vardoger_core.reasoningLabel

private data class ModelChoice(val harness: String, val harnessLabel: String, val model: ModelInfo)

/** The last catalog each host reported, so chips open on real model names. */
private val modelCache = HashMap<String, List<ModelChoice>>()

private fun catalogModels(): List<ModelChoice> = fallbackHarnesses().filter { it.offered }.flatMap { h ->
    fallbackModels(h.id).map { ModelChoice(h.id, h.label, it) }
}

/**
 * "What are we building?" — a composer-first canvas. Context is set with the
 * composer's chips (project, branch or host, model, effort); the prompt is
 * focused at once so the common case is: tap, type, send.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewSessionScreen(
    model: AppModel,
    onClose: (() -> Unit)?,
    onCreated: (String) -> Unit,
    /** Focus the prompt at once (a large screen's page at launch waits: the keyboard would cover the column). */
    focusOnAppear: Boolean = true,
) {
    val workspace by model.workspace.collectAsState()
    val client by model.client.collectAsState()
    var draft by remember { mutableStateOf(model.lastDraft) }
    // What was set up when the page was last closed unsent comes back.
    val composer = remember { ComposerModel(model.newSessionText).apply { images.addAll(model.newSessionImages) } }
    var created by remember { mutableStateOf(false) }
    fun rememberDraft() {
        if (created) return
        model.lastDraft = draft
        model.newSessionText = composer.text
        model.newSessionImages = composer.images.toList()
    }
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current.lifecycle
    androidx.compose.runtime.DisposableEffect(lifecycle) {
        // The app can be killed in the background: keep the draft first.
        val observer = androidx.lifecycle.LifecycleEventObserver { _, event ->
            if (event == androidx.lifecycle.Lifecycle.Event.ON_STOP) rememberDraft()
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            rememberDraft()
        }
    }
    var newProject by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    val projects = workspace?.projects.orEmpty()
    val hosts = workspace?.devices.orEmpty().filter { it.isExecutionHost }
    val project = projects.firstOrNull { it.id == draft.projectId }
    LaunchedEffect(workspace) {
        if (draft.projectId == null && draft.hostId == null) {
            draft = draft.copy(projectId = (projects.firstOrNull { it.deviceOnline } ?: projects.firstOrNull())?.id)
        }
        if (draft.projectId == null && draft.hostId == null) {
            // No projects yet: run on the first reachable host.
            draft = draft.copy(hostId = (hosts.firstOrNull { it.online } ?: hosts.firstOrNull())?.id)
        }
    }
    val deviceId = project?.deviceId ?: draft.hostId ?: ""
    val modelsKey = deviceId

    // Models for the draft's host: cached (or the built-in catalog) at once, then the host's own list.
    var models by remember(modelsKey) {
        mutableStateOf(modelCache[modelsKey] ?: catalogModels())
    }
    LaunchedEffect(modelsKey, client) {
        val c = client ?: return@LaunchedEffect
        if (deviceId.isEmpty()) return@LaunchedEffect
        val harnesses = runCatching { c.listHarnesses(deviceId) }.getOrNull() ?: return@LaunchedEffect
        val fresh = harnesses.filter { it.offered }.flatMap { h ->
            (runCatching { c.listModels(deviceId, h.id) }.getOrNull() ?: fallbackModels(h.id)).map { ModelChoice(h.id, h.label, it) }
        }
        if (fresh.isNotEmpty()) {
            modelCache[deviceId] = fresh
            models = fresh
        }
    }
    val choice = models.firstOrNull { it.harness == draft.harness && it.model.id == draft.model }
        ?: models.firstOrNull { it.harness == draft.harness }

    LaunchedEffect(Unit) { if (focusOnAppear) focus.requestFocus() }

    fun create() {
        model.lastDraft = draft
        val id = model.createSession(draft.copy(model = draft.model ?: choice?.model?.id), composer.encoded(), composer.images.map { it.outgoing })
        if (id != null) {
            created = true
            model.newSessionText = ""
            model.newSessionImages = emptyList()
            onCreated(id)
        } else {
            scope.launch { snackbar.showSnackbar("Choose a project or a host that can run it.") }
        }
    }

    var composerBounds by remember { mutableStateOf<androidx.compose.ui.geometry.Rect?>(null) }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
    dev.freethinkel.vardoger.android.design.WallpaperHero(model.wallpaper, cutout = { composerBounds })
    Scaffold(
        containerColor = androidx.compose.ui.graphics.Color.Transparent,
        snackbarHost = { SnackbarHost(snackbar) },
        topBar = {
            TopAppBar(
                title = { Text("New session", style = MaterialTheme.typography.titleLargeEmphasized, modifier = Modifier.padding(start = 12.dp)) },
                navigationIcon = { onClose?.let { TonalCircleButton(ZIcons.Close, "Close", onClick = it, modifier = Modifier.padding(start = 8.dp), size = 44.dp) } },
                colors = TopAppBarDefaults.topAppBarColors(containerColor = androidx.compose.ui.graphics.Color.Transparent),
            )
        },
    ) { padding ->
        Column(
            Modifier
                .padding(top = padding.calculateTopPadding())
                .fillMaxSize()
                .imePadding()
                .navigationBarsPadding(),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            // The headline lives in the free space above the composer.
            Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    HarnessMark(draft.harness, 40.dp, tint = MaterialTheme.colorScheme.onSurface)
                    Text(
                        "What are we building?",
                        style = MaterialTheme.typography.headlineSmall,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.padding(horizontal = 32.dp),
                    )
                }
            }
            MentionSuggestions(
                composer,
                search = { q ->
                    val c = client
                    if (c == null || project == null) emptyList() else c.searchFiles(project.deviceId, null, project.id, q)
                },
                modifier = Modifier.padding(horizontal = 12.dp).widthIn(max = 768.dp),
            )
            ComposerSurface(
                model = composer,
                placeholder = "Describe the task",
                action = ComposerAction.Send,
                onAction = ::create,
                alwaysCard = true,
                focusRequester = focus,
                modifier = Modifier
                    .padding(horizontal = 12.dp)
                    .padding(bottom = 8.dp)
                    .widthIn(max = 768.dp)
                    .onGloballyPositioned { composerBounds = it.boundsInRoot() },
            ) {
                ProjectChip(draft, projects, onPick = { draft = it }, onNewProject = { newProject = true })
                HostChip(draft, project, projects, hosts) { draft = it }
                if (project != null && project.gitDetected) {
                    BranchChip(model, draft, project.deviceId, project.path) { draft = it }
                }
                ModelChip(
                    draft,
                    choice,
                    models,
                    host = project?.deviceName ?: hosts.firstOrNull { it.id == draft.hostId }?.name ?: "This computer",
                    hostIcon = ZIcons.Laptop,
                    openAtStart = model.launch.companion == "model-picker",
                ) { draft = it }
                val efforts = choice?.model?.reasoningLevels.orEmpty()
                if (efforts.isNotEmpty()) {
                    val effort = draft.effort?.takeIf { it in efforts } ?: choice?.model?.defaultReasoning ?: efforts[efforts.size / 2]
                    EffortChip(effort, efforts) { draft = draft.copy(effort = it) }
                }
            }
        }
    }
    if (newProject) {
        NewProjectScreen(model, onDismiss = { newProject = false }) { id ->
            draft = draft.copy(projectId = id, hostId = null, branch = null)
        }
    }
}
}

@Composable
private fun ProjectChip(
    draft: dev.freethinkel.vardoger.android.core.NewSessionDraft,
    projects: List<uniffi.vardoger_core.ProjectView>,
    onPick: (dev.freethinkel.vardoger.android.core.NewSessionDraft) -> Unit,
    onNewProject: () -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val project = projects.firstOrNull { it.id == draft.projectId }
    ContextChip(
        project?.groupName ?: "No project",
        leading = {
            if (project != null) ProjectTile(project.groupName, project.colorIndex.toInt(), 18.dp)
            else ZIcon(ZIcons.Home, null, Modifier.size(16.dp))
        },
        onClick = { open = true },
    )
    if (open) ProjectSheet(draft, projects, onDismiss = { open = false }, onNewProject = {
        open = false
        onNewProject()
    }) {
        onPick(it)
        open = false
    }
}

/**
 * One entry per project — checkouts of one repository, on any device, are
 * one entry listing their hosts; the host chip then picks which.
 */
@Composable
private fun ProjectSheet(
    draft: dev.freethinkel.vardoger.android.core.NewSessionDraft,
    projects: List<uniffi.vardoger_core.ProjectView>,
    onDismiss: () -> Unit,
    onNewProject: () -> Unit,
    onPick: (dev.freethinkel.vardoger.android.core.NewSessionDraft) -> Unit,
) {
    val sheet = androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val picked = projects.firstOrNull { it.id == draft.projectId }
    val groups = remember(projects) {
        projects.groupBy { it.groupKey }.values.sortedBy { it.first().groupName.lowercase() }
    }
    androidx.compose.material3.ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheet,
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        androidx.compose.foundation.lazy.LazyColumn(contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
            item {
                Text(
                    "Project",
                    style = MaterialTheme.typography.headlineSmallEmphasized,
                    modifier = Modifier.padding(start = 24.dp, bottom = 16.dp),
                )
            }
            groups.forEachIndexed { i, members ->
                val first = members.first()
                item(first.groupKey) {
                    val hosts = members.map { it.deviceName ?: "Host" }.distinct().sortedBy { it.lowercase() }
                    ProjectRow(
                        selected = first.groupKey == picked?.groupKey,
                        index = i,
                        count = groups.size,
                        leading = { ProjectTile(first.groupName, first.colorIndex.toInt(), 40.dp) },
                        title = first.groupName,
                        supporting = hosts.joinToString(", "),
                        mono = false,
                    ) {
                        // Keep the host when it has a checkout of the project, else the first reachable one.
                        val device = picked?.deviceId ?: draft.hostId
                        val pick = members.firstOrNull { it.id == draft.projectId }
                            ?: members.firstOrNull { it.deviceId == device }
                            ?: members.firstOrNull { it.deviceOnline }
                            ?: first
                        onPick(draft.copy(projectId = pick.id, hostId = null, branch = if (pick.id == draft.projectId) draft.branch else null))
                    }
                }
            }
            item("none") {
                Spacer(Modifier.size(16.dp))
                ProjectRow(
                    selected = draft.projectId == null,
                    index = 0,
                    count = 2,
                    leading = { IconTile(ZIcons.Home) },
                    title = "No project",
                    supporting = "Run in a host's home folder",
                    mono = false,
                ) { onPick(draft.copy(projectId = null, hostId = draft.hostId ?: picked?.deviceId ?: projects.firstOrNull()?.deviceId, worktree = false)) }
            }
            item("new") {
                ProjectRow(
                    selected = false,
                    index = 1,
                    count = 2,
                    leading = { IconTile(ZIcons.Plus) },
                    title = "New project…",
                    supporting = "Pick a folder on one of your computers",
                    mono = false,
                    onClick = onNewProject,
                )
            }
        }
    }
}

@Composable
private fun ProjectRow(
    selected: Boolean,
    index: Int,
    count: Int,
    leading: @Composable () -> Unit,
    title: String,
    supporting: String,
    mono: Boolean,
    onClick: () -> Unit,
) {
    androidx.compose.material3.SegmentedListItem(
        onClick = onClick,
        shapes = segmentedShapes(index, count),
        colors = androidx.compose.material3.ListItemDefaults.segmentedColors(
            containerColor = if (selected) MaterialTheme.colorScheme.secondaryContainer else cardColor(),
        ),
        leadingContent = leading,
        supportingContent = {
            Text(
                supporting,
                style = MaterialTheme.typography.bodySmall,
                fontFamily = if (mono) dev.freethinkel.vardoger.android.design.GeistMono else null,
                maxLines = 1,
                overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            )
        },
        trailingContent = if (selected) {
            {
                Box(
                    Modifier.size(28.dp).clip(androidx.compose.foundation.shape.CircleShape).background(MaterialTheme.colorScheme.primary),
                    contentAlignment = Alignment.Center,
                ) { ZIcon(ZIcons.Check, "Selected", Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onPrimary) }
            }
        } else {
            null
        },
        modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = androidx.compose.material3.ListItemDefaults.SegmentedGap),
    ) { Text(title, style = MaterialTheme.typography.titleMedium) }
}

/**
 * With a project: its checkouts, by host (the path tells apart several on
 * one host). Without: every host — project-less sessions run in its home.
 */
@Composable
private fun HostChip(
    draft: dev.freethinkel.vardoger.android.core.NewSessionDraft,
    project: uniffi.vardoger_core.ProjectView?,
    projects: List<uniffi.vardoger_core.ProjectView>,
    hosts: List<uniffi.vardoger_core.DeviceView>,
    onPick: (dev.freethinkel.vardoger.android.core.NewSessionDraft) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val checkouts = remember(project, projects) {
        if (project == null) emptyList()
        else projects.filter { it.groupKey == project.groupKey }.sortedWith(compareBy({ (it.deviceName ?: "").lowercase() }, { it.path }))
    }
    val title = when {
        project != null -> project.deviceName ?: "Host"
        else -> hosts.firstOrNull { it.id == draft.hostId }?.name ?: "Choose host"
    }
    ContextChip(title, leading = { ZIcon(ZIcons.Laptop, null, Modifier.size(16.dp)) }, onClick = { open = true }) {
        val choices = if (checkouts.isNotEmpty()) {
            checkouts.map { p ->
                val shared = checkouts.count { it.deviceId == p.deviceId } > 1
                MenuChoice(p.deviceName ?: "Host", p.id == draft.projectId, if (shared) p.path else if (p.deviceOnline) "Online" else "Offline") {
                    onPick(draft.copy(projectId = p.id, hostId = null, branch = if (p.id == draft.projectId) draft.branch else null))
                }
            }
        } else {
            hosts.map { h -> MenuChoice(h.name, h.id == draft.hostId, if (h.online) "Online" else "Offline") { onPick(draft.copy(hostId = h.id)) } }
        }
        ChoiceMenu(open, { open = false }, listOf(MenuSection("Run on", choices)))
    }
}

@Composable
private fun BranchChip(
    model: AppModel,
    draft: dev.freethinkel.vardoger.android.core.NewSessionDraft,
    deviceId: String,
    repoPath: String,
    onPick: (dev.freethinkel.vardoger.android.core.NewSessionDraft) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    var refs by remember(deviceId, repoPath) { mutableStateOf<List<String>?>(null) }
    val client by model.client.collectAsState()
    LaunchedEffect(open, deviceId, repoPath) {
        if (open && refs == null) {
            refs = runCatching { client?.listRefs(deviceId, repoPath) }.getOrNull().orEmpty()
                .sortedByDescending { it.current }.map { it.name }
        }
    }
    ContextChip(
        if (draft.worktree) "New worktree" else draft.branch ?: "Current branch",
        leading = { ZIcon(ZIcons.Branch, null, Modifier.size(16.dp)) },
        onClick = { open = true },
    ) {
        val branches = refs.orEmpty()
        ChoiceMenu(open, { open = false }, listOf(
            MenuSection("Checkout", listOf(
                MenuChoice("New worktree", draft.worktree, "Run isolated from the checkout") { onPick(draft.copy(worktree = !draft.worktree)) },
            )),
            MenuSection("Branch", branches.map { b ->
                MenuChoice(b, b == (draft.branch ?: branches.firstOrNull())) { onPick(draft.copy(branch = b)) }
            }),
        ))
    }
}

@Composable
private fun ModelChip(
    draft: dev.freethinkel.vardoger.android.core.NewSessionDraft,
    choice: ModelChoice?,
    models: List<ModelChoice>,
    host: String,
    @androidx.annotation.DrawableRes hostIcon: Int,
    openAtStart: Boolean = false,
    onPick: (dev.freethinkel.vardoger.android.core.NewSessionDraft) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        if (openAtStart) {
            kotlinx.coroutines.delay(600)
            open = true
        }
    }
    // Never the harness name in place of a model.
    val title = choice?.model?.label ?: draft.model?.let { modelLabel(draft.harness, it) }
        ?: fallbackModels(draft.harness).firstOrNull()?.label ?: harnessLabel(draft.harness)
    ContextChip(title, leading = { HarnessMark(draft.harness, 14.dp) }, onClick = { open = true })
    if (open) {
        // A tab per provider the target runs (the draft's first), each listing the models already loaded for it.
        val order = (listOf(draft.harness) + models.map { it.harness }).distinct()
        val tabs = order.mapNotNull { h ->
            val list = models.filter { it.harness == h }
            val first = list.firstOrNull() ?: return@mapNotNull null
            val rows = list.map { ModelPickerRow(it.model.id, it.model.label, it.model.description) }
            ModelPickerTab(h, first.harnessLabel) { rows }
        }
        ModelPickerSheet(
            tabs,
            draft.harness,
            draft.model ?: choice?.model?.id,
            host = host,
            hostIcon = hostIcon,
            onPick = { harness, row ->
                onPick(draft.copy(harness = harness, model = row.id, effort = if (harness != draft.harness) null else draft.effort))
            },
            onDismiss = { open = false },
        )
    }
}

@Composable
private fun EffortChip(effort: String, efforts: List<String>, onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    ContextChip(reasoningLabel(effort), leading = { ZIcon(ZIcons.Effort, null, Modifier.size(16.dp)) }, onClick = { open = true }) {
        ChoiceMenu(open, { open = false }, listOf(MenuSection("Reasoning effort", efforts.map { e ->
            MenuChoice(reasoningLabel(e), e == effort) { onPick(e) }
        })))
    }
}
