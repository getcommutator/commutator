import SafariServices
import UIKit

/// Pull requests of every project repository, after T3 Code's page (and Shepherd's
/// pull-requests extension): grouped into Authored, Reviewing and Others, with a row of
/// filters under the title that scrolls away with the list, sorts, and a search that takes
/// `author:`, `repo:` and `is:draft|approved|mine` beside plain words. A row opens the pull
/// request on GitHub.
// ponytail: rows open GitHub; the in-app pull request screen comes later.
// ponytail: State (merged/closed) and Labels need the engine to read them; open only, no labels.
final class PullRequestsViewController: UIViewController, UICollectionViewDelegate, UISearchResultsUpdating {
    private enum Section: Hashable { case filters, group(Group) }
    private enum Group: CaseIterable { case authored, reviewing, others }
    private static let filtersItem = "filters"

    /// The picks, kept on this phone across opens.
    struct Filters: Codable, Equatable {
        var sort = "updated"
        var involvement = "all"
        var author = ""
        var draft = "all"
        var review = "all"
        var checks = "all"
        var repository = ""
        var state = "open"
        var labels: [String] = []

        /// How many differ from the defaults (the first chip fills when any do).
        var active: Int {
            let d = Filters()
            return [state != d.state, !labels.isEmpty, involvement != d.involvement, !author.isEmpty, draft != d.draft, review != d.review, checks != d.checks, !repository.isEmpty].filter { $0 }.count
        }
    }

    private let app: AppModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Section, String>!
    private var all: [PullRequestItem] = []
    private var items: [String: PullRequestItem] = [:]
    private var filters: Filters = {
        (UserDefaults.standard.data(forKey: "pullRequests.filters")).flatMap { try? JSONDecoder().decode(Filters.self, from: $0) } ?? Filters()
    }() {
        didSet { UserDefaults.standard.set(try? JSONEncoder().encode(filters), forKey: "pullRequests.filters") }
    }
    private var query = ""
    private var loaded = false
    private var loadTask: Task<Void, Never>?
    private var loading = false
    private var token: AnyObject?
    private let empty = UILabel()
    private let spinner = UIActivityIndicatorView(style: .medium)

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Pull Requests"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .always

        let search = UISearchController(searchResultsController: nil)
        search.obscuresBackgroundDuringPresentation = false
        search.searchResultsUpdater = self
        search.searchBar.placeholder = "Search, or author:me repo:mono is:draft"
        search.searchBar.autocapitalizationType = .none
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = true

        // The filter row is its own section, plain and edge to edge, so it scrolls with the
        // list and stays when nothing matches.
        let layout = UICollectionViewCompositionalLayout { [weak self] index, env in
            let filters = self?.dataSource?.sectionIdentifier(for: index) == .filters
            var config = UICollectionLayoutListConfiguration(appearance: .plain)
            config.backgroundColor = .clear
            config.showsSeparators = false
            config.headerMode = filters ? .none : .supplementary
            config.headerTopPadding = 0
            return NSCollectionLayoutSection.list(using: config, layoutEnvironment: env)
        }
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: layout)
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.refreshControl = UIRefreshControl(frame: .zero, primaryAction: UIAction { [weak self] _ in self?.load(refresh: true) })
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<PullRequestCell, String> { [weak self] cell, _, url in
            guard let pr = self?.items[url] else { return }
            cell.configure(pr)
        }
        let filtersCell = UICollectionView.CellRegistration<PullRequestFiltersCell, String> { [weak self] cell, _, _ in
            guard let self else { return }
            cell.configure(self.chips())
        }
        let header = UICollectionView.SupplementaryRegistration<PullRequestGroupHeader>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, path in
            guard let self, case let .group(group) = self.dataSource.sectionIdentifier(for: path.section) else { return }
            view.configure(symbol: Self.symbol(group), title: Self.title(group), count: self.dataSource.snapshot().numberOfItems(inSection: .group(group)))
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, id in
            id == Self.filtersItem
                ? cv.dequeueConfiguredReusableCell(using: filtersCell, for: path, item: id)
                : cv.dequeueConfiguredReusableCell(using: cell, for: path, item: id)
        }
        dataSource.supplementaryViewProvider = { cv, _, path in
            cv.dequeueConfiguredReusableSupplementary(using: header, for: path)
        }

        empty.font = Fonts.ui(.sans, 15)
        empty.textColor = Palette.secondary
        empty.numberOfLines = 0
        empty.textAlignment = .center
        empty.translatesAutoresizingMaskIntoConstraints = false
        spinner.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(empty)
        view.addSubview(spinner)
        NSLayoutConstraint.activate([
            empty.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor),
            empty.leadingAnchor.constraint(equalTo: view.layoutMarginsGuide.leadingAnchor, constant: 16),
            empty.trailingAnchor.constraint(equalTo: view.layoutMarginsGuide.trailingAnchor, constant: -16),
            spinner.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            spinner.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor),
        ])
        render()
        load(refresh: false)
        // Opened at launch, the workspace (projects, the client) may not be in yet: ask again
        // once it is, rather than keep "no projects".
        token = app.observe { [weak self] in
            guard let self, !self.loading, !self.loaded || self.lastList?.noRepositories == true else { return }
            self.load(refresh: false)
        }
    }

    // MARK: Loading

    /// Every open pull request; the involvement is a filter here, as T3 Code's.
    // ponytail: the core's first page per repository (50); a big repository can drop your own past it.
    private func load(refresh: Bool) {
        guard let client = app.client else { return }
        loadTask?.cancel()
        loading = true
        if all.isEmpty { spinner.startAnimating(); empty.isHidden = true }
        let state: PullRequestStateFilter = switch filters.state {
        case "merged": .merged
        case "closed": .closed
        case "all": .all
        default: .open
        }
        loadTask = Task { @MainActor [weak self] in
            let list = await client.listPullRequests(filter: .all, state: state, refresh: refresh)
            guard let self, !Task.isCancelled else { return }
            self.loading = false
            self.spinner.stopAnimating()
            self.collectionView.refreshControl?.endRefreshing()
            self.loaded = true
            self.all = list.items
            self.items = Dictionary(list.items.map { ($0.url, $0) }, uniquingKeysWith: { a, _ in a })
            self.render(list)
        }
    }

    func updateSearchResults(for search: UISearchController) {
        let text = search.searchBar.text ?? ""
        guard text != query else { return }
        query = text
        render()
    }

    private var lastList: PullRequestList?

    private func render(_ list: PullRequestList? = nil) {
        if let list { lastList = list }
        let shown = sorted(all.filter(matches))
        var snap = NSDiffableDataSourceSnapshot<Section, String>()
        snap.appendSections([.filters])
        snap.appendItems([Self.filtersItem])
        let wanted: [Group] = switch filters.involvement {
        case "authored": [.authored]
        case "reviewing": [.reviewing]
        default: Group.allCases
        }
        for group in wanted {
            let urls = shown.filter { Self.group(of: $0) == group }.map(\.url)
            guard !urls.isEmpty else { continue }
            snap.appendSections([.group(group)])
            snap.appendItems(urls, toSection: .group(group))
        }
        // Rows keep their identity (url) while their contents change; the chips redraw.
        snap.reconfigureItems(snap.itemIdentifiers)
        dataSource.apply(snap, animatingDifferences: dataSource.snapshot().numberOfItems > 1)

        if !loaded { return }
        let rows = snap.numberOfItems - 1
        empty.isHidden = rows > 0
        if rows > 0 {
            if list != nil, let failure = list?.failures.first {
                Toast.show("\(failure.repository): \(failure.message)", in: view.window)
            }
        } else if lastList?.noRepositories == true {
            empty.text = "No projects with a GitHub or GitLab repository on an online computer."
        } else if all.isEmpty, let failure = lastList?.failures.first {
            empty.text = "\(failure.message)\n\(failure.repository)"
        } else {
            empty.text = all.isEmpty ? (filters.state == "open" ? "No open pull requests." : "No pull requests.") : "No pull requests match."
        }
    }

    // MARK: Filtering

    private static func group(of pr: PullRequestItem) -> Group {
        pr.authored ? .authored : pr.reviewRequested ? .reviewing : .others
    }

    private func matches(_ pr: PullRequestItem) -> Bool {
        let f = filters
        if !f.author.isEmpty, pr.author != f.author { return false }
        if !f.repository.isEmpty, pr.repository != f.repository { return false }
        if !f.labels.isEmpty, !pr.labels.contains(where: { f.labels.contains($0.name) }) { return false }
        if f.draft == "only", !pr.isDraft { return false }
        if f.draft == "hide", pr.isDraft { return false }
        if f.review != "all", Self.reviewKey(pr.review) != f.review { return false }
        if f.checks != "all", Self.checksKey(pr.ci) != f.checks { return false }
        return Self.search(query, pr)
    }

    /// Plain words match the title, repository, author or number; `author:` (`author:me`),
    /// `repo:` and `is:draft|approved|mine` narrow, as the extension's search does.
    static func search(_ query: String, _ pr: PullRequestItem) -> Bool {
        for word in query.lowercased().split(whereSeparator: \.isWhitespace).map(String.init) {
            let parts = word.split(separator: ":", maxSplits: 1).map(String.init)
            let (key, value) = parts.count == 2 ? (parts[0], parts[1]) : ("", word)
            let ok: Bool = switch key {
            case "author" where !value.isEmpty: value == "me" ? pr.authored : pr.author.lowercased().contains(value)
            case "repo" where !value.isEmpty: pr.repository.lowercased().contains(value)
            case "label" where !value.isEmpty: pr.labels.contains { $0.name.lowercased().contains(value) }
            case "is" where !value.isEmpty:
                switch value {
                case "draft": pr.isDraft
                case "approved": pr.review == .approved
                case "mine": pr.authored
                default: true
                }
            default:
                [pr.title, pr.repository, pr.author, "#\(pr.number)"].contains { $0.lowercased().contains(word) }
            }
            if !ok { return false }
        }
        return true
    }

    private func sorted(_ list: [PullRequestItem]) -> [PullRequestItem] {
        switch filters.sort {
        case "stale": list.sorted { $0.updatedAtMs < $1.updatedAtMs }
        case "newest": list.sorted { $0.createdAtMs > $1.createdAtMs }
        case "largest": list.sorted { $0.additions + $0.deletions > $1.additions + $1.deletions }
        case "smallest": list.sorted { $0.additions + $0.deletions < $1.additions + $1.deletions }
        default: list.sorted { $0.updatedAtMs > $1.updatedAtMs }
        }
    }

    private static func reviewKey(_ r: PullRequestReview) -> String {
        switch r {
        case .approved: "approved"
        case .changesRequested: "changes-requested"
        case .reviewRequired: "review-required"
        case .none: "none"
        }
    }

    private static func checksKey(_ c: PullRequestCi) -> String {
        switch c {
        case .passed: "passing"
        case .failed: "failing"
        case .pending: "pending"
        case .none: "none"
        }
    }

    // MARK: Chips

    /// A choice: its value, label and SF Symbol.
    private typealias Choice = (value: String, title: String, symbol: String)

    private static let sorts: [Choice] = [
        ("updated", "Recently updated", "clock.arrow.circlepath"), ("stale", "Least recently updated", "clock"),
        ("newest", "Newest", "sparkles"), ("largest", "Largest", "arrow.up.right"), ("smallest", "Smallest", "arrow.down.right"),
    ]
    private static let states: [Choice] = [("open", "Open", "arrow.triangle.pull"), ("merged", "Merged", "arrow.triangle.merge"), ("closed", "Closed", "xmark.circle"), ("all", "All", "square.stack")]
    private static let involvements: [Choice] = [("all", "All PRs", "square.stack"), ("authored", "Authored", "pencil.line"), ("reviewing", "Reviewing", "eye")]
    private static let drafts: [Choice] = [("all", "All", "square.stack"), ("only", "Drafts only", "pencil.and.outline"), ("hide", "Hide drafts", "eye.slash")]
    private static let reviews: [Choice] = [
        ("all", "All", "square.stack"), ("approved", "Approved", "checkmark.circle"), ("changes-requested", "Changes requested", "xmark.circle"),
        ("review-required", "Review required", "circle.dashed"), ("none", "No reviews", "circle.slash"),
    ]
    private static let checks: [Choice] = [("all", "All", "square.stack"), ("passing", "Passing", "checkmark.circle"), ("failing", "Failing", "xmark.circle"), ("pending", "Pending", "circle.dotted")]

    private func set(_ change: (inout Filters) -> Void) {
        let state = filters.state
        change(&filters)
        // The state is the listing's own: ask for it anew.
        if filters.state != state {
            all = []
            items = [:]
            loaded = false
            render()
            load(refresh: false)
        } else {
            render()
        }
    }

    private func chips() -> [UIButton] {
        let f = filters
        func menu(_ choices: [Choice], _ picked: String, _ key: WritableKeyPath<Filters, String>) -> UIMenu {
            UIMenu(options: .singleSelection, children: choices.map { c in
                UIAction(title: c.title, image: UIImage(systemName: c.symbol), state: c.value == picked ? .on : .off) { [weak self] _ in
                    self?.set { $0[keyPath: key] = c.value }
                }
            })
        }
        func label(_ choices: [Choice], _ picked: String) -> String { choices.first { $0.value == picked }?.title ?? picked }
        /// Who or what the loaded rows have, the most frequent first.
        func names(_ key: KeyPath<PullRequestItem, String>, any: String, _ picked: WritableKeyPath<Filters, String>) -> UIMenu {
            let counts = Dictionary(all.map { ($0[keyPath: key], 1) }, uniquingKeysWith: +)
            let ordered = counts.keys.sorted { (counts[$0]!, $1) > (counts[$1]!, $0) }
            let current = f[keyPath: picked]
            return UIMenu(options: .singleSelection, children: [
                UIAction(title: any, image: UIImage(systemName: "square.stack"), state: current.isEmpty ? .on : .off) { [weak self] _ in self?.set { $0[keyPath: picked] = "" } },
            ] + ordered.map { name in
                UIAction(title: name, subtitle: "\(counts[name]!)", state: name == current ? .on : .off) { [weak self] _ in self?.set { $0[keyPath: picked] = name } }
            })
        }

        /// Labels: any of the picked, the menu staying open while you tick.
        func labelsMenu() -> UIMenu {
            let counts = Dictionary(all.flatMap { $0.labels.map { ($0.name, 1) } }, uniquingKeysWith: +)
            let ordered = counts.keys.sorted { (counts[$0]!, $1) > (counts[$1]!, $0) }
            return UIMenu(children: [
                UIAction(title: "Any", image: UIImage(systemName: "square.stack"), state: f.labels.isEmpty ? .on : .off) { [weak self] _ in self?.set { $0.labels = [] } },
            ] + ordered.map { name in
                UIAction(title: name, subtitle: "\(counts[name]!)", attributes: .keepsMenuPresented, state: f.labels.contains(name) ? .on : .off) { [weak self] _ in
                    self?.set { $0.labels = $0.labels.contains(name) ? $0.labels.filter { $0 != name } : $0.labels + [name] }
                }
            })
        }

        let active = f.active
        let reset = UIAction(title: "Clear Filters", image: UIImage(systemName: "xmark"), attributes: active == 0 ? .disabled : []) { [weak self] _ in
            self?.set { $0 = Filters(sort: $0.sort) }
        }
        let first = UIMenu(children: [UIMenu(title: "Sort", options: .displayInline, children: menu(Self.sorts, f.sort, \.sort).children), reset])
        return [
            Self.chip(nil, symbol: "line.3.horizontal.decrease", active: active > 0, menu: first, chevron: false, label: "Sort and filters"),
            Self.chip(label(Self.states, f.state), symbol: nil, active: f.state != "open", menu: menu(Self.states, f.state, \.state)),
            Self.chip(label(Self.involvements, f.involvement), symbol: nil, active: f.involvement != "all", menu: menu(Self.involvements, f.involvement, \.involvement)),
            Self.chip(f.author.isEmpty ? "Author" : f.author, symbol: nil, active: !f.author.isEmpty, menu: names(\.author, any: "Anyone", \.author)),
            Self.chip(f.labels.isEmpty ? "Labels" : f.labels.count == 1 ? f.labels[0] : "\(f.labels.count) labels", symbol: nil, active: !f.labels.isEmpty, menu: labelsMenu()),
            Self.chip(f.review == "all" ? "Review" : label(Self.reviews, f.review), symbol: nil, active: f.review != "all", menu: menu(Self.reviews, f.review, \.review)),
            Self.chip(f.checks == "all" ? "Checks" : label(Self.checks, f.checks), symbol: nil, active: f.checks != "all", menu: menu(Self.checks, f.checks, \.checks)),
            Self.chip(f.draft == "all" ? "Draft" : label(Self.drafts, f.draft), symbol: nil, active: f.draft != "all", menu: menu(Self.drafts, f.draft, \.draft)),
            Self.chip(f.repository.isEmpty ? "Project" : String(f.repository.split(separator: "/").last ?? ""), symbol: nil, active: !f.repository.isEmpty,
                      menu: names(\.repository, any: "All projects", \.repository)),
        ]
    }

    /// T3 Code's filter pill: a capsule with a chevron; filled in the accent while it filters.
    private static func chip(_ title: String?, symbol: String?, active: Bool, menu: UIMenu, chevron: Bool = true, label: String? = nil) -> UIButton {
        var c = active ? UIButton.Configuration.filled() : UIButton.Configuration.gray()
        c.cornerStyle = .capsule
        c.baseBackgroundColor = active ? Palette.accent : Palette.chip
        c.baseForegroundColor = active ? Palette.onAccent : Palette.text
        c.title = title
        let font = Fonts.ui(.sans, TypeScale.size(15))
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { var a = $0; a.font = font; return a }
        c.image = UIImage(systemName: symbol ?? (chevron ? "chevron.down" : ""))
        c.preferredSymbolConfigurationForImage = .init(pointSize: symbol == nil ? 11 : 14, weight: .semibold)
        c.imagePlacement = symbol == nil ? .trailing : .leading
        c.imagePadding = 6
        c.contentInsets = NSDirectionalEdgeInsets(top: 8, leading: 14, bottom: 8, trailing: 14)
        let button = UIButton(configuration: c)
        button.menu = menu
        button.showsMenuAsPrimaryAction = true
        // Each pill its own width: the row scrolls, nothing stretches.
        button.setContentHuggingPriority(.required, for: .horizontal)
        button.setContentCompressionResistancePriority(.required, for: .horizontal)
        button.accessibilityLabel = label ?? title
        return button
    }

    // MARK: Rows

    func collectionView(_ collectionView: UICollectionView, shouldHighlightItemAt indexPath: IndexPath) -> Bool {
        dataSource.itemIdentifier(for: indexPath) != Self.filtersItem
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard let id = dataSource.itemIdentifier(for: indexPath), id != Self.filtersItem, let url = URL(string: id) else { return }
        let safari = SFSafariViewController(url: url)
        safari.preferredControlTintColor = Palette.accent
        present(safari, animated: true)
    }

    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt indexPaths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        guard let path = indexPaths.first, let url = dataSource.itemIdentifier(for: path), url != Self.filtersItem else { return nil }
        return UIContextMenuConfiguration(actionProvider: { _ in
            UIMenu(children: [
                UIAction(title: "Open in Browser", image: UIImage(systemName: "safari")) { _ in
                    if let u = URL(string: url) { UIApplication.shared.open(u) }
                },
                UIAction(title: "Copy Link", image: UIImage(systemName: "link")) { _ in UIPasteboard.general.string = url },
            ])
        })
    }

    private static func title(_ group: Group) -> String {
        switch group {
        case .authored: "Authored"
        case .reviewing: "Reviewing"
        case .others: "Others"
        }
    }

    private static func symbol(_ group: Group) -> String {
        switch group {
        case .authored: "pencil.line"
        case .reviewing: "eye"
        case .others: "person.2"
        }
    }
}

/// The filter pills, scrolling sideways edge to edge.
final class PullRequestFiltersCell: UICollectionViewListCell {
    private let scroll = UIScrollView()
    private let row = UIStackView()

    override init(frame: CGRect) {
        super.init(frame: frame)
        var bg = UIBackgroundConfiguration.clear()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
        scroll.showsHorizontalScrollIndicator = false
        scroll.alwaysBounceHorizontal = true
        scroll.contentInset = UIEdgeInsets(top: 0, left: 16, bottom: 0, right: 16)
        row.spacing = 8
        row.alignment = .center
        for v in [scroll, row] as [UIView] { v.translatesAutoresizingMaskIntoConstraints = false }
        contentView.addSubview(scroll)
        scroll.addSubview(row)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
            scroll.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 4),
            scroll.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -8),
            row.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor),
            row.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor),
            row.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            row.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor),
            row.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ chips: [UIButton]) {
        row.arrangedSubviews.forEach { $0.removeFromSuperview() }
        chips.forEach(row.addArrangedSubview)
    }
}

/// A group's header, Shepherd's: its icon, name and count, then a hairline to the edge.
final class PullRequestGroupHeader: UICollectionReusableView {
    private let icon = UIImageView()
    private let title = UILabel()
    private let count = UILabel()
    private let line = UIView()

    override init(frame: CGRect) {
        super.init(frame: frame)
        icon.tintColor = Palette.secondary
        icon.preferredSymbolConfiguration = .init(pointSize: 14, weight: .regular)
        title.font = Fonts.ui(.sansMedium, TypeScale.size(16))
        title.textColor = Palette.secondary
        count.font = Fonts.ui(.sans, TypeScale.size(16))
        count.textColor = Palette.tertiary
        line.backgroundColor = Palette.hairline
        let row = UIStackView(arrangedSubviews: [icon, title, count, line])
        row.spacing = 8
        row.alignment = .center
        row.setCustomSpacing(12, after: count)
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        for v in [icon, title, count] { v.setContentHuggingPriority(.required, for: .horizontal) }
        NSLayoutConstraint.activate([
            row.leadingAnchor.constraint(equalTo: layoutMarginsGuide.leadingAnchor),
            row.trailingAnchor.constraint(equalTo: layoutMarginsGuide.trailingAnchor),
            row.topAnchor.constraint(equalTo: topAnchor, constant: 20),
            row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -6),
            line.heightAnchor.constraint(equalToConstant: 1 / UIScreen.main.scale),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(symbol: String, title: String, count: Int) {
        icon.image = UIImage(systemName: symbol)
        self.title.text = title
        self.count.text = "\(count)"
    }
}

/// A pull request row, Shepherd's: the state glyph (a conflict lays a red triangle over its
/// corner); `#number` and the title over the author, repository and status chips; CI and
/// the size over the time on the right.
final class PullRequestCell: UICollectionViewListCell {
    private let icon = UIImageView()
    private let conflict = UIImageView(image: UIImage(systemName: "exclamationmark.triangle"))
    private let title = UILabel()
    private let author = UILabel()
    private let avatar = AvatarView()
    private let repo = UILabel()
    private let chips = UIStackView()
    private let ci = UIImageView()
    private let stats = UILabel()
    private let time = UILabel()

    private static let grouped: NumberFormatter = {
        let f = NumberFormatter()
        f.numberStyle = .decimal
        f.locale = Locale(identifier: "en_US")
        return f
    }()

    override init(frame: CGRect) {
        super.init(frame: frame)
        conflict.preferredSymbolConfiguration = .init(pointSize: 10, weight: .bold)
        conflict.tintColor = Palette.danger
        title.numberOfLines = 2
        title.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        author.font = Fonts.ui(.sans, TypeScale.size(13.5))
        author.textColor = Palette.secondary
        repo.font = Fonts.ui(.sans, TypeScale.size(13.5))
        repo.textColor = Palette.secondary
        repo.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        chips.spacing = 6
        chips.setContentCompressionResistancePriority(.defaultLow - 1, for: .horizontal)
        ci.preferredSymbolConfiguration = .init(pointSize: 13, weight: .regular)
        stats.font = Fonts.ui(.mono, TypeScale.size(12.5))
        time.font = Fonts.ui(.sans, TypeScale.size(13))
        time.textColor = Palette.secondary
        for v in [ci, stats, time, author] as [UIView] {
            v.setContentCompressionResistancePriority(.required, for: .horizontal)
            v.setContentHuggingPriority(.required, for: .horizontal)
        }

        let size = UIStackView(arrangedSubviews: [ci, stats])
        size.spacing = 6
        size.alignment = .center
        let top = UIStackView(arrangedSubviews: [title, size])
        top.spacing = 10
        top.alignment = .firstBaseline
        let meta = UIStackView(arrangedSubviews: [avatar, author, repo, chips, UIView(), time])
        meta.spacing = 8
        meta.alignment = .center
        let column = UIStackView(arrangedSubviews: [top, meta])
        column.axis = .vertical
        column.spacing = 6
        for v in [icon, conflict, column] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            contentView.addSubview(v)
        }
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: contentView.layoutMarginsGuide.leadingAnchor),
            icon.topAnchor.constraint(equalTo: column.topAnchor, constant: 1),
            icon.widthAnchor.constraint(equalToConstant: 20),
            icon.heightAnchor.constraint(equalToConstant: 20),
            conflict.centerXAnchor.constraint(equalTo: icon.trailingAnchor, constant: -1),
            conflict.centerYAnchor.constraint(equalTo: icon.bottomAnchor, constant: -1),
            column.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 10),
            column.trailingAnchor.constraint(equalTo: contentView.layoutMarginsGuide.trailingAnchor),
            column.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 10),
            column.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -10),
        ])
        var bg = UIBackgroundConfiguration.listPlainCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
    }

    required init?(coder: NSCoder) { fatalError() }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listPlainCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.controlFill : .clear
        backgroundConfiguration = bg
    }

    func configure(_ pr: PullRequestItem) {
        icon.image = PRIcon.image(side: 20)
        // The conflict is the badge; the icon keeps the open tone under it.
        icon.tintColor = switch (pr.state, pr.tone) {
        case (.merged, _): Palette.projectDots[2]
        case (.closed, _): Palette.danger
        case (_, .draft): Palette.tertiary
        case (_, .changesRequested): Palette.warning
        default: Palette.success
        }
        conflict.isHidden = pr.tone != .conflict || pr.state != .open

        let text = NSMutableAttributedString(
            string: "\(pr.provider == "gitlab" ? "!" : "#")\(pr.number)  ",
            attributes: [.font: Fonts.ui(.mono, TypeScale.size(13)), .foregroundColor: Palette.secondary]
        )
        text.append(NSAttributedString(string: pr.title, attributes: [.font: Fonts.ui(.sansMedium, TypeScale.size(15.5)), .foregroundColor: Palette.text]))
        title.attributedText = text
        title.lineBreakMode = .byTruncatingTail

        author.text = pr.author
        avatar.load(pr.authorAvatarUrl)
        repo.text = pr.repository
        // Shepherd's: the first three labels, then how many more.
        chips.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for label in pr.labels.prefix(3) {
            chips.addArrangedSubview(Self.chip(label.name))
        }
        if pr.labels.count > 3 {
            let more = UILabel()
            more.text = "+\(pr.labels.count - 3)"
            more.font = Fonts.ui(.sans, TypeScale.size(12.5))
            more.textColor = Palette.secondary
            chips.addArrangedSubview(more)
        }
        time.text = relativeTimeLabel(atMs: pr.updatedAtMs, nowMs: Int64(Date().timeIntervalSince1970 * 1000))

        switch pr.ci {
        case .passed: ci.image = UIImage(systemName: "checkmark.circle"); ci.tintColor = Palette.success; ci.accessibilityLabel = "CI passed"
        case .pending: ci.image = UIImage(systemName: "circle.dotted.circle"); ci.tintColor = Palette.warning; ci.accessibilityLabel = "CI pending"
        case .failed: ci.image = UIImage(systemName: "xmark.circle"); ci.tintColor = Palette.danger; ci.accessibilityLabel = "CI failing"
        case .none: ci.image = nil
        }
        ci.isHidden = ci.image == nil
        let n = { (v: UInt64) in Self.grouped.string(from: NSNumber(value: v)) ?? String(v) }
        let size = NSMutableAttributedString(string: "+\(n(pr.additions))", attributes: [.foregroundColor: Palette.success])
        size.append(NSAttributedString(string: " −\(n(pr.deletions))", attributes: [.foregroundColor: Palette.danger]))
        stats.attributedText = size
        stats.isHidden = pr.additions == 0 && pr.deletions == 0

        accessibilityIdentifier = "pull-request-\(pr.number)"
        accessibilityLabel = [pr.title, "\(pr.repository) #\(pr.number)", pr.author, pr.tone == .conflict ? "Conflicts" : nil, pr.labels.map(\.name).joined(separator: ", "), time.text]
            .compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: ", ")
    }

    private static func chip(_ text: String) -> UIView {
        let label = UILabel()
        label.text = text
        label.font = Fonts.ui(.sans, TypeScale.size(12.5))
        label.textColor = Palette.text
        label.setContentCompressionResistancePriority(.defaultLow - 1, for: .horizontal)
        let box = UIView()
        box.backgroundColor = Palette.chip
        box.layer.cornerRadius = 6
        label.translatesAutoresizingMaskIntoConstraints = false
        box.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: box.leadingAnchor, constant: 7),
            label.trailingAnchor.constraint(equalTo: box.trailingAnchor, constant: -7),
            label.topAnchor.constraint(equalTo: box.topAnchor, constant: 3),
            label.bottomAnchor.constraint(equalTo: box.bottomAnchor, constant: -3),
        ])
        return box
    }
}

/// An author's picture, 18 pt round; hidden until it loads. Kept for the session.
final class AvatarView: UIImageView {
    private static let cache = NSCache<NSString, UIImage>()
    private var url: String?

    init() {
        super.init(frame: .zero)
        contentMode = .scaleAspectFill
        clipsToBounds = true
        layer.cornerRadius = 9
        backgroundColor = Palette.chip
        translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([widthAnchor.constraint(equalToConstant: 18), heightAnchor.constraint(equalToConstant: 18)])
    }

    required init?(coder: NSCoder) { fatalError() }

    func load(_ url: String?) {
        self.url = url
        image = url.flatMap { Self.cache.object(forKey: $0 as NSString) }
        isHidden = url == nil
        guard image == nil, let url, let remote = URL(string: url) else { return }
        Task { @MainActor [weak self] in
            guard let (data, _) = try? await URLSession.shared.data(from: remote), let image = UIImage(data: data) else { return }
            Self.cache.setObject(image, forKey: url as NSString)
            if self?.url == url { self?.image = image }
        }
    }
}
