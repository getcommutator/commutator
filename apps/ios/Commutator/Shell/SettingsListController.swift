import UIKit

/// An inset-grouped settings list: sections of rows with an icon (or colour swatches), a
/// subtitle and an accessory. Subclasses give the sections and handle taps.
class SettingsListController: UIViewController, UICollectionViewDelegate {
    private(set) var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<String, Row>!

    struct Row: Hashable {
        let id: String
        let title: String
        var subtitle: String?
        var symbol: String
        var tint: UIColor = Palette.secondary
        var accessory: Accessory = .disclosure
        var destructive = false
        /// Out of reach for now (dimmed, its switch off-limits).
        var disabled = false
        /// Colour dots shown in place of the symbol (a theme's background, text, accent, …).
        var swatches: [UInt32] = []

        enum Accessory: Hashable {
            case disclosure
            case none
            case check(Bool)
            case dot(Bool)
            case toggle(Bool)
        }
    }

    /// The list's sections, in order.
    func sections() -> [(title: String, rows: [Row])] { [] }

    func didSelect(_ row: Row, at path: IndexPath) {}

    func toggled(_ id: String, _ on: Bool) {}

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
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Row> { [weak self] cell, _, row in
            var c = UIListContentConfiguration.subtitleCell()
            c.text = row.title
            c.textProperties.font = Fonts.ui(.sansMedium, 16)
            c.textProperties.color = row.destructive ? Palette.danger : row.disabled ? Palette.tertiary : Palette.text
            c.secondaryText = row.subtitle
            c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
            c.secondaryTextProperties.color = Palette.secondary
            if row.swatches.isEmpty {
                c.image = UIImage(systemName: row.symbol)
                c.imageProperties.tintColor = row.destructive ? Palette.danger : row.tint
            } else {
                c.image = Self.swatchImage(row.swatches)
            }
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
            switch row.accessory {
            case .disclosure: cell.accessories = [.disclosureIndicator()]
            case .none: cell.accessories = []
            case let .check(on): cell.accessories = on ? [.checkmark(options: .init(tintColor: Palette.accent))] : []
            case let .dot(online):
                let dot = UIView(frame: CGRect(x: 0, y: 0, width: 8, height: 8))
                dot.backgroundColor = online ? Palette.success : Palette.tertiary
                dot.layer.cornerRadius = 4
                cell.accessories = [.customView(configuration: .init(customView: dot, placement: .trailing()))]
            case let .toggle(on):
                let toggle = UISwitch()
                toggle.isOn = on
                toggle.isEnabled = !row.disabled
                toggle.onTintColor = Palette.toggleOn
                toggle.accessibilityIdentifier = "toggle-\(row.id)"
                toggle.addAction(UIAction { [weak self, weak toggle] _ in
                    self?.toggled(row.id, toggle?.isOn ?? false)
                }, for: .valueChanged)
                cell.accessories = [.customView(configuration: .init(customView: toggle, placement: .trailing()))]
            }
            cell.accessibilityIdentifier = "settings-\(row.id)"
        }
        let header = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, path in
            var c = UIListContentConfiguration.groupedHeader()
            c.text = self?.dataSource.snapshot().sectionIdentifiers[path.section]
            view.contentConfiguration = c
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, row in
            cv.dequeueConfiguredReusableCell(using: cell, for: path, item: row)
        }
        dataSource.supplementaryViewProvider = { cv, _, path in
            cv.dequeueConfiguredReusableSupplementary(using: header, for: path)
        }
        reload()
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        if dataSource != nil { reload() }
    }

    func reload() {
        guard dataSource != nil else { return }
        var s = NSDiffableDataSourceSnapshot<String, Row>()
        for section in sections() {
            s.appendSections([section.title])
            s.appendItems(section.rows)
        }
        dataSource.apply(s, animatingDifferences: false)
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        collectionView.deselectItem(at: path, animated: true)
        guard let row = dataSource.itemIdentifier(for: path) else { return }
        didSelect(row, at: path)
    }

    /// A row of overlapping colour dots, ringed so they read on any background.
    private static func swatchImage(_ colors: [UInt32]) -> UIImage {
        let d: CGFloat = 16, step: CGFloat = 10
        let size = CGSize(width: d + step * CGFloat(colors.count - 1), height: d)
        return UIGraphicsImageRenderer(size: size).image { ctx in
            for (i, argb) in colors.enumerated() {
                let rect = CGRect(x: step * CGFloat(i), y: 0, width: d, height: d).insetBy(dx: 0.5, dy: 0.5)
                UIColor(argb: argb).setFill()
                ctx.cgContext.fillEllipse(in: rect)
                UIColor(white: 0.5, alpha: 0.35).setStroke()
                ctx.cgContext.strokeEllipse(in: rect)
            }
        }.withRenderingMode(.alwaysOriginal)
    }
}
