import UIKit

/// Shared list machinery for every session list (front page, folders,
/// search): diffable by id, reconfigure-in-place on content
/// changes, fixed row heights, swipe + context actions.
class SessionListController: UIViewController, UICollectionViewDelegate {
    enum Item: Hashable {
        case folder(String)
        case session(String)
        case header(String)
        /// "Show N more" at a section's foot (Activity's Archived).
        case more(String)
    }

    let app: AppModel
    var collectionView: UICollectionView!
    var dataSource: UICollectionViewDiffableDataSource<String, Item>!
    private(set) var sessions: [String: SessionRowVM] = [:]
    private(set) var folders: [String: FolderRowVM] = [:]
    private var headers: [String: String] = [:]
    private var token: AnyObject?
    /// Show drag handles in edit mode (Pinned).
    var reorderable = false
    /// The Archived list: rows offer Unarchive instead of Archive / Pin / Move.
    var archivedRows = false
    /// The session open beside this list (iPad sidebar): drawn as current.
    var currentChatId: String? {
        didSet {
            guard oldValue != currentChatId, dataSource != nil else { return }
            var s = dataSource.snapshot()
            let ids = Set([oldValue, currentChatId].compactMap { $0 })
            s.reconfigureItems(s.itemIdentifiers.filter { if case let .session(id) = $0 { return ids.contains(id) } else { return false } })
            dataSource.apply(s, animatingDifferences: false)
        }
    }
    /// Headers are disclosure rows that fold their section (front page).
    var collapsible = false
    /// Long-press drags a session between the list's sections (front page);
    /// `dropTarget` names what each section means as a destination.
    var movesAcrossSections = false
    private(set) var collapsed = Set(UserDefaults.standard.stringArray(forKey: "collapsedSections") ?? [])
    /// Activity's shelves start folded: the ones opened, kept apart.
    private(set) var openShelves = Set(UserDefaults.standard.stringArray(forKey: ActivityList.Keys.openShelves) ?? [])
    private var headerStates: [String: SectionHeaderCell.State] = [:]

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        var config = UICollectionLayoutListConfiguration(appearance: .plain)
        config.backgroundColor = .clear
        config.showsSeparators = false
        config.leadingSwipeActionsConfigurationProvider = { [weak self] path in self?.leadingSwipe(path) }
        config.trailingSwipeActionsConfigurationProvider = { [weak self] path in self?.trailingSwipe(path) }
        let layout = UICollectionViewCompositionalLayout.list(using: config)
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: layout)
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        if movesAcrossSections {
            collectionView.dragDelegate = self
            collectionView.dropDelegate = self
            // Off by default on iPhone.
            collectionView.dragInteractionEnabled = true
        }
        view.addSubview(collectionView)

        let sessionReg = UICollectionView.CellRegistration<SessionCell, String> { [weak self] cell, path, id in
            guard let self, let vm = self.sessions[id] else { return }
            cell.compact = self.compactRows
            cell.hidesProject = !self.showsProject
            cell.configure(vm)
            cell.isCurrent = id == self.currentChatId
            cell.accessories = self.reorderable ? [.reorder(displayed: .whenEditing)] : []
        }
        let folderReg = UICollectionView.CellRegistration<FolderCell, String> { [weak self] cell, _, id in
            guard let vm = self?.folders[id] else { return }
            cell.configure(vm)
        }
        let headerReg = UICollectionView.CellRegistration<UICollectionViewListCell, String> { [weak self] cell, _, id in
            var c = UIListContentConfiguration.plainHeader()
            c.text = self?.headers[id]
            c.textProperties.font = Fonts.ui(.sansSemibold, 13)
            c.textProperties.color = Palette.secondary
            c.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 18, leading: 20, bottom: 6, trailing: 20)
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.clear()
            bg.backgroundColor = .clear
            cell.backgroundConfiguration = bg
        }
        let moreReg = UICollectionView.CellRegistration<UICollectionViewListCell, String> { [weak self] cell, _, id in
            var c = UIListContentConfiguration.cell()
            c.text = "Show \(self?.moreCount(id) ?? 0) more"
            c.textProperties.font = Fonts.ui(.sansMedium, TypeScale.size(14))
            c.textProperties.color = Palette.secondary
            c.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 10, leading: 20, bottom: 10, trailing: 20)
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.listCell()
            bg.backgroundColor = .clear
            cell.backgroundConfiguration = bg
            cell.accessibilityIdentifier = "more-\(id)"
            cell.accessibilityTraits = .button
        }
        let sectionReg = UICollectionView.CellRegistration<SectionHeaderCell, String> { [weak self] cell, _, id in
            guard let state = self?.headerStates[id] else { return }
            cell.configure(state)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { [weak self] cv, path, item in
            switch item {
            case let .session(id): cv.dequeueConfiguredReusableCell(using: sessionReg, for: path, item: id)
            case let .folder(id): cv.dequeueConfiguredReusableCell(using: folderReg, for: path, item: id)
            case let .header(id) where self?.collapsible == true: cv.dequeueConfiguredReusableCell(using: sectionReg, for: path, item: id)
            case let .header(id): cv.dequeueConfiguredReusableCell(using: headerReg, for: path, item: id)
            case let .more(id): cv.dequeueConfiguredReusableCell(using: moreReg, for: path, item: id)
            }
        }
        token = app.observe { [weak self] in self?.reload(animated: true) }
        artObserver = NotificationCenter.default.addObserver(forName: ProjectArt.loaded, object: nil, queue: .main) { [weak self] _ in
            // Headers first (their state carries the icon), then every row.
            self?.reload(animated: false)
            self?.redrawRows()
        }
        reload(animated: false)
        // Text size changes: fixed row heights change with the category.
        registerForTraitChanges([UITraitPreferredContentSizeCategory.self]) { (self: SessionListController, _) in
            var s = self.dataSource.snapshot()
            s.reconfigureItems(s.itemIdentifiers)
            self.dataSource.apply(s, animatingDifferences: false)
            self.collectionView.collectionViewLayout.invalidateLayout()
        }
        // Pull to re-probe sync (health + room redial) when a network looks stale.
        // (The control is captured directly: a primary action's `sender` is
        // nil, so ending via `action.sender` left the spinner running forever.)
        let refresh = UIRefreshControl()
        refresh.addAction(UIAction { [weak self, weak refresh] _ in
            guard let self else { return refresh?.endRefreshing() ?? () }
            self.app.refresh { refresh?.endRefreshing() }
        }, for: .valueChanged)
        refresh.accessibilityIdentifier = "pull-to-refresh"
        collectionView.refreshControl = refresh
    }

    private var artObserver: NSObjectProtocol?

    /// A section header's icon (Chats grouped by project).
    func headerIcon(_ id: String) -> UIImage? { nil }

    /// Pin and Sleep (menu and swipes): Activity's, off in a project's chats.
    var activityActions: Bool { true }

    /// One-line session rows (a project's chats offer it).
    var compactRows: Bool { false }
    /// Rows name their project (off on a project's own page).
    var showsProject: Bool { true }

    /// Subclasses build their sections from the app model.
    func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] { [] }

    func reload(animated: Bool) {
        let sections = buildSections()
        var snapshot = NSDiffableDataSourceSnapshot<String, Item>()
        var nextSessions: [String: SessionRowVM] = [:]
        var nextFolders: [String: FolderRowVM] = [:]
        var changed: [Item] = []
        for s in sections {
            snapshot.appendSections([s.id])
            var items: [Item] = []
            if let header = s.header {
                headers[s.id] = header
                items.append(.header(s.id))
                if collapsible {
                    let state = SectionHeaderCell.State(
                        id: s.id,
                        title: header,
                        count: s.sessions.count,
                        collapsed: isCollapsed(s.id),
                        live: SessionCell.urgentGlyph(s.sessions),
                        icon: headerIcon(s.id)
                    )
                    if let old = headerStates[s.id], old != state { changed.append(.header(s.id)) }
                    headerStates[s.id] = state
                    if state.collapsed {
                        snapshot.appendItems(items, toSection: s.id)
                        continue
                    }
                }
            }
            for f in s.folders {
                if folders[f.id] != nil, folders[f.id] != f { changed.append(.folder(f.id)) }
                nextFolders[f.id] = f
                items.append(.folder(f.id))
            }
            for r in s.sessions where nextSessions[r.id] == nil {
                if let old = sessions[r.id], old != r { changed.append(.session(r.id)) }
                nextSessions[r.id] = r
                items.append(.session(r.id))
            }
            if moreCount(s.id) > 0 {
                items.append(.more(s.id))
                changed.append(.more(s.id))
            }
            snapshot.appendItems(items, toSection: s.id)
        }
        sessions = nextSessions
        folders = nextFolders
        let present = Set(snapshot.itemIdentifiers)
        snapshot.reconfigureItems(changed.filter { present.contains($0) })
        dataSource.apply(snapshot, animatingDifferences: animated && view.window != nil)
        if shownCompact != nil, shownCompact != compactRows { redrawRows() }
        shownCompact = compactRows
    }

    private var shownCompact: Bool?

    /// Every row again (row height, project artwork).
    func redrawRows() {
        var s = dataSource.snapshot()
        s.reconfigureItems(s.itemIdentifiers)
        dataSource.apply(s, animatingDifferences: false)
        collectionView.collectionViewLayout.invalidateLayout()
    }

    /// Subclasses follow the scroll (the front page fades its wallpaper).
    func listDidScroll(_ scrollView: UIScrollView) {}

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        listDidScroll(scrollView)
    }

    // MARK: Navigation

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        switch dataSource.itemIdentifier(for: indexPath) {
        case let .session(id):
            openSession(id)
        case let .folder(id):
            openFolder(id)
        case let .header(id) where collapsible:
            toggleSection(id)
        case let .more(id):
            showMore(id)
        default:
            break
        }
    }

    func collectionView(_ collectionView: UICollectionView, shouldSelectItemAt indexPath: IndexPath) -> Bool {
        if case .header = dataSource.itemIdentifier(for: indexPath) { return collapsible }
        return true
    }

    func toggleSection(_ id: String) {
        setCollapsed(id, !isCollapsed(id))
        UISelectionFeedbackGenerator().selectionChanged()
        reload(animated: true)
    }

    /// Whether section `id` is folded: as the person left it, Activity's
    /// shelves folded until opened.
    func isCollapsed(_ id: String) -> Bool {
        ActivityList.Section.shelves.contains(id) ? !openShelves.contains(id) : collapsed.contains(id)
    }

    func setCollapsed(_ id: String, _ folded: Bool) {
        if ActivityList.Section.shelves.contains(id) {
            if folded { openShelves.remove(id) } else { openShelves.insert(id) }
            UserDefaults.standard.set(Array(openShelves), forKey: ActivityList.Keys.openShelves)
        } else {
            if folded { collapsed.insert(id) } else { collapsed.remove(id) }
            UserDefaults.standard.set(Array(collapsed), forKey: "collapsedSections")
        }
    }

    /// How many of section `id`'s sessions aren't shown yet ("Show N more").
    func moreCount(_ id: String) -> Int { 0 }

    /// "Show N more" tapped: show the next few.
    func showMore(_ id: String) {}

    /// Long-press actions for a collapsible section header.
    func headerMenu(_ id: String) -> UIMenu? { nil }

    /// What dropping a dragged session into list section `id` means; nil
    /// where sessions can't be dropped.
    func dropTarget(_ id: String) -> SessionDropTarget? { nil }

    func openSession(_ id: String) {
        // iPad: sessions open beside the sidebar.
        if let split = splitViewController as? SplitRootController, !split.isCollapsed {
            split.openSession(id)
            return
        }
        guard let session = SessionViewController(app: app, chatId: id) else {
            mainShell?.showSessionOpenError()
            return
        }
        navigationController?.pushViewController(session, animated: true)
    }

    func openFolder(_ id: String) {
        guard let f = folders[id] ?? app.frontPage.folders.first(where: { $0.id == id }) else { return }
        navigationController?.pushViewController(FolderViewController(app: app, folder: f), animated: true)
    }

    // MARK: Actions

    private func sessionId(_ path: IndexPath) -> String? {
        if case let .session(id) = dataSource.itemIdentifier(for: path) { return id }
        return nil
    }

    /// An archived row (the Archived list, or Activity's Archived shelf):
    /// Unarchive instead of Archive / Pin / Move.
    private func isArchivedRow(_ path: IndexPath) -> Bool {
        if archivedRows { return true }
        let sections = dataSource.snapshot().sectionIdentifiers
        return path.section < sections.count && sections[path.section] == ActivityList.Section.archived
    }

    private func leadingSwipe(_ path: IndexPath) -> UISwipeActionsConfiguration? {
        guard activityActions, !isArchivedRow(path), let id = sessionId(path), let vm = sessions[id] else { return nil }
        let pin = UIContextualAction(style: .normal, title: vm.pinned ? "Unpin" : "Pin") { [weak self] _, _, done in
            self?.app.setPinned(id, !vm.pinned)
            done(true)
        }
        pin.image = UIImage(systemName: vm.pinned ? "pin.slash.fill" : "pin.fill")
        pin.backgroundColor = .systemGray
        return UISwipeActionsConfiguration(actions: [pin])
    }

    private func trailingSwipe(_ path: IndexPath) -> UISwipeActionsConfiguration? {
        guard let id = sessionId(path) else { return nil }
        if isArchivedRow(path) {
            let restore = UIContextualAction(style: .normal, title: "Unarchive") { [weak self] _, _, done in
                self?.app.unarchive(id)
                done(true)
            }
            restore.image = UIImage(systemName: "tray.and.arrow.up.fill")
            restore.backgroundColor = .systemGray
            return UISwipeActionsConfiguration(actions: [restore])
        }
        let archive = UIContextualAction(style: .destructive, title: "Archive") { [weak self] _, _, done in
            self?.app.archive(id)
            done(true)
        }
        archive.image = UIImage(systemName: "archivebox.fill")
        archive.backgroundColor = Palette.secondary
        // Sleep (or, asleep, Wake) beside Archive. A chat under way isn't
        // archived (as the desktop's Archive: stop it first).
        guard activityActions, let vm = sessions[id] else { return UISwipeActionsConfiguration(actions: [archive]) }
        do {
            let underWay = vm.status == .working
            if vm.sleepUntil != nil {
                let wake = UIContextualAction(style: .normal, title: "Wake") { [weak self] _, _, done in
                    self?.app.wake(id)
                    done(true)
                }
                wake.image = UIImage(systemName: "sun.max.fill")
                wake.backgroundColor = .systemOrange
                return UISwipeActionsConfiguration(actions: underWay ? [wake] : [archive, wake])
            }
            let sleep = UIContextualAction(style: .normal, title: "Sleep") { [weak self] _, view, done in
                self?.presentSleepSheet(id, from: view)
                done(true)
            }
            sleep.image = UIImage(systemName: "moon.fill")
            sleep.backgroundColor = .systemIndigo
            return UISwipeActionsConfiguration(actions: underWay ? [sleep] : [archive, sleep])
        }
    }

    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt indexPaths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        if let path = indexPaths.first, case let .header(id) = dataSource.itemIdentifier(for: path), let menu = headerMenu(id) {
            return UIContextMenuConfiguration(identifier: nil, previewProvider: nil) { _ in menu }
        }
        guard let path = indexPaths.first, let id = sessionId(path), let vm = sessions[id] else { return nil }
        let archivedRow = isArchivedRow(path)
        return UIContextMenuConfiguration(identifier: id as NSString, previewProvider: nil) { [weak self] _ in
            guard let self else { return nil }
            if archivedRow {
                return UIMenu(children: [
                    UIAction(title: "Unarchive", image: UIImage(systemName: "tray.and.arrow.up")) { _ in self.app.unarchive(id) },
                    UIAction(title: "Rename…", image: UIImage(systemName: "pencil")) { _ in self.renameChat(id, app: self.app) },
                    self.deleteChatAction(id, app: self.app),
                ])
            }
            // Pin and Sleep belong to Activity, not a project's chats.
            let activity: [UIMenuElement] = self.activityActions ? [
                UIAction(title: vm.pinned ? "Unpin" : "Pin", image: UIImage(systemName: vm.pinned ? "pin.slash" : "pin")) { _ in self.app.setPinned(id, !vm.pinned) },
                self.sleepMenu(vm),
            ] : []
            return UIMenu(children: activity + self.chatMenu(vm, app: self.app))
        }
    }

    // MARK: Sleep

    /// Sleep's times (each saying when it wakes) and Custom…, or, asleep,
    /// Wake.
    func sleepMenu(_ vm: SessionRowVM) -> UIMenuElement {
        let id = vm.id
        if vm.sleepUntil != nil {
            return UIAction(title: "Wake", image: UIImage(systemName: "sun.max")) { [weak self] _ in self?.app.wake(id) }
        }
        var times: [UIMenuElement] = ActivityList.presets(now: Date()).map { p in
            UIAction(title: p.title, subtitle: p.detail) { [weak self] _ in self?.app.sleep(id, until: p.until) }
        }
        times.append(UIMenu(options: .displayInline, children: [
            UIAction(title: "Custom…", image: UIImage(systemName: "calendar")) { [weak self] _ in self?.presentCustomSleep(id) },
        ]))
        return UIMenu(title: "Sleep", image: UIImage(systemName: "moon"), children: times)
    }

    /// The swipe's Sleep: its times as an action sheet.
    func presentSleepSheet(_ id: String, from view: UIView) {
        let sheet = UIAlertController(title: "Sleep Until", message: nil, preferredStyle: .actionSheet)
        for p in ActivityList.presets(now: Date()) {
            sheet.addAction(UIAlertAction(title: "\(p.title) · \(p.detail)", style: .default) { [weak self] _ in self?.app.sleep(id, until: p.until) })
        }
        sheet.addAction(UIAlertAction(title: "Custom…", style: .default) { [weak self] _ in self?.presentCustomSleep(id) })
        sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        sheet.popoverPresentationController?.sourceView = view
        present(sheet, animated: true)
    }

    /// Custom…: the system date picker, from tomorrow at 9.
    func presentCustomSleep(_ id: String) {
        let picker = SleepPickerViewController(title: sessions[id]?.title) { [weak self] until in
            self?.app.sleep(id, until: until)
        }
        present(UINavigationController(rootViewController: picker), animated: true)
    }
}

/// Where a dragged session lands. Pinned keeps a placed order (shared with
/// desktop); Recent is by activity, so only membership moves.
enum SessionDropTarget: Equatable {
    case pinned
    case recent
}

// MARK: Drag between sections

extension SessionListController: UICollectionViewDragDelegate, UICollectionViewDropDelegate {
    func collectionView(_ collectionView: UICollectionView, itemsForBeginning session: UIDragSession, at indexPath: IndexPath) -> [UIDragItem] {
        guard let id = sessionId(indexPath), dropTarget(listSection(of: indexPath)) != nil else { return [] }
        let item = UIDragItem(itemProvider: NSItemProvider())
        item.localObject = id
        return [item]
    }

    func collectionView(_ collectionView: UICollectionView, canHandle session: UIDropSession) -> Bool {
        session.localDragSession?.items.first?.localObject is String
    }

    func collectionView(_ collectionView: UICollectionView, dropSessionDidUpdate session: UIDropSession, withDestinationIndexPath destinationIndexPath: IndexPath?) -> UICollectionViewDropProposal {
        guard let id = session.localDragSession?.items.first?.localObject as? String,
              let path = destinationIndexPath,
              let slot = dropSlot(for: id, at: path)
        else { return UICollectionViewDropProposal(operation: .cancel) }
        // Recent sorts by activity: a drag within it has nothing to place.
        if slot.target == .recent, currentTarget(of: id) == .recent {
            return UICollectionViewDropProposal(operation: .cancel)
        }
        return UICollectionViewDropProposal(operation: .move, intent: .insertAtDestinationIndexPath)
    }

    func collectionView(_ collectionView: UICollectionView, performDropWith coordinator: UICollectionViewDropCoordinator) {
        guard let item = coordinator.items.first,
              let id = item.dragItem.localObject as? String,
              let path = coordinator.destinationIndexPath,
              let slot = dropSlot(for: id, at: path)
        else { return }
        let source = currentTarget(of: id)
        if slot.target == .recent, source == .recent { return }
        // Show the row in its slot at once; the synced state follows. A
        // folded destination opens, as desktop's does, to show where it went.
        var snapshot = dataSource.snapshot()
        let section = snapshot.sectionIdentifiers[path.section]
        if isCollapsed(section) { setCollapsed(section, false) }
        snapshot.deleteItems([.session(id)])
        if slot.index > 0 {
            snapshot.insertItems([.session(id)], afterItem: slot.items[slot.index - 1])
        } else if let first = slot.items.first {
            snapshot.insertItems([.session(id)], beforeItem: first)
        } else {
            snapshot.appendItems([.session(id)], toSection: section)
        }
        dataSource.apply(snapshot, animatingDifferences: false)
        coordinator.drop(item.dragItem, toItemAt: IndexPath(item: slot.index, section: path.section))
        UISelectionFeedbackGenerator().selectionChanged()

        switch slot.target {
        case .pinned:
            if source == .pinned {
                app.movePin(id, after: slot.after, before: slot.before)
            } else {
                app.pin(id, after: slot.after, before: slot.before)
            }
        case .recent:
            if source == .pinned { app.setPinned(id, false) }
        }
    }

    private func listSection(of path: IndexPath) -> String {
        dataSource.snapshot().sectionIdentifiers[path.section]
    }

    /// The destination the session sits in now.
    private func currentTarget(of id: String) -> SessionDropTarget? {
        dataSource.snapshot().sectionIdentifier(containingItem: .session(id)).flatMap(dropTarget)
    }

    /// Where a drop at `path` places `id`: the destination, the section's items
    /// without the dragged row, the row's index among them, and its session
    /// neighbours. A drop above a section's header has no slot.
    private func dropSlot(for id: String, at path: IndexPath) -> (target: SessionDropTarget, items: [Item], index: Int, after: String?, before: String?)? {
        let snapshot = dataSource.snapshot()
        guard path.section < snapshot.numberOfSections else { return nil }
        let section = snapshot.sectionIdentifiers[path.section]
        guard let target = dropTarget(section) else { return nil }
        let items = snapshot.itemIdentifiers(inSection: section).filter { $0 != .session(id) }
        let headed: Int
        if case .header = items.first { headed = 1 } else { headed = 0 }
        if path.item < headed { return nil }
        let index = min(path.item, items.count)
        func session(_ item: Item?) -> String? {
            if case let .session(id) = item { return id }
            return nil
        }
        let after = index > 0 ? session(items[index - 1]) : nil
        let before = index < items.count ? session(items[index]) : nil
        return (target, items, index, after, before)
    }
}

/// Front page, laid out like the desktop sidebar: Pinned and the user's
/// sections inline as foldable groups, then everything else under Recent.
/// Fold state persists; long-press a header for its actions.
class SessionsViewController: SessionListController {
    /// Search from the iPad sidebar: non-empty shows matches instead of sections.
    var query = "" { didSet { if query != oldValue { reload(animated: true) } } }

    /// The chat wallpaper behind the top of the front page. Fixed behind the
    /// list, fading out as the list scrolls up over it.
    private let wallpaper = WallpaperView()
    /// Until the registry first arrives the list is empty because nothing
    /// has loaded, not because there are no chats.
    private let loading = UIActivityIndicatorView(style: .medium)

    override func viewDidLoad() {
        collapsible = true
        movesAcrossSections = true
        super.viewDidLoad()
        let backdrop = UIView()
        backdrop.addSubview(wallpaper)
        loading.translatesAutoresizingMaskIntoConstraints = false
        loading.hidesWhenStopped = true
        backdrop.addSubview(loading)
        NSLayoutConstraint.activate([
            loading.centerXAnchor.constraint(equalTo: backdrop.centerXAnchor),
            loading.centerYAnchor.constraint(equalTo: backdrop.centerYAnchor),
        ])
        collectionView.backgroundView = backdrop
        title = "Sessions"
        navigationItem.largeTitleDisplayMode = .always
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: optionsMenu())
        navigationItem.rightBarButtonItem?.accessibilityIdentifier = "sessions-options"
    }

    /// iPhone: search lives in the bottom bar (the system's toolbar-integrated
    /// field) and filters this list in place.
    func enableToolbarSearch() {
        guard navigationItem.searchController == nil else { return }
        let search = UISearchController(searchResultsController: nil)
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        search.searchBar.placeholder = "Search"
        navigationItem.searchController = search
        navigationItem.preferredSearchBarPlacement = .integrated
        navigationItem.searchBarPlacementAllowsToolbarIntegration = true
        // Otherwise the list opens scrolled past the large title.
        navigationItem.hidesSearchBarWhenScrolling = false
    }

    func activateSearch() {
        navigationItem.searchController?.isActive = true
        DispatchQueue.main.async { [weak self] in
            self?.navigationItem.searchController?.searchBar.becomeFirstResponder()
        }
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        if !query.trimmingCharacters(in: .whitespaces).isEmpty {
            return [("results", nil, [], app.search(query))]
        }
        switch app.sessionsView {
        case .chats: return activitySections()
        // Its own page stands over this list.
        case .projects: return []
        }
    }

    /// Projects: its own page, over this list while that view is on and
    /// nothing is searched.
    private var projectsPage: ProjectsViewController?

    override func reload(animated: Bool) {
        super.reload(animated: animated)
        if app.synced { loading.stopAnimating() } else { loading.startAnimating() }
        let wanted = app.sessionsView == .projects && query.trimmingCharacters(in: .whitespaces).isEmpty
        if wanted, projectsPage == nil {
            let page = ProjectsViewController(app: app)
            page.openChat = { [weak self] id in self?.openSession(id) }
            page.openCheckout = { [weak self] project, checkout in
                guard let self else { return }
                self.navigationController?.pushViewController(ProjectChatsViewController(app: self.app, project: project, checkout: checkout), animated: true)
            }
            page.newChat = { [weak self] projectId, cwd in
                guard let self, let router = self.router else { return }
                self.app.lastDraft.projectId = projectId
                self.app.newChatIn = cwd.map { (projectId, $0) }
                self.app.lastDraft.hostId = nil
                self.app.lastDraft.branch = nil
                router.presentNewSession(prompt: nil)
            }
            addChild(page)
            page.view.frame = view.bounds
            page.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
            view.addSubview(page.view)
            page.didMove(toParent: self)
            projectsPage = page
            // The large title follows the page's own scrolling.
            setContentScrollView(page.collectionView, for: .top)
        } else if !wanted, let page = projectsPage {
            page.willMove(toParent: nil)
            page.view.removeFromSuperview()
            page.removeFromParent()
            projectsPage = nil
            setContentScrollView(collectionView, for: .top)
        }
    }

    /// Activity: Pinned above, then one list (every other chat, sections
    /// and all, the one back last first), then its shelves at the foot,
    /// each folded until opened: Working, Asleep, Archived. Grouped by
    /// project (t3code's), one list per project instead, no shelves. The
    /// project filter keeps one project's chats either way.
    private func activitySections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        var out: [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] = []
        let now = Date()
        let owner = app.projectByChat()
        // A filter on a project that's gone shows everything.
        let filter = app.activityProject.flatMap { id in app.frontPage.projects.first { $0.id == id } }
        func keep(_ vm: SessionRowVM) -> Bool {
            guard let filter else { return true }
            if let tree = owner[vm.id] { return tree.id == filter.id }
            // Archived chats sit in no checkout's list: by their project's name.
            return vm.hasProject && vm.projectName == filter.name
        }
        let pinned = app.sessions(inFolder: "pinned").filter(keep)
        var rows = app.frontPage.sessions
        for f in app.frontPage.folders where f.id != "pinned" {
            rows += app.sessions(inFolder: f.id)
        }
        rows = rows.filter(keep)
        if app.activityGroupByProject {
            // Every chat, Pinned and asleep too, the one back last first; the
            // projects in the order their latest chat came back.
            var seen = Set<String>()
            let unique: [SessionRowVM] = (pinned + rows).filter { seen.insert($0.id).inserted }
            let ranked: [(rank: TimeInterval, vm: SessionRowVM)] = unique.map { (ActivityList.rank($0, shelf: nil), ActivityList.decorate($0, now: now)) }
            let all: [SessionRowVM] = ranked.sorted { a, b in a.rank != b.rank ? a.rank > b.rank : a.vm.id < b.vm.id }.map { $0.vm }
            var groups: [(id: String, name: String, chats: [SessionRowVM])] = []
            for vm in all {
                let tree = owner[vm.id]
                let id = "activity.project." + (tree?.id ?? "none")
                if let k = groups.firstIndex(where: { $0.id == id }) {
                    groups[k].chats.append(vm)
                } else {
                    groups.append((id, tree?.name ?? "No Project", [vm]))
                }
            }
            return groups.map { ($0.id, $0.name, [], $0.chats) }
        }
        if !pinned.isEmpty {
            out.append(("pinned", "Pinned", [], pinned.map { ActivityList.decorate($0, now: now) }))
        }
        let page = ActivityList.page(rows, workingShelf: app.activityWorkingShelf, now: now)
        out.append((ActivityList.Section.list, "Activity", [], page.list))
        if !page.working.isEmpty { out.append((ActivityList.Section.working, "Working", [], page.working)) }
        if !page.asleep.isEmpty { out.append((ActivityList.Section.asleep, "Asleep", [], page.asleep)) }
        let archived = app.archived.filter(keep)
        if !archived.isEmpty {
            archivedTotal = archived.count
            out.append((ActivityList.Section.archived, "Archived", [], Array(archived.prefix(archivedShown))))
        }
        return out
    }

    override var compactRows: Bool { app.compactRows }

    override func headerIcon(_ id: String) -> UIImage? {
        guard id.hasPrefix("activity.project."), let tree = app.frontPage.projects.first(where: { "activity.project." + $0.id == id }) else { return nil }
        let side = (18 * TypeScale.factor).rounded()
        return tree.projectIds[tree.main.id].flatMap { ProjectArt.tile(spaceId: $0, side: side) }
            ?? ProjectTile.image(name: tree.name, colorIndex: tree.colorIndex, side: side)
    }

    /// Activity's Archived shows its first 10, then 25 more a tap (the
    /// desktop's shelf).
    private var archivedShown = 10
    private var archivedTotal = 0

    override func moreCount(_ id: String) -> Int {
        guard id == ActivityList.Section.archived, app.sessionsView == .chats, !isCollapsed(id) else { return 0 }
        return max(0, archivedTotal - archivedShown)
    }

    override func showMore(_ id: String) {
        archivedShown += 25
        reload(animated: true)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        // Desktop hero: 72% of the viewport, at most 760pt.
        wallpaper.frame = CGRect(x: 0, y: 0, width: view.bounds.width, height: min(view.bounds.height * 0.72, 760))
    }

    override func listDidScroll(_ scrollView: UIScrollView) {
        let travel = scrollView.contentOffset.y + scrollView.adjustedContentInset.top
        wallpaper.scrollFade = 1 - max(0, travel) / max(1, wallpaper.bounds.height * 0.6)
    }

    override func dropTarget(_ id: String) -> SessionDropTarget? {
        // Search results and project groups aren't places a session is put.
        guard query.trimmingCharacters(in: .whitespaces).isEmpty else { return nil }
        // Both views order themselves: only Pinned is a place a session is put.
        return app.sessionsView == .chats && id == "pinned" ? .pinned : nil
    }

    override func headerMenu(_ id: String) -> UIMenu? {
        guard id == "pinned", app.frontPage.folders.contains(where: { $0.id == id }) else { return nil }
        return UIMenu(children: [
            UIAction(title: "Open", image: UIImage(systemName: "arrow.up.right")) { [weak self] _ in self?.openFolder(id) },
            UIAction(title: "Reorder…", image: UIImage(systemName: "arrow.up.arrow.down")) { [weak self] _ in self?.openFolder(id) },
        ])
    }

    /// The view (Chats or Projects), and in Chats its project filter and
    /// grouping (rebuilt on each open, as Settings switches the view too).
    func optionsMenu() -> UIMenu {
        UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] done in
                guard let app = self?.app else { return done([]) }
                var items: [UIMenuElement] = [UIMenu(title: "Activity", options: .displayInline, children: AppModel.SessionsView.allCases.map { view in
                    UIAction(title: view.title, image: UIImage(systemName: view.symbol), state: app.sessionsView == view ? .on : .off) { _ in
                        app.sessionsView = view
                    }
                })]
                if app.sessionsView == .chats {
                    let current = app.activityProject.flatMap { id in app.frontPage.projects.first { $0.id == id } }
                    let projects = [UIAction(title: "All Projects", image: UIImage(systemName: "square.stack"), state: current == nil ? .on : .off) { _ in
                        app.activityProject = nil
                    }] + app.frontPage.projects.map { p in
                        // The repository's artwork once loaded, else its monogram.
                        let art = p.projectIds[p.main.id].flatMap { ProjectArt.tile(spaceId: $0, side: 20) }
                        return UIAction(title: p.name, image: art ?? ProjectTile.image(name: p.name, colorIndex: p.colorIndex, side: 20), state: current?.id == p.id ? .on : .off) { _ in
                            app.activityProject = p.id
                        }
                    }
                    let groupBy = [(false, "None", "list.bullet"), (true, "Project", "folder")].map { on, title, symbol in
                        UIAction(title: title, image: UIImage(systemName: symbol), state: app.activityGroupByProject == on ? .on : .off) { _ in
                            app.activityGroupByProject = on
                        }
                    }
                    items.append(UIMenu(options: .displayInline, children: [
                        UIMenu(title: "Project", subtitle: current?.name ?? "All Projects", image: UIImage(systemName: "line.3.horizontal.decrease"), children: projects),
                        UIMenu(title: "Group By", subtitle: app.activityGroupByProject ? "Project" : "None", image: UIImage(systemName: "rectangle.3.group"), children: groupBy),
                        UIAction(title: "Compact", image: UIImage(systemName: "rectangle.compress.vertical"), state: app.compactRows ? .on : .off) { _ in
                            app.compactRows.toggle()
                        },
                    ]))
                }
                done(items)
            },
        ])
    }
}

/// One folder's sessions (Pinned, Archived).
final class FolderViewController: SessionListController {
    private let folder: FolderRowVM

    init(app: AppModel, folder: FolderRowVM) {
        self.folder = folder
        super.init(app: app)
        // The front page's bottom bar is the front page's alone.
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        archivedRows = folder.id == "archived"
        super.viewDidLoad()
        title = folder.name
        navigationItem.largeTitleDisplayMode = .always
        if folder.id == "pinned" {
            // Pins are an ordered list: drag to reorder (synced to desktop).
            navigationItem.rightBarButtonItem = editButtonItem
            reorderable = true
            reload(animated: false)
            dataSource.reorderingHandlers.canReorderItem = { _ in true }
            dataSource.reorderingHandlers.didReorder = { [weak self] tx in self?.pinsReordered(tx) }
        }
    }

    override func setEditing(_ editing: Bool, animated: Bool) {
        super.setEditing(editing, animated: animated)
        collectionView.isEditing = editing
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("sessions", nil, [], app.sessions(inFolder: folder.id))]
    }

    private func pinsReordered(_ tx: NSDiffableDataSourceTransaction<String, Item>) {
        let ids = tx.finalSnapshot.itemIdentifiers.compactMap { item -> String? in
            if case let .session(id) = item { return id }
            return nil
        }
        for change in tx.difference.insertions {
            guard case let .insert(offset, item, _) = change, case let .session(id) = item else { continue }
            let after = offset > 0 ? ids[offset - 1] : nil
            let before = offset + 1 < ids.count ? ids[offset + 1] : nil
            app.movePin(id, after: after, before: before)
        }
    }

}

extension SessionsViewController: UISearchResultsUpdating {
    func updateSearchResults(for searchController: UISearchController) {
        query = searchController.searchBar.text ?? ""
    }
}

/// Search tab: live results as you type (matching runs in the core).
final class SearchViewController: SessionListController, UISearchResultsUpdating {
    private var query = ""

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Search"
        navigationItem.largeTitleDisplayMode = .always
        let search = UISearchController(searchResultsController: nil)
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        search.searchBar.placeholder = "Sessions, projects, branches"
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = false
    }

    func updateSearchResults(for searchController: UISearchController) {
        query = searchController.searchBar.text ?? ""
        reload(animated: true)
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("results", nil, [], app.search(query))]
    }
}

extension UIViewController {
    /// A chat's own actions (every list): Rename, Stop Waiting, Archive, Delete.
    func chatMenu(_ vm: SessionRowVM, app: AppModel) -> [UIMenuElement] {
        let id = vm.id
        var items: [UIMenuElement] = [UIAction(title: "Rename…", image: UIImage(systemName: "pencil")) { [weak self] _ in self?.renameChat(id, app: app) }]
        if vm.canStopWaiting {
            items.append(UIAction(title: "Stop Waiting", image: UIImage(systemName: "xmark.circle")) { _ in app.stopWaiting(id) })
        }
        items.append(vm.status == .working
            ? UIAction(title: "Archive", subtitle: "Stop it first: its agent is still working", image: UIImage(systemName: "archivebox"), attributes: .disabled) { _ in }
            : UIAction(title: "Archive", image: UIImage(systemName: "archivebox")) { _ in app.archive(id) })
        items.append(deleteChatAction(id, app: app))
        return items
    }

    func deleteChatAction(_ id: String, app: AppModel) -> UIAction {
        UIAction(title: "Delete", image: UIImage(systemName: "trash"), attributes: .destructive) { [weak self, weak app] _ in
            self?.confirmDelete(app: app, id) {}
        }
    }

    func renameChat(_ id: String, app: AppModel) {
        let alert = UIAlertController(title: "Rename Session", message: nil, preferredStyle: .alert)
        alert.addTextField { $0.text = app.row(id)?.title }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Rename", style: .default) { [weak app, weak alert] _ in
            let title = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            if !title.isEmpty { app?.rename(id, title) }
        })
        present(alert, animated: true)
    }
}
