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
    var newChat: (_ projectId: String) -> Void = { _ in }

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
        list.append(all.map { .chat($0.id) })
        dataSource.apply(list, to: .chats, animatingDifferences: false)

        var snapshot = dataSource.snapshot()
        snapshot.reconfigureItems(snapshot.itemIdentifiers.filter(old.contains))
        dataSource.apply(snapshot, animatingDifferences: false)
    }

    private func configure(_ cell: UICollectionViewListCell, _ item: Item) {
        var c = UIListContentConfiguration.cell()
        c.textProperties.font = Fonts.ui(.sans, TypeScale.size(17))
        c.textProperties.color = Palette.text
        c.textProperties.numberOfLines = 1
        c.imageToTextPadding = 12
        var accessories: [UICellAccessory] = []
        switch item {
        case let .project(id):
            guard let p = projects[id] else { break }
            c.text = p.name
            c.image = ProjectTile.image(name: p.name, colorIndex: p.colorIndex, side: 28)
            if !p.worktrees.isEmpty {
                // `.cell`: the row opens the project; only the chevron folds it.
                accessories.append(.outlineDisclosure(options: .init(style: .cell, tintColor: Palette.tertiary)))
            } else {
                accessories.append(status(checkout: p.main.id))
            }
        case let .checkout(id):
            guard let w = checkouts[id] else { break }
            c.text = w.name
            c.image = UIImage(systemName: "arrow.triangle.branch")
            c.imageProperties.tintColor = Palette.secondary
            c.imageProperties.maximumSize = CGSize(width: 20, height: 20)
            accessories.append(status(checkout: id))
        case .newProject:
            c.text = "New project"
            c.textProperties.color = Palette.accent
            c.image = UIImage(systemName: "plus")
            c.imageProperties.tintColor = Palette.accent
        case let .chat(id):
            guard let vm = chats[id] else { break }
            c.text = vm.title
            c.image = BrandMarks.image(for: vm.harness ?? "claude-code", side: 24)
            accessories.append(glyph(SessionCell.corner(vm)?.glyph ?? .dot(StatusTone.idle)))
        }
        cell.contentConfiguration = c
        cell.accessories = accessories
        cell.indentationWidth = 28
        var bg = UIBackgroundConfiguration.listGroupedCell()
        bg.backgroundColor = Palette.elevated
        cell.backgroundConfiguration = bg
    }

    /// A checkout's dot: its most pressing chat's, else idle.
    private func status(checkout id: String) -> UICellAccessory {
        let list = app.sessions(inFolder: id)
        let order: [SessionRowVM.Status] = [.working, .awaiting, .errored, .woke]
        let urgent = order.lazy.compactMap { s in list.first { $0.status == s } }.first
        return glyph(urgent.flatMap { SessionCell.corner($0)?.glyph } ?? .dot(StatusTone.idle))
    }

    private func glyph(_ kind: StatusGlyph.Kind) -> UICellAccessory {
        .customView(configuration: .init(customView: StatusGlyph(kind), placement: .trailing(), reservedLayoutWidth: .custom(16)))
    }

    /// Its chats, or, with none yet, a new one there.
    private func open(_ project: AppModel.ProjectTreeVM, _ checkout: FolderRowVM) {
        if app.sessions(inFolder: checkout.id).isEmpty, let projectId = project.projectIds[checkout.id] {
            newChat(projectId)
        } else {
            openCheckout(project, checkout)
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

/// How a project's chats run (inside each checkout when grouped), as
/// Shepherd's agents sort.
enum ChatSort: String, CaseIterable {
    /// The one back last first.
    case recent
    /// What needs you first: input, a failure, a wake-up, then work
    /// under way, then the rest; the one back last first within each.
    case status
    /// By title, A to Z.
    case name

    var title: String {
        switch self {
        case .recent: "Recent"
        case .status: "Needs You"
        case .name: "Name"
        }
    }

    func sorted(_ chats: [SessionRowVM]) -> [SessionRowVM] {
        switch self {
        case .recent:
            return chats
        case .status:
            func rank(_ vm: SessionRowVM) -> Int {
                switch vm.status {
                case .awaiting: 0
                case .errored: 1
                case .woke: 2
                case .working: 3
                case .completed: vm.unseen ? 4 : 5
                case .idle: 5
                }
            }
            // Stable: within a rank, the incoming most-recent-first order.
            return chats.enumerated().sorted { (rank($0.element), $0.offset) < (rank($1.element), $1.offset) }.map(\.element)
        case .name:
            return chats.sorted { $0.title.localizedStandardCompare($1.title) == .orderedAscending }
        }
    }
}

/// One checkout's chats, opened from Projects: the repository's own (main,
/// develop…) or a worktree's. "…" sorts them (kept on this phone).
final class ProjectChatsViewController: SessionListController {
    private let project: AppModel.ProjectTreeVM
    private let checkout: FolderRowVM

    private var sort = UserDefaults.standard.string(forKey: "projectChatsSort").flatMap(ChatSort.init) ?? .recent {
        didSet {
            UserDefaults.standard.set(sort.rawValue, forKey: "projectChatsSort")
            reload(animated: true)
        }
    }

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
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: UIMenu(children: [
            UIDeferredMenuElement.uncached { [weak self] done in
                guard let self else { return done([]) }
                done([UIMenu(title: "Sort", options: .displayInline, children: ChatSort.allCases.map { sort in
                    UIAction(title: sort.title, state: self.sort == sort ? .on : .off) { [weak self] _ in self?.sort = sort }
                })])
            },
        ]))
        navigationItem.rightBarButtonItem?.accessibilityLabel = "Chats options"
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("chats", nil, [], sort.sorted(app.sessions(inFolder: checkout.id)))]
    }
}
