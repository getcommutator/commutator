import UIKit

/// Settings → Appearance: light/dark mode, the light and the dark theme.
final class AppearanceViewController: SettingsListController {
    init() {
        super.init(nibName: nil, bundle: nil)
        title = "Appearance"
    }

    required init?(coder: NSCoder) { fatalError() }

    /// "Dark · Dracula": the mode and the family of the theme on screen now.
    static func summary(traits: UITraitCollection) -> String {
        let style = UserDefaults.standard.integer(forKey: "appearance")
        let mode = ["System", "Light", "Dark"][min(max(style, 0), 2)]
        let dark = style == 2 || style == 0 && traits.userInterfaceStyle == .dark
        let id = dark ? ThemeStore.darkId : ThemeStore.lightId
        return "\(mode) · \(ThemeStore.family(of: id)?.name ?? "Commutator")"
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        observers = [ThemeStore.didChange].map {
            NotificationCenter.default.addObserver(forName: $0, object: nil, queue: .main) { [weak self] _ in self?.reload() }
        }
    }

    private var observers: [NSObjectProtocol] = []

    private static func variantName(_ id: String) -> String {
        ThemeStore.variant(id)?.name ?? id
    }

    override func sections() -> [(title: String, rows: [Row])] {
        let style = UserDefaults.standard.integer(forKey: "appearance")
        let light = ThemeStore.lightId, dark = ThemeStore.darkId
        return [
            ("Mode", [
                Row(id: "appearance:0", title: "System", symbol: "circle.lefthalf.filled", accessory: .check(style == 0)),
                Row(id: "appearance:1", title: "Light", symbol: "sun.max", accessory: .check(style == 1)),
                Row(id: "appearance:2", title: "Dark", symbol: "moon", accessory: .check(style == 2)),
            ]),
            ("Theme", [
                Row(id: "theme:light", title: "Light Theme", subtitle: Self.variantName(light), symbol: "sun.max",
                    swatches: ThemeStore.variant(light)?.swatches ?? []),
                Row(id: "theme:dark", title: "Dark Theme", subtitle: Self.variantName(dark), symbol: "moon",
                    swatches: ThemeStore.variant(dark)?.swatches ?? []),
            ]),
        ]
    }

    override func didSelect(_ row: Row, at path: IndexPath) {
        switch row.id {
        case let id where id.hasPrefix("appearance:"):
            let style = Int(id.dropFirst(11)) ?? 0
            UserDefaults.standard.set(style, forKey: "appearance")
            view.window?.overrideUserInterfaceStyle = UIUserInterfaceStyle(rawValue: style) ?? .unspecified
            reload()
        case "theme:light", "theme:dark":
            navigationController?.pushViewController(ThemePickerViewController(dark: row.id == "theme:dark"), animated: true)
        default:
            break
        }
    }
}

/// The built-in themes of one appearance as a grid of preview cards (each painted in its own
/// colours), Commutator's first; a tap applies one. Filterable by name.
final class ThemePickerViewController: UIViewController, UICollectionViewDelegate, UISearchResultsUpdating {
    private let dark: Bool
    /// (variant, its palette, its section), in desktop order.
    private let themes: [(info: ThemeVariantInfo, palette: PhonePalette, section: String)]
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<String, String>!
    private var query = ""

    init(dark: Bool) {
        self.dark = dark
        themes = ThemeStore.families.flatMap(\.variants).filter { $0.dark == dark }.map {
            ($0, themePalette(variantId: $0.id, dark: dark), $0.id.hasPrefix("commutator-") ? "Default" : "Built-in")
        }
        super.init(nibName: nil, bundle: nil)
        title = dark ? "Dark Theme" : "Light Theme"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        let search = UISearchController(searchResultsController: nil)
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        search.searchBar.placeholder = "Filter themes"
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = false

        let item = NSCollectionLayoutItem(layoutSize: .init(widthDimension: .fractionalWidth(0.5), heightDimension: .fractionalHeight(1)))
        item.contentInsets = .init(top: 0, leading: 2, bottom: 0, trailing: 2)
        let group = NSCollectionLayoutGroup.horizontal(layoutSize: .init(widthDimension: .fractionalWidth(1), heightDimension: .absolute(ThemeCardCell.height)), subitems: [item])
        let section = NSCollectionLayoutSection(group: group)
        section.interGroupSpacing = 2
        section.contentInsets = .init(top: 0, leading: 12, bottom: 16, trailing: 12)
        let header = NSCollectionLayoutBoundarySupplementaryItem(layoutSize: .init(widthDimension: .fractionalWidth(1), heightDimension: .absolute(36)), elementKind: UICollectionView.elementKindSectionHeader, alignment: .top)
        section.boundarySupplementaryItems = [header]
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout(section: section))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.keyboardDismissMode = .onDrag
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<ThemeCardCell, String> { [weak self] cell, _, id in
            guard let self, let theme = themes.first(where: { $0.info.id == id }) else { return }
            cell.configure(name: theme.info.name, palette: theme.palette, selected: id == currentId)
            cell.accessibilityIdentifier = "settings-variant:\(id)"
        }
        let headerReg = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, path in
            var c = UIListContentConfiguration.header()
            c.text = self?.dataSource.snapshot().sectionIdentifiers[path.section]
            c.textProperties.font = Fonts.ui(.sansSemibold, 17)
            c.textProperties.color = Palette.text
            c.directionalLayoutMargins.leading = 8
            view.contentConfiguration = c
            view.backgroundConfiguration = .clear()
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, id in
            cv.dequeueConfiguredReusableCell(using: cell, for: path, item: id)
        }
        dataSource.supplementaryViewProvider = { cv, _, path in
            cv.dequeueConfiguredReusableSupplementary(using: headerReg, for: path)
        }
        reload()
    }

    private var currentId: String { dark ? ThemeStore.darkId : ThemeStore.lightId }

    private func reload() {
        var s = NSDiffableDataSourceSnapshot<String, String>()
        let shown = themes.filter { query.isEmpty || $0.info.name.localizedCaseInsensitiveContains(query) }
        for title in ["Default", "Built-in"] {
            let ids = shown.filter { $0.section == title }.map(\.info.id)
            guard !ids.isEmpty else { continue }
            s.appendSections([title])
            s.appendItems(ids)
        }
        s.reconfigureItems(s.itemIdentifiers)
        dataSource.apply(s, animatingDifferences: false)
    }

    func updateSearchResults(for searchController: UISearchController) {
        query = searchController.searchBar.text?.trimmingCharacters(in: .whitespaces) ?? ""
        reload()
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        guard let id = dataSource.itemIdentifier(for: path) else { return }
        if dark { ThemeStore.select(darkId: id) } else { ThemeStore.select(lightId: id) }
        reload()
    }
}

/// One theme's preview: its background with "Aa" in its text, a `~/` chip in its accent and a
/// cursor, and its eight ANSI colours; the name underneath. The selected card is ringed.
private final class ThemeCardCell: UICollectionViewCell {
    static let cardHeight: CGFloat = 102
    static let height: CGFloat = cardHeight + 24

    private let ring = UIView()
    private let card = UIView()
    private let sample = UILabel()
    private let chip = UILabel()
    private let cursor = UILabel()
    private let dots = DotsView()
    private let name = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        ring.layer.borderWidth = 3
        ring.layer.cornerRadius = 27
        ring.layer.cornerCurve = .continuous
        card.layer.cornerRadius = 22
        card.layer.cornerCurve = .continuous
        card.layer.borderWidth = 1
        sample.text = "Aa"
        sample.font = Fonts.ui(.sans, 26)
        chip.text = "~/"
        chip.font = Fonts.ui(.mono, 16)
        chip.textAlignment = .center
        chip.layer.cornerRadius = 7
        chip.layer.masksToBounds = true
        cursor.text = "_"
        cursor.font = Fonts.ui(.monoSemibold, 14)
        cursor.textAlignment = .center
        cursor.baselineAdjustment = .none
        cursor.layer.cornerRadius = 6
        cursor.layer.masksToBounds = true
        name.font = Fonts.ui(.sansMedium, 14)
        name.textAlignment = .center
        for v in [ring, card, name] { contentView.addSubview(v) }
        for v in [sample, chip, cursor, dots] as [UIView] { card.addSubview(v) }
        isAccessibilityElement = true
        accessibilityTraits = .button
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(name text: String, palette p: PhonePalette, selected: Bool) {
        let bg = UIColor(argb: p.app.background), accent = UIColor(argb: p.app.accent)
        card.backgroundColor = bg
        card.layer.borderColor = UIColor(white: 0.5, alpha: 0.2).cgColor
        sample.textColor = UIColor(argb: p.app.text)
        // The accent is the card's biggest colour: a solid chip, the cursor beside it in the text colour.
        chip.textColor = UIColor(argb: p.app.onAccent)
        chip.backgroundColor = accent
        cursor.backgroundColor = UIColor(argb: p.app.text).withAlphaComponent(0.85)
        cursor.textColor = bg
        dots.colors = p.terminal.ansi.prefix(8).map { UIColor(argb: $0) }
        dots.separator = bg
        // The app's own accent, so the ring reads on the page around the card.
        ring.layer.borderColor = selected ? Palette.accent.resolvedColor(with: traitCollection).cgColor : UIColor.clear.cgColor
        name.text = text
        name.textColor = selected ? Palette.text : Palette.secondary
        accessibilityLabel = text
        accessibilityTraits = selected ? [.button, .selected] : .button
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let w = contentView.bounds.width
        card.frame = CGRect(x: 5, y: 5, width: w - 10, height: Self.cardHeight - 10)
        ring.frame = card.frame.insetBy(dx: -5, dy: -5)
        let cw = card.bounds.width, pad: CGFloat = 14
        sample.frame = CGRect(x: pad, y: 9, width: 54, height: 40)
        cursor.frame = CGRect(x: cw - pad - 20, y: 11, width: 20, height: 34)
        chip.frame = CGRect(x: cursor.frame.minX - 9 - 40, y: 12, width: 40, height: 32)
        dots.frame = CGRect(x: pad - 2, y: card.bounds.height - pad - 23, width: cw - 2 * pad + 4, height: 23)
        name.frame = CGRect(x: 0, y: Self.cardHeight + 1, width: w, height: 19)
    }
}

/// Overlapping colour dots, each cut out of the one before by a ring of the card's colour.
private final class DotsView: UIView {
    var colors: [UIColor] = [] { didSet { setNeedsDisplay() } }
    var separator: UIColor = .clear { didSet { setNeedsDisplay() } }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        contentMode = .redraw
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ rect: CGRect) {
        guard colors.count > 1, let ctx = UIGraphicsGetCurrentContext() else { return }
        let d = bounds.height, step = min(d * 0.8, (bounds.width - d) / CGFloat(colors.count - 1))
        for (i, color) in colors.enumerated() {
            let circle = CGRect(x: step * CGFloat(i), y: 0, width: d, height: d)
            separator.setFill()
            ctx.fillEllipse(in: circle)
            color.setFill()
            ctx.fillEllipse(in: circle.insetBy(dx: 1.5, dy: 1.5))
        }
    }
}
