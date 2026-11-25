import UIKit

/// The Sessions page's Projects view, as Shepherd's phone app lays out its
/// workspaces: a Projects card (each repository, its worktrees folded under
/// it, and New project) and a Chats card (every chat, most recent first).
/// A repository opens its own checkout's chats (main, develop…), a worktree
/// its own; with none yet, a new chat there. A chat opens itself.
final class ProjectsViewController: UIViewController, UICollectionViewDelegate {
    enum Item: Hashable {
        case project(String)
        case checkout(String)
        case newProject
        case chat(String)
    }

    private enum Section: String {
        case projects = "Projects"
        case chats = "Chats"
    }

    private let app: AppModel
    var openChat: (String) -> Void = { _ in }
    /// One checkout's chats: the repository's own (main, develop…) or a
    /// worktree's.
    var openCheckout: (AppModel.ProjectTreeVM, FolderRowVM) -> Void = { _, _ in }
    /// A new chat in this project (a checkout with none yet).
    var newChat: (_ projectId: String, _ cwd: String?) -> Void = { _, _ in }

    private(set) var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Section, Item>!
    private var projects: [String: AppModel.ProjectTreeVM] = [:]
    private var checkouts: [String: FolderRowVM] = [:]
    private var chats: [String: SessionRowVM] = [:]
    private var token: AnyObject?
    /// Repositories folded shut, kept on this phone.
    private var folded = Set(UserDefaults.standard.stringArray(forKey: "projectsFolded") ?? [])

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = .clear
        config.headerMode = .supplementary
        // The system separator is too loud on the dark card: the app's hairline.
        config.separatorConfiguration.color = Palette.hairline
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Item> { [weak self] cell, _, item in
            self?.configure(cell, item)
        }
        let header = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, path in
            guard let self else { return }
            let section = self.dataSource.sectionIdentifier(for: path.section)
            // As the Chats view's section headers (Activity, Working).
            var c = UIListContentConfiguration.groupedHeader()
            c.text = section?.rawValue
            c.textProperties.font = Fonts.ui(.sansSemibold, TypeScale.size(13.5))
            c.textProperties.color = Palette.secondary
            c.textProperties.transform = .none
            view.contentConfiguration = c
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, item in
            cv.dequeueConfiguredReusableCell(using: cell, for: path, item: item)
        }
        dataSource.supplementaryViewProvider = { cv, _, path in
            cv.dequeueConfiguredReusableSupplementary(using: header, for: path)
        }
        dataSource.sectionSnapshotHandlers.willExpandItem = { [weak self] item in self?.setFolded(item, false) }
        dataSource.sectionSnapshotHandlers.willCollapseItem = { [weak self] item in self?.setFolded(item, true) }
        token = app.observe { [weak self] in self?.reload() }
        reload()
    }

    private func setFolded(_ item: Item, _ shut: Bool) {
        guard case let .project(id) = item else { return }
        if shut { folded.insert(id) } else { folded.remove(id) }
        UserDefaults.standard.set(Array(folded), forKey: "projectsFolded")
    }

    private func reload() {
        let trees = app.frontPage.projects
        projects = Dictionary(trees.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        checkouts = Dictionary(trees.flatMap { [$0.main] + $0.worktrees }.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        let all = app.allChats
        chats = Dictionary(all.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })

        var outer = NSDiffableDataSourceSnapshot<Section, Item>()
        outer.appendSections([.projects, .chats])
        // Rows draw from the dictionaries: redraw them all on every change.
        let old = Set(dataSource.snapshot().itemIdentifiers)
        dataSource.apply(outer, animatingDifferences: false)

        var tree = NSDiffableDataSourceSectionSnapshot<Item>()
        for p in trees {
            tree.append([.project(p.id)])
            if !p.worktrees.isEmpty {
                tree.append(p.worktrees.map { .checkout($0.id) }, to: .project(p.id))
                if !folded.contains(p.id) { tree.expand([.project(p.id)]) }
            }
        }
        tree.append([.newProject])
        dataSource.apply(tree, to: .projects, animatingDifferences: false)
        var list = NSDiffableDataSourceSectionSnapshot<Item>()
        list.append(needsYouFirst(all).map { .chat($0.id) })
        dataSource.apply(list, to: .chats, animatingDifferences: false)

        var snapshot = dataSource.snapshot()
        snapshot.reconfigureItems(snapshot.itemIdentifiers.filter(old.contains))
        dataSource.apply(snapshot, animatingDifferences: false)
    }

    private func configure(_ cell: UICollectionViewListCell, _ item: Item) {
        var c = UIListContentConfiguration.cell()
        // Compact rows, as the desktop sidebar's.
        c.textProperties.font = Fonts.ui(.sans, TypeScale.size(16.5))
        c.textProperties.color = Palette.text
        c.textProperties.numberOfLines = 1
        c.imageToTextPadding = 10
        c.directionalLayoutMargins.top = 7
        c.directionalLayoutMargins.bottom = 7
        let side = (26 * TypeScale.factor).rounded()
        c.imageProperties.reservedLayoutSize = CGSize(width: side, height: side)
        var accessories: [UICellAccessory] = []
        switch item {
        case let .project(id):
            guard let p = projects[id] else { break }
            c.text = p.name
            if let art = ProjectArt.image(client: app.client, device: p.deviceId, root: p.path, loaded: { [weak self] in self?.reload() }) {
                c.image = art
                c.imageProperties.maximumSize = CGSize(width: side, height: side)
                c.imageProperties.cornerRadius = 6
            } else {
                c.image = ProjectTile.image(name: p.name, colorIndex: p.colorIndex, side: side)
            }
            accessories.append(status(checkout: p.main.id))
            if !p.worktrees.isEmpty {
                // `.cell`: the row opens the project; only the chevron folds it.
                accessories.append(.outlineDisclosure(options: .init(style: .cell, tintColor: Palette.tertiary)))
            }
        case let .checkout(id):
            guard let w = checkouts[id] else { break }
            c.text = w.name
            c.image = UIImage(systemName: "arrow.triangle.branch")
            c.imageProperties.tintColor = Palette.secondary
            c.imageProperties.maximumSize = CGSize(width: 18, height: 18)
            if w.count == 0 {
                // No chats yet: dimmed, as the desktop draws it.
                c.textProperties.color = Palette.text.withAlphaComponent(0.45)
                c.imageProperties.tintColor = Palette.secondary.withAlphaComponent(0.45)
            } else {
                accessories.append(status(checkout: id))
            }
        case .newProject:
            c.text = "New project"
            c.textProperties.color = Palette.accent
            c.image = UIImage(systemName: "plus")
            c.imageProperties.tintColor = Palette.accent
        case let .chat(id):
            guard let vm = chats[id] else { break }
            c.text = vm.title
            c.image = BrandMarks.image(for: vm.harness ?? "claude-code", side: 20)
            accessories.append(glyph(SessionCell.corner(vm)?.glyph ?? .dot(StatusTone.idle)))
        }
        cell.contentConfiguration = c
        cell.accessories = accessories
        cell.indentationWidth = side
        var bg = UIBackgroundConfiguration.listGroupedCell()
        bg.backgroundColor = Palette.elevated
        cell.backgroundConfiguration = bg
    }

    /// A checkout's glyph: its most pressing chat's, else idle.
    private func status(checkout id: String) -> UICellAccessory {
        glyph(SessionCell.urgentGlyph(app.sessions(inFolder: id)) ?? .dot(StatusTone.idle))
    }

    private func glyph(_ kind: StatusGlyph.Kind) -> UICellAccessory {
        // A sized frame: with none the check's path (drawn to bounds) collapses to a dot.
        let view = StatusGlyph(kind)
        view.frame = CGRect(x: 0, y: 0, width: 14, height: 14)
        return .customView(configuration: .init(customView: view, placement: .trailing(), reservedLayoutWidth: .custom(16)))
    }

    /// Its chats, or, with none yet, a new one there.
    private func open(_ project: AppModel.ProjectTreeVM, _ checkout: FolderRowVM) {
        guard app.sessions(inFolder: checkout.id).isEmpty else { return openCheckout(project, checkout) }
        if let projectId = project.projectIds[checkout.id] {
            newChat(projectId, nil)
        } else if let path = checkout.path, let projectId = project.projectIds[project.main.id] {
            // A worktree that isn't a project of its own: the repository's, run in its folder.
            newChat(projectId, path)
        } else {
            openCheckout(project, checkout)
        }
    }

    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt paths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        guard let path = paths.first, let item = dataSource.itemIdentifier(for: path) else { return nil }
        switch item {
        case let .project(id):
            guard let p = projects[id] else { return nil }
            return UIContextMenuConfiguration(actionProvider: { [weak self] _ in
                UIMenu(children: [
                    UIAction(title: "New Worktree", image: UIImage(systemName: "arrow.triangle.branch")) { _ in
                        self?.run("Couldn't create the worktree") { try await self?.app.createWorktree(p) }
                    },
                    UIAction(title: "Remove Project", image: UIImage(systemName: "trash"), attributes: .destructive) { _ in
                        self?.confirm("Remove \(p.name)?", message: "It leaves Commutator; its folder stays on disk.", action: "Remove") {
                            self?.run("Couldn't remove the project") { try await MainActor.run { try self?.app.removeProject(p) } }
                        }
                    },
                ])
            })
        case let .checkout(id):
            guard let w = checkouts[id], let p = projects.values.first(where: { $0.worktrees.contains(w) }) else { return nil }
            return UIContextMenuConfiguration(actionProvider: { [weak self] _ in
                var actions = [UIAction(title: "Rename", image: UIImage(systemName: "pencil")) { _ in self?.rename(w) }]
                if w.path != nil {
                    actions.append(UIAction(title: "Delete Worktree", image: UIImage(systemName: "trash"), attributes: .destructive) { _ in
                        self?.confirm("Delete \(w.name)?", message: "Its folder is removed from disk (git worktree remove).", action: "Delete") {
                            self?.run("Couldn't delete the worktree") { try await self?.app.deleteWorktree(p, w) }
                        }
                    })
                }
                return UIMenu(children: actions)
            })
        case let .chat(id):
            guard let vm = chats[id] else { return nil }
            // As a project's chats: no Pin or Sleep (Activity's).
            return UIContextMenuConfiguration(identifier: id as NSString, previewProvider: nil) { [weak self] _ in
                guard let self else { return nil }
                return UIMenu(children: self.chatMenu(vm, app: self.app))
            }
        case .newProject:
            return nil
        }
    }

    private func rename(_ w: FolderRowVM) {
        let alert = UIAlertController(title: "Rename Worktree", message: nil, preferredStyle: .alert)
        alert.addTextField { $0.text = w.name; $0.clearButtonMode = .whileEditing }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Rename", style: .default) { [weak self, weak alert] _ in
            self?.app.renameWorktree(w.id, to: alert?.textFields?.first?.text)
        })
        present(alert, animated: true)
    }

    private func confirm(_ title: String, message: String, action: String, _ go: @escaping () -> Void) {
        let alert = UIAlertController(title: title, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: action, style: .destructive) { _ in go() })
        present(alert, animated: true)
    }

    /// A host call; a failure shows as a toast.
    private func run(_ failure: String, _ work: @escaping () async throws -> Void) {
        Task { @MainActor [weak self] in
            do { try await work() } catch {
                Toast.show("\(failure): \(error.localizedDescription)", in: self?.view.window)
            }
        }
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        collectionView.deselectItem(at: path, animated: true)
        guard let item = dataSource.itemIdentifier(for: path) else { return }
        switch item {
        case let .project(id):
            guard let p = projects[id] else { return }
            open(p, p.main)
        case let .checkout(id):
            guard let w = checkouts[id], let p = projects.values.first(where: { $0.worktrees.contains(w) }) else { return }
            open(p, w)
        case .newProject:
            present(UINavigationController(rootViewController: NewProjectViewController(app: app)), animated: true)
        case let .chat(id):
            openChat(id)
        }
    }
}

/// The order chats run in, as the desktop's Agents (t3code's): what waits
/// on you (input, a failure), then finished and not seen (or woke), then
/// work under way, then the rest; the one back last first within each.
func needsYouFirst(_ chats: [SessionRowVM]) -> [SessionRowVM] {
    func rank(_ vm: SessionRowVM) -> Int {
        switch vm.status {
        case .awaiting, .errored: 0
        case .woke: 1
        case .completed: vm.unseen ? 1 : 3
        case .working: 2
        case .idle: 3
        }
    }
    // Stable: within a rank, the incoming most-recent-first order.
    return chats.enumerated().sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }.map(\.element)
}

/// One checkout's chats, opened from Projects: the repository's own (main,
/// develop…) or a worktree's. The toolbar toggles Compact (kept on this phone).
final class ProjectChatsViewController: SessionListController {
    private let project: AppModel.ProjectTreeVM
    private let checkout: FolderRowVM

    /// Shared with Chats (the desktop's one sidebar Compact).
    private var compact: Bool {
        get { app.compactRows }
        set { app.compactRows = newValue }
    }

    override func reload(animated: Bool) {
        super.reload(animated: animated)
        updateCompactButton()
    }

    private lazy var compactItem = UIBarButtonItem(primaryAction: UIAction { [weak self] _ in self?.compact.toggle() })

    /// A new chat here: this checkout's project, or, a worktree that isn't
    /// one, the repository's run in its folder.
    private func newChat() {
        let own = project.projectIds[checkout.id]
        guard let projectId = own ?? project.projectIds[project.main.id], let router else { return }
        app.lastDraft.projectId = projectId
        app.lastDraft.hostId = nil
        app.lastDraft.branch = nil
        app.newChatIn = own == nil ? checkout.path.map { (projectId, $0) } : nil
        router.presentNewSession(prompt: nil)
    }

    /// The toolbar's Compact toggle: its icon says what a tap does.
    private func updateCompactButton() {
        let item: UIBarButtonItem? = compactItem
        item?.image = UIImage(systemName: compact ? "rectangle.expand.vertical" : "rectangle.compress.vertical")
        item?.accessibilityLabel = "Compact"
        item?.accessibilityValue = compact ? "On" : "Off"
    }

    override var compactRows: Bool { compact }
    override var activityActions: Bool { false }
    override var showsProject: Bool { false }

    init(app: AppModel, project: AppModel.ProjectTreeVM, checkout: FolderRowVM) {
        self.project = project
        self.checkout = checkout
        super.init(app: app)
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = checkout.id == project.main.id ? project.name : checkout.name
        navigationItem.largeTitleDisplayMode = .always
        let newChat = UIBarButtonItem(image: UIImage(systemName: "square.and.pencil"), primaryAction: UIAction { [weak self] _ in self?.newChat() })
        newChat.accessibilityLabel = "New Chat"
        // New Chat to the left of the toggle, as the Sessions page's compose sits.
        navigationItem.rightBarButtonItems = [compactItem, newChat]
        updateCompactButton()
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("chats", nil, [], needsYouFirst(app.sessions(inFolder: checkout.id)))]
    }
}
