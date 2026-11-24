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
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.refreshControl = UIRefreshControl(frame: .zero, primaryAction: UIAction { [weak self] _ in self?.load(refresh: true) })
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, String> { [weak self] cell, _, url in
            guard let pr = self?.items[url] else { return }
            Self.configure(cell, pr)
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
        navigationItem.rightBarButtonItem = UIBarButtonItem(customView: picker)

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

    private static func tone(_ tone: PullRequestTone) -> UIColor {
        switch tone {
        case .open: Palette.success
        case .draft: Palette.tertiary
        case .conflict: Palette.danger
        case .changesRequested: Palette.warning
        }
    }

    private static func configure(_ cell: UICollectionViewListCell, _ pr: PullRequestItem) {
        var c = UIListContentConfiguration.subtitleCell()
        c.text = pr.title
        c.textProperties.font = Fonts.ui(.sansMedium, 15)
        c.textProperties.color = Palette.text
        c.textProperties.numberOfLines = 2
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        var meta = "\(pr.repository) \(pr.provider == "gitlab" ? "!" : "#")\(pr.number) · \(pr.author) · \(relativeTimeLabel(atMs: pr.updatedAtMs, nowMs: now))"
        if !pr.status.isEmpty { meta += "\n\(pr.status)" }
        c.secondaryText = meta
        c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
        c.secondaryTextProperties.color = Palette.secondary
        c.secondaryTextProperties.numberOfLines = 2
        c.textToSecondaryTextVerticalPadding = 4
        c.image = PRIcon.image(side: 18)
        c.imageProperties.tintColor = tone(pr.tone)
        c.imageProperties.reservedLayoutSize = CGSize(width: 22, height: 22)
        cell.contentConfiguration = c
        var bg = UIBackgroundConfiguration.listGroupedCell()
        bg.backgroundColor = Palette.elevated
        cell.backgroundConfiguration = bg
        cell.accessories = [.customView(configuration: .init(customView: trailing(pr), placement: .trailing()))]
        cell.accessibilityIdentifier = "pull-request-\(pr.number)"
    }

    /// CI mark over the diff stats.
    private static func trailing(_ pr: PullRequestItem) -> UIView {
        let stats = UILabel()
        stats.isHidden = pr.additions == 0 && pr.deletions == 0
        let text = NSMutableAttributedString(string: "+\(pr.additions)", attributes: [.foregroundColor: Palette.success])
        text.append(NSAttributedString(string: " −\(pr.deletions)", attributes: [.foregroundColor: Palette.danger]))
        stats.attributedText = text
        stats.font = Fonts.ui(.mono, 11)
        let ci = UIImageView()
        ci.preferredSymbolConfiguration = .init(pointSize: 12, weight: .semibold)
        switch pr.ci {
        case .passed: ci.image = UIImage(systemName: "checkmark.circle.fill"); ci.tintColor = Palette.success; ci.accessibilityLabel = "CI passed"
        case .pending: ci.image = UIImage(systemName: "clock.fill"); ci.tintColor = Palette.warning; ci.accessibilityLabel = "CI pending"
        case .failed: ci.image = UIImage(systemName: "xmark.circle.fill"); ci.tintColor = Palette.danger; ci.accessibilityLabel = "CI failing"
        case .none: ci.isHidden = true
        }
        let stack = UIStackView(arrangedSubviews: [ci, stats])
        stack.axis = .vertical
        stack.alignment = .trailing
        stack.spacing = 4
        stack.frame.size = stack.systemLayoutSizeFitting(UIView.layoutFittingCompressedSize)
        return stack
    }
}
