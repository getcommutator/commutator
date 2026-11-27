import Network
import Security
import UIKit

/// App-wide state owner: holds the Rust `CoreClient`, maps its snapshots
/// into view models, and fans change notifications out to screens. All
/// members are main-thread; core events hop here through `ListenerBridge`.
final class AppModel {
    struct FrontPage: Equatable {
        var folders: [FolderRowVM] = []
        var sessions: [SessionRowVM] = []
        var sectionSessions: [String: [SessionRowVM]] = [:]
        /// Repositories, the Projects view's sections (checkouts of one
        /// repository share one), most recently active first.
        var projects: [ProjectTreeVM] = []
    }

    /// One repository in the Projects view (Shepherd's workspace card): its
    /// main checkout, then its other checkouts (worktrees). A checkout's
    /// chats are `sectionSessions[checkout.id]`.
    struct ProjectTreeVM: Equatable {
        /// `project:<group key>`.
        let id: String
        var name: String
        var colorIndex: Int
        var main: FolderRowVM
        var worktrees: [FolderRowVM]
        /// The project a checkout row stands for (a new chat goes there);
        /// none for a worktree known only from where a chat runs.
        var projectIds: [String: String]
        /// Where the main checkout lives (its artwork is read from there).
        var deviceId = ""
        var path = ""
    }

    /// The Sessions page's two views, under Activity: Chats (the chats that
    /// need you in one list, Working, Asleep and Archived at its foot) and
    /// Projects (chats under their repository and its worktrees).
    enum SessionsView: String, CaseIterable {
        case chats, projects
        var title: String { self == .chats ? "Chats" : "Projects" }
        var symbol: String { self == .chats ? "tray.full" : "folder" }
    }

    /// Local to this phone; Chats by default.
    var sessionsView: SessionsView {
        get { UserDefaults.standard.string(forKey: "sessionsView").flatMap(SessionsView.init) ?? .chats }
        set {
            guard newValue != sessionsView else { return }
            UserDefaults.standard.set(newValue.rawValue, forKey: "sessionsView")
            settingsChanged()
        }
    }

    /// Activity's project filter (a `ProjectTreeVM.id`, t3code's): nil, every project.
    var activityProject: String? {
        get { UserDefaults.standard.string(forKey: "activity.project") }
        set {
            guard newValue != activityProject else { return }
            UserDefaults.standard.set(newValue, forKey: "activity.project")
            settingsChanged()
        }
    }

    /// One-line chat rows (the desktop sidebar's Compact): Chats and a project's chats.
    var compactRows: Bool {
        get { UserDefaults.standard.bool(forKey: "projectChatsCompact") }
        set {
            guard newValue != compactRows else { return }
            UserDefaults.standard.set(newValue, forKey: "projectChatsCompact")
            settingsChanged()
        }
    }

    /// Activity grouped by project: one list per project, no status shelves.
    var activityGroupByProject: Bool {
        get { UserDefaults.standard.bool(forKey: "activity.groupByProject") }
        set {
            guard newValue != activityGroupByProject else { return }
            UserDefaults.standard.set(newValue, forKey: "activity.groupByProject")
            settingsChanged()
        }
    }

    /// Each chat's project (by its checkouts' chats), for Activity's filter and groups.
    func projectByChat() -> [String: ProjectTreeVM] {
        var out: [String: ProjectTreeVM] = [:]
        for tree in frontPage.projects {
            for checkout in [tree.main] + tree.worktrees {
                for vm in sessions(inFolder: checkout.id) { out[vm.id] = tree }
            }
        }
        return out
    }

    /// Activity folds chats under way (working, or waiting on work that
    /// will wake them) into Working.
    var activityWorkingShelf: Bool {
        get { UserDefaults.standard.object(forKey: ActivityList.Keys.workingShelf) as? Bool ?? true }
        set {
            guard newValue != activityWorkingShelf else { return }
            UserDefaults.standard.set(newValue, forKey: ActivityList.Keys.workingShelf)
            settingsChanged()
        }
    }

    /// Days a chat may sit idle before Activity archives it (0: never).
    var activityArchiveAfter: Int {
        get { UserDefaults.standard.integer(forKey: ActivityList.Keys.archiveAfter) }
        set {
            guard newValue != activityArchiveAfter else { return }
            UserDefaults.standard.set(newValue, forKey: ActivityList.Keys.archiveAfter)
            settingsChanged()
        }
    }

    /// A view setting changed: redraw, and sweep at once rather than a
    /// minute on.
    private func settingsChanged() {
        lastSweep = nil
        refreshWorkspace()
        observers.values.forEach { $0() }
    }

    /// Chats on screen now (a session page): never archived for being idle.
    var openChatIds = Set<String>()
    private var lastSweep: Date?
    /// The archived chats the last sweep saw (nil until the first: it only
    /// looks).
    private var sweptArchived: Set<String>?

    struct LiveCounts: Equatable {
        var working = 0
        var awaiting = 0
    }

    /// Every chat's terminals (shells on their hosts).
    @MainActor lazy var terminals = TerminalStore(app: self)
    private(set) var client: CoreClient?
    private(set) var frontPage = FrontPage()
    /// The registry has reached this phone since launch; until then an
    /// empty front page means "not loaded yet", not "no chats".
    private(set) var synced = false
    private(set) var archived: [SessionRowVM] = []
    private(set) var connectivity: Connectivity?
    /// Front-page sessions that are working / waiting on the user.
    private(set) var live = LiveCounts()
    private var rows: [String: SessionRow] = [:]
    private var rawProjects: [ProjectView] = []
    private var lastHosts: [HostOption] = []
    private var workspaceRevision: UInt64 = 0
    private var observers: [UUID: () -> Void] = [:]
    private var sessionObservers: [String: [UUID: () -> Void]] = [:]
    private lazy var bridge = ListenerBridge(app: self)
    private var refreshScheduled = false
    private var clock: Timer?
    private let path = NWPathMonitor()

    var onSignedIn: (() -> Void)?
    var onSignOut: (() -> Void)?
    /// The new-session page's options (project, host, branch, model,
    /// effort), kept across launches.
    var lastDraft: NewSessionDraft = {
        var d = !AppModel.persistsNewSession ? NewSessionDraft() : UserDefaults.standard.data(forKey: "newSessionDraft").flatMap { try? JSONDecoder().decode(NewSessionDraft.self, from: $0) } ?? NewSessionDraft()
        // `-harness mock`: live-stack tests must never start a real agent.
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-harness"), i + 1 < args.count { d.harness = args[i + 1] }
        return d
    }() {
        didSet {
            guard Self.persistsNewSession else { return }
            UserDefaults.standard.set(try? JSONEncoder().encode(lastDraft), forKey: "newSessionDraft")
        }
    }

    /// The demo (and the UI tests on it) keep the new-session page in memory
    /// only: one run's leftovers must not seed the next.
    private static let persistsNewSession = !ProcessInfo.processInfo.arguments.contains("-demo")
    private var volatileNewSessionText = ""

    /// What was typed on the new-session page when it was closed unsent.
    var newSessionText: String {
        get { Self.persistsNewSession ? Drafts.load(Self.newSessionDraftKey) : volatileNewSessionText }
        set {
            if Self.persistsNewSession { Drafts.save(Self.newSessionDraftKey, newValue) } else { volatileNewSessionText = newValue }
        }
    }

    /// Images staged there (this launch only).
    var newSessionImages: [StagedImage] = []

    private static let newSessionDraftKey = "new-session"

    /// This phone's sign-in to the account's Commutator server (Keychain).
    private(set) var account = SavedAccount.load()
    /// The machine whose engine this phone drives (one of the account's).
    private(set) var host: SavedHost? = SavedHost.load()

    var isSignedIn: Bool { client != nil }
    var isDemo: Bool { client?.isDemo() ?? false }

    init() {
        // Online/offline + interface changes cut sync backoff short.
        path.pathUpdateHandler = { [weak self] p in
            DispatchQueue.main.async {
                guard let self else { return }
                #if DEBUG
                // A simulator shares its Mac's network, so a rig plays
                // airplane mode with this.
                if ProcessInfo.processInfo.arguments.contains("-network-offline") {
                    self.client?.setNetworkOnline(online: false)
                    return
                }
                #endif
                self.client?.setNetworkOnline(online: p.status == .satisfied)
            }
        }
        path.start(queue: DispatchQueue(label: "dev.freethinkel.commutator.path"))
        let args = ProcessInfo.processInfo.arguments
        #if DEBUG
        // Test hooks (never in release builds): wipe the sign-in, or use a
        // device token minted on a local dev server (`-account <server url>
        // <token> <device id> [-host <machine id>]`; Google can't sign in a
        // local server).
        if args.contains("-signedout") {
            SavedAccount.clear()
            SavedHost.clear()
            account = nil
            host = nil
        }
        if let i = args.firstIndex(of: "-account"), i + 3 < args.count {
            let a = SavedAccount(serverUrl: args[i + 1], token: args[i + 2], deviceId: args[i + 3], email: "dev@localhost", name: "Local dev")
            a.save()
            account = a
            if let h = args.firstIndex(of: "-host"), h + 1 < args.count {
                host = SavedHost(id: args[h + 1], name: args[h + 1])
                host?.save()
            }
        }
        #endif
        if args.contains("-demo") {
            // The demo (and its UI tests) start on Chats, or Projects as asked
            // (`-projects`): one run's choice must not seed the next.
            UserDefaults.standard.set(args.contains("-projects") ? "projects" : "chats", forKey: "sessionsView")
            for key in [ActivityList.Keys.workingShelf, ActivityList.Keys.archiveAfter, ActivityList.Keys.unarchived, ActivityList.Keys.openShelves] {
                UserDefaults.standard.removeObject(forKey: key)
            }
            // Nothing folded either.
            UserDefaults.standard.removeObject(forKey: "collapsedSections")
        }
        if args.contains("-demo") || args.contains("-route") && account == nil {
            start(.demo(options: Self.demoOptions()))
        } else if let account, let host {
            start(.remote(serverUrl: account.serverUrl, token: account.token, hostDeviceId: host.id))
        }
    }

    static func demoOptions() -> DemoOptions {
        let args = ProcessInfo.processInfo.arguments
        let scale: TranscriptScale = args.contains("-huge") ? .huge : args.contains("-big") ? .big : .normal
        return DemoOptions(
            fixture: args.contains("-no-projects") ? .noProjects : args.contains("-ios-only") ? .iosOnly : .standard,
            transcriptScale: scale,
            streamSpeed: args.contains("-fast") ? .fast : .realistic,
            longReply: args.contains("-longreply")
        )
    }

    // MARK: Lifecycle

    private static var coreDir: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("core", isDirectory: true)
    }

    /// Local docs belong to one sign-in: another account (or server) starts
    /// from an empty store (it must never see the last one's cache). Each
    /// machine's engine mirrors into its own folder under it.
    private static func claimCoreDir(for account: SavedAccount) {
        let owner = "\(account.serverUrl)|\(account.deviceId)"
        let marker = coreDir.appendingPathComponent(".owner")
        if (try? String(contentsOf: marker, encoding: .utf8)) != owner {
            try? FileManager.default.removeItem(at: coreDir)
        }
        try? FileManager.default.createDirectory(at: coreDir, withIntermediateDirectories: true)
        try? owner.write(to: marker, atomically: true, encoding: .utf8)
    }

    @discardableResult
    private func start(_ credentials: Credentials) -> Bool {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        var dir = support.appendingPathComponent("demo", isDirectory: true)
        if case let .remote(_, _, hostDeviceId) = credentials, let account {
            Self.claimCoreDir(for: account)
            dir = Self.coreDir.appendingPathComponent("hosts", isDirectory: true).appendingPathComponent(hostDeviceId, isDirectory: true)
        }
        // `-fresh-demo`: the demo from its fixtures alone, none of an earlier
        // run's writes (sleeps, archives) carried over.
        if credentials.isDemo, ProcessInfo.processInfo.arguments.contains("-fresh-demo") {
            try? FileManager.default.removeItem(at: dir)
        }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let config = CoreConfig(
            dataDir: dir.path,
            // The server's id for this phone: what the engine sees it as.
            deviceId: credentials.isDemo ? Self.deviceId : account?.deviceId ?? Self.deviceId,
            deviceName: UIDevice.current.name,
            platform: "ios",
            appVersion: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0"
        )
        do {
            client = try CoreClient(config: config, credentials: credentials, listener: bridge)
            refreshWorkspace()
            client?.preloadSessions()
            clock?.invalidate()
            // Relative times ("4m") and 45s staleness age without events.
            clock = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in self?.refreshWorkspace() }
            return true
        } catch {
            NSLog("core start failed: \(error)")
            client = nil
            return false
        }
    }

    static var deviceId: String {
        if let id = UserDefaults.standard.string(forKey: "deviceId") { return id }
        let id = "ios-" + UUID().uuidString.lowercased().prefix(8)
        UserDefaults.standard.set(id, forKey: "deviceId")
        return id
    }

    /// Signed in to the server: next, pick the machine.
    @MainActor func signedIn(_ a: ServerAccount) {
        let saved = SavedAccount(serverUrl: a.serverUrl, token: a.token, deviceId: a.deviceId, email: a.email, name: a.name)
        saved.save()
        account = saved
        onSignedIn?()
    }

    /// The account's machines (the picker's list), or why not.
    func machines() async throws -> [ServerDevice] {
        guard let account else { throw CoreError.Auth(message: "Signed out") }
        return try await serverDevices(serverUrl: account.serverUrl, token: account.token)
    }

    /// Drive `machine`'s engine from now on.
    @MainActor func choose(_ machine: ServerDevice) {
        guard let account else { return }
        let h = SavedHost(id: machine.id, name: machine.name)
        h.save()
        host = h
        if start(.remote(serverUrl: account.serverUrl, token: account.token, hostDeviceId: h.id)) {
            onSignedIn?()
        }
    }

    /// Back to the machine picker: the account stays.
    @MainActor func leaveMachine() {
        dropMachine()
        onSignedIn?()
    }

    /// Settings › Devices: drive another of the account's machines.
    @MainActor func switchMachine(to machine: ServerDevice) {
        dropMachine()
        choose(machine)
    }

    @MainActor private func dropMachine() {
        terminals.signOut()
        client?.shutdown()
        client = nil
        clock?.invalidate()
        SavedHost.clear()
        host = nil
        resetState()
    }

    /// The user signing out: this device forgets the account's local docs.
    @MainActor func signOut() {
        forgetOnSignOut = true
        // The server forgets this phone too; signed out here either way.
        if let account {
            Task { try? await serverSignOut(serverUrl: account.serverUrl, token: account.token, deviceId: account.deviceId) }
        }
        onSignOut?()
    }

    /// Set only by a sign-out the user asked for. An expired session keeps its
    /// store (the outbox may hold unsent messages); the `.owner` marker still
    /// keeps a different account out of it.
    private var forgetOnSignOut = false

    @MainActor func signOutLocally() {
        terminals.signOut()
        endDeliveryBackgroundTask()
        lastDeliveryActivity = .distantPast
        client?.shutdown()
        client = nil
        SavedAccount.clear()
        SavedHost.clear()
        account = nil
        host = nil
        clock?.invalidate()
        if forgetOnSignOut { try? FileManager.default.removeItem(at: Self.coreDir) }
        forgetOnSignOut = false
        resetState()
    }

    /// Nothing of the account (or machine) stays in memory.
    private func resetState() {
        frontPage = FrontPage()
        synced = false
        archived = []
        rows = [:]
        rawProjects = []
        lastHosts = []
        connectivity = nil
        live = LiveCounts()
        workspaceRevision = 0
        lastDraft = NewSessionDraft()
        newSessionText = ""
        newSessionImages = []
    }

    private var lastDeliveryActivity = Date.distantPast

    /// The person sent something (a new chat or a message): also brings up
    /// the agents' Live Activity.
    func noteDeliveryActivity() {
        lastDeliveryActivity = Date()
        Task { @MainActor in LiveActivities.shared.armForLocalWork() }
    }

    private var deliveryBackgroundTask: UIBackgroundTaskIdentifier = .invalid
    private var deliveryBackgroundDeadline: DispatchWorkItem?

    private func endDeliveryBackgroundTask() {
        deliveryBackgroundDeadline?.cancel()
        deliveryBackgroundDeadline = nil
        guard deliveryBackgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(deliveryBackgroundTask)
        deliveryBackgroundTask = .invalid
    }

    func didEnterBackground() {
        // Give the already-durable room outbox and host wake time to reach
        // the edge when the user leaves immediately after tapping Send.
        endDeliveryBackgroundTask()
        if client != nil, !isDemo,
           Date().timeIntervalSince(lastDeliveryActivity) < 120 || rows.values.contains(where: { $0.sendState == .sending || $0.sendState == .queued }) {
            deliveryBackgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Finish message delivery") { [weak self] in
                self?.endDeliveryBackgroundTask()
            }
            let deadline = DispatchWorkItem { [weak self] in self?.endDeliveryBackgroundTask() }
            deliveryBackgroundDeadline = deadline
            DispatchQueue.main.asyncAfter(deadline: .now() + 25, execute: deadline)
        }
        client?.onBackground()
    }
    func willEnterForeground() {
        endDeliveryBackgroundTask()
        client?.onForeground()
        refreshWorkspace()
    }

    /// Pull to refresh: redial the registry and rooms and re-probe health (the
    /// same resync as returning to the foreground), then report back once the
    /// workspace has updated — at least `minimum` so the spinner doesn't
    /// flicker, at most `maximum` if nothing changes.
    func refresh(minimum: TimeInterval = 0.6, maximum: TimeInterval = 3, completion: @escaping () -> Void) {
        let started = Date()
        var finished = false
        var token: AnyObject?
        let finish = {
            guard !finished else { return }
            finished = true
            token = nil
            let wait = max(0, minimum - Date().timeIntervalSince(started))
            DispatchQueue.main.asyncAfter(deadline: .now() + wait, execute: completion)
        }
        // A reconnecting/offline blip from the redial isn't the resync landing.
        token = observe { [weak self] in
            if let state = self?.connectivity?.state, state == .reconnecting || state == .offline { return }
            finish()
        }
        client?.onForeground()
        refreshWorkspace()
        DispatchQueue.main.asyncAfter(deadline: .now() + maximum) {
            finish()
            _ = token
        }
    }

    // MARK: Observation

    @discardableResult
    func observe(_ handler: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        observers[id] = handler
        return Token { [weak self] in self?.observers[id] = nil }
    }

    func observeSession(_ chatId: String, _ handler: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        sessionObservers[chatId, default: [:]][id] = handler
        return Token { [weak self] in self?.sessionObservers[chatId]?[id] = nil }
    }

    fileprivate final class Token {
        let cancel: () -> Void
        init(cancel: @escaping () -> Void) { self.cancel = cancel }
        deinit { cancel() }
    }

    fileprivate func handle(_ event: ClientEvent) {
        switch event {
        case .workspaceChanged:
            scheduleRefresh()
        case let .sessionChanged(chatId, _), let .composerChanged(chatId, _):
            sessionObservers[chatId]?.values.forEach { $0() }
        case let .connectivityChanged(c):
            connectivity = c
            observers.values.forEach { $0() }
        }
    }

    /// Coalesce bursts of registry frames into one rebuild per runloop turn.
    private func scheduleRefresh() {
        guard !refreshScheduled else { return }
        refreshScheduled = true
        DispatchQueue.main.async { [weak self] in
            self?.refreshScheduled = false
            self?.refreshWorkspace()
        }
    }

    /// Something outside the registry stream changed the workspace (a Cloud
    /// project written here): rebuild now rather than on the next frame.
    func workspaceDidChange() { refreshWorkspace() }

    private func refreshWorkspace() {
        guard let client else { return }
        let ws = client.workspace()
        workspaceRevision = ws.revision
        var all: [String: SessionRow] = [:]
        func vm(_ r: SessionRow) -> SessionRowVM {
            all[r.id] = r
            return Self.vm(r)
        }
        var page = FrontPage()
        let pinned = ws.front.pinned.map(vm)
        if !pinned.isEmpty {
            page.folders.append(FolderRowVM(id: "pinned", name: "Pinned", count: pinned.count, symbol: "pin"))
        }
        page.sectionSessions["pinned"] = pinned
        page.sessions = ws.front.recent.map(vm)
        page.projects = Self.projectTree(ws.projects, worktrees: gitWorktrees, names: worktreeNames, sectionSessions: &page.sectionSessions)
        rawProjects = ws.projects
        loadWorktrees(ws.projects)
        // Each repository's artwork, for every row and header that names it.
        for tree in page.projects {
            for spaceId in tree.projectIds.values { ProjectArt.link(spaceId: spaceId, device: tree.deviceId, root: tree.path) }
            _ = ProjectArt.image(client: client, device: tree.deviceId, root: tree.path) {}
        }
        // Sessions reachable only through their project still resolve by id.
        for p in ws.projects {
            for r in p.sessions where all[r.id] == nil { all[r.id] = r }
        }
        let archivedVMs = ws.archived.map(vm)
        var counts = LiveCounts()
        var seen = Set<String>()
        for row in page.sessions + page.sectionSessions.values.flatMap({ $0 }) where seen.insert(row.id).inserted {
            if row.status == .working { counts.working += 1 }
            if row.status == .awaiting { counts.awaiting += 1 }
        }
        // A row's own content (its model, say) counts even when the lists
        // don't show it: open sessions read their chips from it.
        let rowsChanged = all.count != rows.count || all.contains { rows[$0.key]?.revision != $0.value.revision }
        rows = all
        // Devices coming and going (Settings, host pickers) count as changes.
        let hosts = hostOptions
        let changed = rowsChanged || page != frontPage || archivedVMs != archived || counts != live || hosts != lastHosts || ws.synced != synced
        synced = ws.synced
        lastHosts = hosts
        live = counts
        if !freshSideChats.isEmpty { sweepFreshSideChats() }
        frontPage = page
        archived = archivedVMs
        if changed { observers.values.forEach { $0() } }
        sweepIdle(pinsReady: ws.pinsReady)
    }

    /// Activity's auto-archive, at most once a minute: each chat idle longer
    /// than the days set goes to Archived. Never one that's pinned, open now,
    /// asleep, woken, under way or needing you; nothing until the pins are
    /// known (or a pinned chat could look unpinned). A chat brought back
    /// from Archived counts as just moved, so being old doesn't send it
    /// straight back. (As the desktop's sweep: screens/shell/activity.)
    private func sweepIdle(pinsReady: Bool, now: Date = Date()) {
        if let last = lastSweep, now.timeIntervalSince(last) < 60 { return }
        guard pinsReady else { return }
        lastSweep = now
        let active = frontPage.sessions + frontPage.sectionSessions.values.flatMap { $0 }
        let archivedNow = Set(archived.map(\.id))
        var back = UserDefaults.standard.dictionary(forKey: ActivityList.Keys.unarchived) as? [String: Double] ?? [:]
        let was = sweptArchived
        sweptArchived = archivedNow
        let here = Set(active.map(\.id))
        var changed = false
        if let was {
            for id in here where was.contains(id) {
                back[id] = now.timeIntervalSince1970
                changed = true
            }
        }
        for id in back.keys where !here.contains(id) {
            back[id] = nil
            changed = true
        }
        if changed { UserDefaults.standard.set(back, forKey: ActivityList.Keys.unarchived) }
        guard was != nil, activityArchiveAfter > 0 else { return }
        let cutoff = now.addingTimeInterval(-Double(activityArchiveAfter) * 86_400)
        var seen = Set<String>()
        for vm in active where seen.insert(vm.id).inserted {
            let idleSince = max(vm.lastActivity, back[vm.id].map { Date(timeIntervalSince1970: $0) } ?? .distantPast)
            if ActivityList.archivesWhenIdle(vm, open: openChatIds.contains(vm.id)), idleSince < cutoff {
                attempt("auto-archive") { try client?.archiveSession(chatId: vm.id) }
            }
        }
    }

    /// The Projects view: checkouts grouped by repository (`groupKey`), the
    /// one named for the group as its main checkout. Its worktrees are the
    /// group's other checkouts and every folder a chat runs in that isn't a
    /// checkout's own (the desktop's worktree groups), named for the folder.
    /// Repositories run most recently active first.
    /// Each repository's linked worktrees (`device:path` of its main
    /// checkout → worktree paths), as the desktop lists them with no chats yet.
    private var gitWorktrees: [String: [String]] = [:]
    private var worktreesAsked: [String: Date] = [:]

    /// Ask each git checkout's host for its worktrees, at most every 15 s
    /// (the desktop's freshness); a changed answer rebuilds the tree.
    private func loadWorktrees(_ projects: [ProjectView]) {
        guard let client else { return }
        for p in projects where p.gitDetected && p.name == p.groupName {
            let home = p.deviceId + ":" + Self.trimPath(p.path)
            guard Date().timeIntervalSince(worktreesAsked[home] ?? .distantPast) > 15 else { continue }
            worktreesAsked[home] = Date()
            Task { @MainActor [weak self] in
                guard let refs = try? await client.listRefs(deviceId: p.deviceId, repoPath: p.path) else { return }
                let main = Self.trimPath(p.path)
                let paths = refs.compactMap(\.worktreePath).map(Self.trimPath).filter { $0 != main }
                guard let self, self.gitWorktrees[home] != paths else { return }
                self.gitWorktrees[home] = paths
                self.refreshWorkspace()
            }
        }
    }

    /// Worktree names given on this phone (Projects' Rename), by checkout id.
    private(set) var worktreeNames = UserDefaults.standard.dictionary(forKey: "worktreeNames") as? [String: String] ?? [:]

    func renameWorktree(_ checkoutId: String, to name: String?) {
        worktreeNames[checkoutId] = name.flatMap { $0.trimmingCharacters(in: .whitespaces).isEmpty ? nil : $0 }
        UserDefaults.standard.set(worktreeNames, forKey: "worktreeNames")
        refreshWorkspace()
    }

    /// A new worktree off the main checkout's HEAD.
    func createWorktree(_ project: ProjectTreeVM) async throws {
        guard let client, let spaceId = project.projectIds[project.main.id] else { return }
        _ = try await client.createWorktree(deviceId: project.deviceId, spaceId: spaceId, repoPath: project.path, base: "HEAD")
        await MainActor.run { reloadWorktrees(project) }
    }

    /// `git worktree remove` on its host, and its project entry if it had one.
    func deleteWorktree(_ project: ProjectTreeVM, _ checkout: FolderRowVM) async throws {
        guard let client, let path = checkout.path else { return }
        try await client.deleteWorktree(deviceId: project.deviceId, repoPath: project.path, worktreePath: path)
        await MainActor.run {
            if let spaceId = project.projectIds[checkout.id] { try? client.deleteProject(spaceId: spaceId) }
            reloadWorktrees(project)
        }
    }

    /// Every checkout of the repository leaves the list (folders stay on disk).
    func removeProject(_ project: ProjectTreeVM) throws {
        guard let client else { return }
        for spaceId in Set(project.projectIds.values) { try client.deleteProject(spaceId: spaceId) }
        refreshWorkspace()
    }

    private func reloadWorktrees(_ project: ProjectTreeVM) {
        worktreesAsked[project.deviceId + ":" + project.path] = nil
        refreshWorkspace()
    }

    private static func trimPath(_ path: String) -> String {
        var path = path.trimmingCharacters(in: .whitespaces)
        while path.count > 1, path.hasSuffix("/") || path.hasSuffix("\\") { path.removeLast() }
        return path
    }

    private static func projectTree(_ projects: [ProjectView], worktrees gitWorktrees: [String: [String]] = [:], names: [String: String] = [:], sectionSessions: inout [String: [SessionRowVM]]) -> [ProjectTreeVM] {
        var groups: [String: [ProjectView]] = [:]
        var order: [String] = []
        for p in projects {
            if groups[p.groupKey] == nil { order.append(p.groupKey) }
            groups[p.groupKey, default: []].append(p)
        }
        let trim = trimPath
        func rows(_ list: [SessionRow]) -> [SessionRowVM] {
            list.sorted { $0.lastActivityMs > $1.lastActivityMs }.map(vm)
        }
        var out: [(tree: ProjectTreeVM, last: Int64)] = []
        for key in order {
            let checkouts = groups[key] ?? []
            guard let main = checkouts.first(where: { $0.name == $0.groupName }) ?? checkouts.first else { continue }
            // Chats by the folder they run in: a checkout's own path, or a
            // worktree beside it.
            var byFolder: [String: [SessionRow]] = [:]
            var folderOrder: [String] = []
            var checkoutOf: [String: ProjectView] = [:]
            for c in checkouts {
                let home = c.deviceId + ":" + trim(c.path)
                checkoutOf[home] = c
                if byFolder[home] == nil { folderOrder.append(home); byFolder[home] = [] }
            }
            for c in checkouts {
                for chat in c.sessions {
                    let cwd = chat.cwd.map(trim).flatMap { $0.isEmpty ? nil : $0 }
                    let folder = c.deviceId + ":" + (cwd ?? trim(c.path))
                    if byFolder[folder] == nil { folderOrder.append(folder) }
                    byFolder[folder, default: []].append(chat)
                }
            }
            let mainHome = main.deviceId + ":" + trim(main.path)
            // Worktrees with no chats yet (the host's own list).
            for path in gitWorktrees[mainHome] ?? [] {
                let folder = main.deviceId + ":" + path
                if byFolder[folder] == nil { folderOrder.append(folder); byFolder[folder] = [] }
            }
            var mainRow: FolderRowVM!
            var worktrees: [FolderRowVM] = []
            var projectIds: [String: String] = [:]
            for folder in folderOrder {
                let list = rows(byFolder[folder] ?? [])
                // Per repository: one folder can belong to two groups (a
                // worktree two projects run chats in), and rows need unique ids.
                let id = "checkout:" + key + "|" + folder
                sectionSessions[id] = list
                if let c = checkoutOf[folder] { projectIds[id] = c.id }
                let name = checkoutOf[folder]?.name ?? String(folder.split(whereSeparator: { $0 == "/" || $0 == "\\" }).last ?? Substring(folder))
                if folder == mainHome {
                    mainRow = FolderRowVM(id: id, name: name, count: list.count, symbol: "folder")
                } else {
                    let path = folder.hasPrefix(main.deviceId + ":") ? String(folder.dropFirst(main.deviceId.count + 1)) : nil
                    worktrees.append(FolderRowVM(id: id, name: names[id] ?? name, count: list.count, symbol: "arrow.triangle.branch", path: path))
                }
            }
            // Worktrees in use most recently first; ones without chats last.
            let latest = sectionSessions
            worktrees.sort { (latest[$0.id]?.first?.lastActivity ?? .distantPast) > (latest[$1.id]?.first?.lastActivity ?? .distantPast) }
            let last = checkouts.flatMap(\.sessions).map(\.lastActivityMs).max() ?? main.createdAtMs
            out.append((ProjectTreeVM(id: "project:" + key, name: main.groupName, colorIndex: Int(main.colorIndex), main: mainRow, worktrees: worktrees, projectIds: projectIds, deviceId: main.deviceId, path: trim(main.path)), last))
        }
        return out.sorted { $0.last > $1.last }.map(\.tree)
    }

    /// Every chat not archived, most recent first (the Projects view's Chats).
    var allChats: [SessionRowVM] {
        var seen = Set<String>()
        let rows = frontPage.folders.flatMap { sessions(inFolder: $0.id) } + frontPage.sessions
        return rows.filter { seen.insert($0.id).inserted }.sorted { $0.lastActivity > $1.lastActivity }
    }

    static func status(_ i: ChatIndicator) -> SessionRowVM.Status {
        switch i {
        // Waiting is drawn exactly as Working (`SessionRowVM.waiting` tells
        // them apart).
        case .working, .waiting: .working
        case .awaitingInput: .awaiting
        case .errored: .errored
        case .completed: .completed
        case .idle: .idle
        }
    }

    static func vm(_ r: SessionRow) -> SessionRowVM {
        SessionRowVM(
            id: r.id,
            title: r.title,
            projectName: r.project?.name ?? r.deviceName ?? "No project",
            hasProject: r.project != nil,
            // Project-less sessions tone like the desktop's "home" tile.
            colorIndex: Int(r.project?.colorIndex ?? projectColorIndex(spacePath: "home")),
            harness: r.harness,
            branch: r.branch,
            pr: r.pullRequest.map { pr in
                switch pr.state {
                case .open: .open
                case .merged: .merged
                case .closed: .closed
                }
            },
            prNumber: r.pullRequest?.number,
            status: status(r.indicator),
            timeLabel: r.timeLabel,
            unseen: r.unseen,
            pinned: r.pinned,
            sendFailed: r.sendState == .failed,
            waiting: r.indicator == .waiting,
            canStopWaiting: r.waitsOnProcess,
            sleepUntil: r.sleepUntilMs.map { Date(timeIntervalSince1970: TimeInterval($0) / 1000) },
            wokeAt: r.wokeAtMs.map { Date(timeIntervalSince1970: TimeInterval($0) / 1000) },
            lastActivity: Date(timeIntervalSince1970: TimeInterval(r.lastActivityMs) / 1000),
            projectId: r.project?.id
        )
    }

    func session(_ id: String) -> SessionRowVM? {
        rows[id].map(Self.vm) ?? client?.sessionRow(chatId: id).map(Self.vm)
    }

    func row(_ id: String) -> SessionRow? {
        rows[id] ?? client?.sessionRow(chatId: id)
    }

    func sessions(inFolder id: String) -> [SessionRowVM] {
        id == "archived" ? archived : frontPage.sectionSessions[id] ?? []
    }

    func search(_ query: String) -> [SessionRowVM] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard let client, !q.isEmpty else { return frontPage.sessions }
        return client.search(query: q, limit: 60).map { Self.vm($0.session) }
    }


    // MARK: Writes

    private func attempt(_ what: String, _ body: () throws -> Void) {
        do { try body() } catch { NSLog("\(what) failed: \(error)") }
    }

    func setPinned(_ id: String, _ pinned: Bool) {
        attempt("pin") { pinned ? try client?.pinSession(chatId: id) : try client?.unpinSession(chatId: id) }
    }

    func archive(_ id: String) {
        attempt("archive") { try client?.archiveSession(chatId: id) }
        let window = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
        Toast.show("Archived", action: "Undo", in: window) { [weak self] in self?.unarchive(id) }
    }
    func unarchive(_ id: String) { attempt("unarchive") { try client?.unarchiveSession(chatId: id) } }

    /// Activity's Sleep: asleep until `until`, on every device.
    func sleep(_ id: String, until: Date) {
        do {
            try client?.sleepSession(chatId: id, untilMs: Int64((until.timeIntervalSince1970 * 1000).rounded()))
        } catch {
            NSLog("sleep failed: \(error)")
            Self.toast("Couldn't put this chat to sleep")
        }
    }

    /// Activity's Wake: awake now.
    func wake(_ id: String) {
        do {
            try client?.sleepSession(chatId: id, untilMs: nil)
        } catch {
            NSLog("wake failed: \(error)")
            Self.toast("Couldn't wake this chat")
        }
    }

    /// Stop waiting on the background process its agent said it waits on
    /// (asked of the chat's host): its turn then reads as finished.
    func stopWaiting(_ id: String) {
        guard let client else { return }
        Task { @MainActor in
            do {
                _ = try await client.stopWaiting(chatId: id)
            } catch {
                NSLog("stop waiting failed: \(error)")
                Self.toast("Couldn't stop waiting")
            }
        }
    }

    private static func toast(_ text: String) {
        let window = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
        Toast.show(text, in: window)
    }
    /// Permanently: the row on every device (a Cloud chat's machine too).
    func delete(_ id: String) { attempt("delete") { try client?.deleteSession(chatId: id) } }
    func movePin(_ id: String, after: String?, before: String?) {
        attempt("move pin") { try client?.movePin(chatId: id, after: after, before: before) }
    }
    /// Pin between neighbours (`nil` at either end), as a drop into Pinned does.
    func pin(_ id: String, after: String?, before: String?) {
        attempt("pin") { try client?.pinSessionAt(chatId: id, after: after, before: before) }
    }
    func rename(_ id: String, _ title: String) { attempt("rename") { try client?.renameSession(chatId: id, title: title) } }
    func markSeen(_ id: String) {
        Task { @MainActor in LiveActivities.shared.dropFinished(chatId: id) }
        attempt("seen") { try client?.markSeen(chatId: id) }
    }

    // MARK: Sessions

    func sessionSource(_ chatId: String) -> SessionSource? {
        guard let client else { return nil }
        do {
            let handle = try client.openSession(chatId: chatId)
            return CoreSessionSource(app: self, client: client, handle: handle, chatId: chatId)
        } catch {
            NSLog("open session failed: \(error)")
            return nil
        }
    }

    // MARK: Companions (side chats, subagents)

    /// A session's side chats, newest first.
    func sideChats(of chatId: String) -> [SessionRowVM] {
        (client?.childSessions(parentId: chatId) ?? []).filter { !$0.archived }.map(Self.vm)
    }

    /// The subagents a session's agent spawned (its open transcript).
    func sessionSubagents(_ chatId: String) -> [SubagentRow] {
        guard let client, !chatId.contains("--sub--") else { return [] }
        return (client.session(chatId: chatId) ?? (try? client.openSession(chatId: chatId)))?.subagents() ?? []
    }

    /// Processes the session's agent left running (a dev server, a watcher),
    /// oldest first.
    func sessionBackground(_ chatId: String) -> [BackgroundTaskRow] {
        guard let client, !chatId.contains("--sub--") else { return [] }
        return client.session(chatId: chatId)?.composer().background ?? []
    }

    /// Ask the session's host to stop one; it leaves the list when the agent
    /// reports it gone. `false`: it had ended already.
    func stopBackgroundTask(_ chatId: String, id: String) async throws -> Bool {
        guard let handle = client?.session(chatId: chatId) else { return false }
        return try await handle.stopBackgroundTask(taskId: id)
    }

    /// The tail of one's output; `nil` once it isn't listed.
    func backgroundOutput(_ chatId: String, id: String) async throws -> BackgroundOutput? {
        guard let handle = client?.session(chatId: chatId) else { return nil }
        return try await handle.backgroundTaskOutput(taskId: id)
    }

    /// Side chats started here that nothing has been sent in yet: closed
    /// unused, they go (the desktop only writes one at its first send).
    /// Kept across launches, so one left open when the app was killed goes
    /// too.
    private(set) var freshSideChats = Set(UserDefaults.standard.stringArray(forKey: "freshSideChats") ?? []) {
        didSet { UserDefaults.standard.set(Array(freshSideChats), forKey: "freshSideChats") }
    }

    func forgetFreshSideChat(_ id: String) {
        freshSideChats.remove(id)
    }

    /// Fresh side chats nobody has open: empty a minute on, they go; one
    /// that got a message (from any device) is just a side chat now.
    private func sweepFreshSideChats() {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        for id in freshSideChats {
            guard let row = rows[id] ?? client?.sessionRow(chatId: id) else { continue }
            if row.preview != nil {
                freshSideChats.remove(id)
            } else if SessionViewController.live(id) == nil, now - row.createdAtMs > 60_000 {
                freshSideChats.remove(id)
                delete(id)
            }
        }
    }

    /// A fresh side chat under `parentId`; nil when it couldn't be made.
    func createSideChat(parentId: String) -> String? {
        do {
            let id = try client?.createSideChat(parentId: parentId)
            if let id { freshSideChats.insert(id) }
            refreshWorkspace()
            return id
        } catch {
            NSLog("side chat failed: \(error)")
            return nil
        }
    }

    /// Fork `sourceId` into a side chat (its host copies the conversation).
    func forkSideChat(sourceId: String) async throws -> String {
        guard let client else { throw CoreError.Closed }
        let id = try await client.forkSideChat(sourceId: sourceId)
        await MainActor.run { refreshWorkspace() }
        return id
    }

    // MARK: New session

    /// Who's signed in, by name.
    var accountName: String {
        if isDemo { return "Demo" }
        guard let account else { return "Signed out" }
        return account.name ?? account.email
    }

    var accountDetail: String {
        if isDemo { return "Offline demo workspace" }
        guard let account else { return "Commutator account" }
        let server = URL(string: account.serverUrl)?.host ?? account.serverUrl
        return [account.name != nil ? account.email : nil, server].compactMap { $0 }.joined(separator: " · ")
    }

    @MainActor var projectOptions: [ProjectOption] {
        rawProjects.map { ProjectOption(id: $0.id, name: $0.name, path: $0.path, device: $0.deviceId, deviceName: $0.deviceName ?? deviceName($0.deviceId), online: $0.deviceOnline, git: $0.gitDetected, groupKey: $0.groupKey, groupName: $0.groupName, colorIndex: Int($0.colorIndex)) }
    }

    var hostOptions: [HostOption] {
        (client?.executionDevices() ?? []).map { HostOption(id: $0.id, name: $0.name, online: $0.online) }
    }

    /// A device's name; never its id.
    func deviceName(_ id: String) -> String {
        client?.devices().first { $0.id == id }?.name ?? "Unknown device"
    }

    func models(for deviceId: String) async -> [ModelChoice] {
        guard let client else { return [] }
        let harnesses = (try? await client.listHarnesses(deviceId: deviceId)) ?? fallbackHarnesses()
        var out: [ModelChoice] = []
        for h in harnesses where h.offered {
            let models = (try? await client.listModels(deviceId: deviceId, harness: h.id)) ?? fallbackModels(harness: h.id)
            out += models.map { ModelChoice(harness: h.id, harnessLabel: h.label, id: $0.id, label: $0.label, efforts: $0.reasoningLevels, detail: $0.description) }
        }
        return out
    }

    func listFolders(deviceId: String, path: String?) async -> FolderListing? {
        try? await client?.listFolders(deviceId: deviceId, path: path)
    }

    @MainActor
    func createProject(deviceId: String, path: String, gitDetected: Bool) async -> String? {
        guard let client else { return nil }
        do {
            let id = try await client.createProject(deviceId: deviceId, path: path, gitDetected: gitDetected)
            refreshWorkspace()
            return id
        } catch {
            NSLog("create project failed: \(error)")
            return nil
        }
    }

    func searchFiles(deviceId: String, spaceId: String, query: String) async -> [FileMatch] {
        guard let client else { return [] }
        return (try? await client.searchFiles(deviceId: deviceId, chatId: nil, spaceId: spaceId, query: query)) ?? []
    }

    func refs(projectId: String) async -> [String] {
        guard let client, let p = rawProjects.first(where: { $0.id == projectId }) else { return [] }
        let refs = (try? await client.listRefs(deviceId: p.deviceId, repoPath: p.path)) ?? []
        return refs.sorted { $0.current && !$1.current }.map(\.name)
    }

    /// The worktree a new session starts in. Only a git project can make one:
    /// the pick outlives a switch to a plain folder (where its chip is hidden)
    /// and the host would refuse the run, so there it's left for the next git
    /// project rather than sent.
    static func worktreeSpec(draft: NewSessionDraft, project: ProjectView?) -> WorktreeSpec? {
        guard draft.worktree, let project, project.gitDetected else { return nil }
        return WorktreeSpec(repoPath: project.path, base: draft.branch ?? "HEAD", spaceId: project.id)
    }

    /// The worktree the next new chat in this project runs in (Projects'
    /// worktree without chats); cleared when the new-chat screen closes.
    var newChatIn: (projectId: String, cwd: String)?

    /// Create the chat, open it, and send the first message.
    func createSession(draft: NewSessionDraft, text: String, images: [StagedImage]) -> String? {
        guard let client else { return nil }
        let target: SessionTarget
        if let p = draft.projectId {
            target = .project(spaceId: p)
        } else if let h = draft.hostId {
            target = .projectless(deviceId: h)
        } else {
            return nil
        }
        let config = ChatConfig(harness: draft.harness, model: draft.model, reasoning: draft.effort, modelOptions: [:], sandbox: .workspaceWrite)
        do {
            // An existing worktree picked from Projects runs the chat there, not in a new one.
            let cwd = newChatIn?.projectId == draft.projectId ? newChatIn?.cwd : nil
            newChatIn = nil
            let worktree = cwd != nil ? nil : Self.worktreeSpec(draft: draft, project: rawProjects.first { $0.id == draft.projectId })
            let chatId = try client.createSession(newSession: NewSession(target: target, config: config, branch: worktree == nil && cwd == nil ? draft.branch : nil, cwd: cwd, title: nil))
            let handle = try client.openSession(chatId: chatId)
            _ = try handle.send(request: SendRequest(text: text, attachments: images.map(\.outgoing), worktree: worktree, busy: .queue))
            noteDeliveryActivity()
            refreshWorkspace()
            return chatId
        } catch {
            NSLog("create session failed: \(error)")
            return nil
        }
    }
}

extension StagedImage {
    var outgoing: OutgoingAttachment { OutgoingAttachment(name: name, mimeType: "image/jpeg", data: data) }
}

/// Core events arrive on Rust runtime threads; hop to main.
private final class ListenerBridge: ClientListener, @unchecked Sendable {
    weak var app: AppModel?

    init(app: AppModel) { self.app = app }

    func onEvent(event: ClientEvent) {
        DispatchQueue.main.async { [weak self] in self?.app?.handle(event) }
    }
}

// MARK: - Sign-in persistence (Keychain)

extension Credentials {
    var isDemo: Bool {
        if case .demo = self { return true }
        return false
    }
}

/// This phone's sign-in to the account's server: its bearer (Keychain).
struct SavedAccount: Codable, Equatable {
    var serverUrl: String
    var token: String
    var deviceId: String
    var email: String
    var name: String?

    private static let service = "dev.freethinkel.commutator.ios"
    private static let account = "account"

    static func load() -> SavedAccount? {
        Keychain.load(service: service, account: account).flatMap { try? JSONDecoder().decode(SavedAccount.self, from: $0) }
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { Keychain.save(data, service: Self.service, account: Self.account) }
    }

    static func clear() {
        Keychain.delete(service: service, account: account)
    }
}

/// The machine picked to drive (not a secret: UserDefaults).
struct SavedHost: Codable, Equatable {
    var id: String
    var name: String

    private static let key = "host"

    static func load() -> SavedHost? {
        UserDefaults.standard.data(forKey: key).flatMap { try? JSONDecoder().decode(SavedHost.self, from: $0) }
    }

    func save() {
        UserDefaults.standard.set(try? JSONEncoder().encode(self), forKey: Self.key)
    }

    static func clear() {
        UserDefaults.standard.removeObject(forKey: key)
    }
}

enum Keychain {
    static func save(_ data: Data, service: String, account: String) {
        delete(service: service, account: account)
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        SecItemAdd(q as CFDictionary, nil)
    }

    static func load(service: String, account: String) -> Data? {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: AnyObject?
        return SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess ? out as? Data : nil
    }

    static func delete(service: String, account: String) {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(q as CFDictionary)
    }
}

extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}
