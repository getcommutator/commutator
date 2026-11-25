import SafariServices
import UIKit

/// Pull requests of every project repository, grouped like the desktop board
/// (Needs attention · Awaiting review · Approved · Drafts). A row opens the
/// pull request on GitHub.
// ponytail: rows open GitHub; the in-app pull request screen comes later.
final class PullRequestsViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<PullRequestSection, String>!
    private var items: [String: PullRequestItem] = [:]
    private var filter = PullRequestFilter.authored
    private var loadTask: Task<Void, Never>?
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

        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = .clear
        config.headerMode = .supplementary
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.translatesAutoresizingMaskIntoConstraints = false
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.refreshControl = UIRefreshControl(frame: .zero, primaryAction: UIAction { [weak self] _ in self?.load(refresh: true) })
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<PullRequestCell, String> { [weak self] cell, _, url in
            guard let pr = self?.items[url] else { return }
            cell.configure(pr)
        }
        let header = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, path in
            guard let self, let section = self.dataSource.sectionIdentifier(for: path.section) else { return }
            var c = UIListContentConfiguration.groupedHeader()
            c.text = "\(Self.title(section)) · \(self.dataSource.snapshot().numberOfItems(inSection: section))"
            view.contentConfiguration = c
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, url in
            cv.dequeueConfiguredReusableCell(using: cell, for: path, item: url)
        }
        dataSource.supplementaryViewProvider = { cv, _, path in
            cv.dequeueConfiguredReusableSupplementary(using: header, for: path)
        }

        let picker = UISegmentedControl(items: ["Mine", "Reviewing", "All"])
        picker.selectedSegmentIndex = 0
        picker.accessibilityIdentifier = "pull-requests-filter"
        picker.addAction(UIAction { [weak self, weak picker] _ in
            guard let self, let picker else { return }
            self.filter = [.authored, .reviewing, .all][picker.selectedSegmentIndex]
            self.load(refresh: false)
        }, for: .valueChanged)
        // Under the title (T3 Code's filter row), not in the bar: pinned, so it's there when a
        // filter comes back empty.
        picker.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(picker)
        setContentScrollView(collectionView)
        NSLayoutConstraint.activate([
            picker.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 4),
            picker.leadingAnchor.constraint(equalTo: view.layoutMarginsGuide.leadingAnchor),
            picker.trailingAnchor.constraint(equalTo: view.layoutMarginsGuide.trailingAnchor),
            collectionView.topAnchor.constraint(equalTo: picker.bottomAnchor, constant: 4),
            collectionView.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            collectionView.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            collectionView.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])

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
        load(refresh: false)
    }

    private func load(refresh: Bool) {
        guard let client = app.client else { return }
        loadTask?.cancel()
        if items.isEmpty { spinner.startAnimating(); empty.isHidden = true }
        let filter = filter
        loadTask = Task { @MainActor [weak self] in
            let list = await client.listPullRequests(filter: filter, refresh: refresh)
            guard let self, !Task.isCancelled else { return }
            self.apply(list)
        }
    }

    private func apply(_ list: PullRequestList) {
        spinner.stopAnimating()
        collectionView.refreshControl?.endRefreshing()
        items = Dictionary(list.items.map { ($0.url, $0) }, uniquingKeysWith: { a, _ in a })
        var snap = NSDiffableDataSourceSnapshot<PullRequestSection, String>()
        for section in [PullRequestSection.attention, .review, .approved, .drafts] {
            let urls = list.items.filter { $0.section == section }.map(\.url)
            guard !urls.isEmpty else { continue }
            snap.appendSections([section])
            snap.appendItems(urls, toSection: section)
        }
        // Rows keep their identity (url) while their contents change.
        snap.reconfigureItems(snap.itemIdentifiers)
        dataSource.apply(snap, animatingDifferences: dataSource.snapshot().numberOfItems > 0)

        if !list.items.isEmpty {
            empty.isHidden = true
            if let failure = list.failures.first {
                Toast.show("\(failure.repository): \(failure.message)", in: view.window)
            }
        } else {
            empty.isHidden = false
            empty.text = if list.noRepositories {
                "No projects with a GitHub or GitLab repository on an online computer."
            } else if let failure = list.failures.first {
                "\(failure.message)\n\(failure.repository)"
            } else {
                switch filter {
                case .authored: "You have no open pull requests."
                case .reviewing: "No reviews requested from you."
                case .all: "No open pull requests."
                }
            }
        }
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard let url = dataSource.itemIdentifier(for: indexPath).flatMap(URL.init(string:)) else { return }
        let safari = SFSafariViewController(url: url)
        safari.preferredControlTintColor = Palette.accent
        present(safari, animated: true)
    }

    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt indexPaths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        guard let path = indexPaths.first, let url = dataSource.itemIdentifier(for: path) else { return nil }
        return UIContextMenuConfiguration(actionProvider: { _ in
            UIMenu(children: [
                UIAction(title: "Open in Browser", image: UIImage(systemName: "safari")) { _ in
                    if let u = URL(string: url) { UIApplication.shared.open(u) }
                },
                UIAction(title: "Copy Link", image: UIImage(systemName: "link")) { _ in UIPasteboard.general.string = url },
            ])
        })
    }

    private static func title(_ section: PullRequestSection) -> String {
        switch section {
        case .attention: "Needs attention"
        case .review: "Awaiting review"
        case .approved: "Approved"
        case .drafts: "Drafts"
        }
    }

}

/// A pull request row, laid out like T3 Code's: repository and number over the title, then the
/// status chips with CI and the diff stats. A conflict is a warning badge on the PR icon.
final class PullRequestCell: UICollectionViewListCell {
    private let icon = UIImageView()
    private let conflict = UIImageView(image: UIImage(systemName: "exclamationmark.triangle"))
    private let meta = UILabel()
    private let time = UILabel()
    private let title = UILabel()
    private let chips = UIStackView()
    private let ci = UIImageView()
    private let stats = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        conflict.preferredSymbolConfiguration = .init(pointSize: 10, weight: .bold)
        conflict.tintColor = Palette.danger
        meta.font = Fonts.ui(.sans, 13)
        meta.textColor = Palette.secondary
        time.font = Fonts.ui(.sans, 13)
        time.textColor = Palette.secondary
        time.setContentCompressionResistancePriority(.required, for: .horizontal)
        title.font = Fonts.ui(.sansMedium, 15)
        title.textColor = Palette.text
        title.numberOfLines = 2
        chips.spacing = 6
        ci.preferredSymbolConfiguration = .init(pointSize: 12, weight: .semibold)
        stats.font = Fonts.ui(.mono, 12)
        stats.setContentCompressionResistancePriority(.required, for: .horizontal)
        ci.setContentCompressionResistancePriority(.required, for: .horizontal)

        let top = UIStackView(arrangedSubviews: [meta, time])
        top.spacing = 8
        let bottom = UIStackView(arrangedSubviews: [chips, UIView(), ci, stats])
        bottom.spacing = 6
        bottom.alignment = .center
        let column = UIStackView(arrangedSubviews: [top, title, bottom])
        column.axis = .vertical
        column.spacing = 4
        column.setCustomSpacing(8, after: title)
        for v in [icon, conflict, column] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            contentView.addSubview(v)
        }
        NSLayoutConstraint.activate([
            icon.leadingAnchor.constraint(equalTo: contentView.layoutMarginsGuide.leadingAnchor),
            icon.topAnchor.constraint(equalTo: column.topAnchor),
            icon.widthAnchor.constraint(equalToConstant: 20),
            icon.heightAnchor.constraint(equalToConstant: 20),
            conflict.centerXAnchor.constraint(equalTo: icon.trailingAnchor, constant: -1),
            conflict.centerYAnchor.constraint(equalTo: icon.bottomAnchor, constant: -1),
            column.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 12),
            column.trailingAnchor.constraint(equalTo: contentView.layoutMarginsGuide.trailingAnchor),
            column.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 12),
            column.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -12),
        ])
        var bg = UIBackgroundConfiguration.listGroupedCell()
        bg.backgroundColor = Palette.elevated
        backgroundConfiguration = bg
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ pr: PullRequestItem) {
        icon.image = PRIcon.image(side: 20)
        // The conflict is the badge; the icon keeps the open tone under it.
        icon.tintColor = switch pr.tone {
        case .open, .conflict: Palette.success
        case .draft: Palette.tertiary
        case .changesRequested: Palette.warning
        }
        conflict.isHidden = pr.tone != .conflict
        meta.text = "\(pr.repository) \(pr.provider == "gitlab" ? "!" : "#")\(pr.number) · \(pr.author)"
        time.text = relativeTimeLabel(atMs: pr.updatedAtMs, nowMs: Int64(Date().timeIntervalSince1970 * 1000))
        title.text = pr.title

        chips.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for word in pr.status.components(separatedBy: " · ") where !word.isEmpty {
            chips.addArrangedSubview(Self.chip(word))
        }
        switch pr.ci {
        case .passed: ci.image = UIImage(systemName: "checkmark.circle"); ci.tintColor = Palette.success; ci.accessibilityLabel = "CI passed"
        case .pending: ci.image = UIImage(systemName: "clock"); ci.tintColor = Palette.warning; ci.accessibilityLabel = "CI pending"
        case .failed: ci.image = UIImage(systemName: "xmark.circle"); ci.tintColor = Palette.danger; ci.accessibilityLabel = "CI failing"
        case .none: ci.image = nil
        }
        ci.isHidden = ci.image == nil
        let text = NSMutableAttributedString(string: "+\(pr.additions)", attributes: [.foregroundColor: Palette.success])
        text.append(NSAttributedString(string: "  −\(pr.deletions)", attributes: [.foregroundColor: Palette.danger]))
        stats.attributedText = text
        stats.isHidden = pr.additions == 0 && pr.deletions == 0
        accessibilityIdentifier = "pull-request-\(pr.number)"
        accessibilityLabel = [pr.title, meta.text, pr.tone == .conflict ? "Conflicts" : nil, pr.status].compactMap { $0 }.joined(separator: ", ")
    }

    private static func chip(_ text: String) -> UIView {
        let label = UILabel()
        label.text = text
        label.font = Fonts.ui(.sans, 12)
        label.textColor = Palette.secondary
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
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
