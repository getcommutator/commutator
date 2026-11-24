import UIKit

/// Settings & utilities: account and machine, devices, appearance, archive.
final class MoreViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var collectionView: UICollectionView!
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

        enum Accessory: Hashable {
            case disclosure
            case none
            case check(Bool)
            case dot(Bool)
            case toggle(Bool)
        }
    }

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Settings"
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
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Row> { [weak self] cell, _, row in
            var c = UIListContentConfiguration.subtitleCell()
            c.text = row.title
            c.textProperties.font = Fonts.ui(.sansMedium, 16)
            c.textProperties.color = row.destructive ? Palette.danger : row.disabled ? Palette.tertiary : Palette.text
            c.secondaryText = row.subtitle
            c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
            c.secondaryTextProperties.color = Palette.secondary
            c.image = UIImage(systemName: row.symbol)
            c.imageProperties.tintColor = row.destructive ? Palette.danger : row.tint
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
        wallpaperObserver = NotificationCenter.default.addObserver(forName: WallpaperStore.didChange, object: nil, queue: .main) { [weak self] _ in self?.reload() }
        // Devices go on/offline after opening.
        appToken = app.observe { [weak self] in self?.reload() }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        if dataSource != nil { reload() }
    }

    private func toggled(_ id: String, _ on: Bool) {
        if id == "math:render" {
            MathSettings.render = on
            return reload()
        }
    }

    private var wallpaperObserver: NSObjectProtocol?
    private var appToken: AnyObject?

    private func reload() {
        var s = NSDiffableDataSourceSnapshot<String, Row>()
        s.appendSections(["Account"])
        s.appendItems([
            Row(id: "account", title: app.accountName, subtitle: app.accountDetail, symbol: "person.crop.circle", accessory: .none),
        ])
        // The machine this phone drives; the account's others are a tap away.
        if !app.isDemo, let host = app.host {
            s.appendSections(["Machine"])
            s.appendItems([Row(id: "machine", title: host.name, subtitle: "Switch to another machine", symbol: "desktopcomputer")])
        }
        s.appendSections(["Devices"])
        s.appendItems(app.hostOptions.map { Row(id: "device:\($0.id)", title: $0.name, subtitle: $0.online ? "Online" : "Offline", symbol: "desktopcomputer", accessory: .dot($0.online)) })
        s.appendSections(["Appearance"])
        let style = UserDefaults.standard.integer(forKey: "appearance")
        s.appendItems([
            Row(id: "appearance:0", title: "System", symbol: "circle.lefthalf.filled", accessory: .check(style == 0)),
            Row(id: "appearance:1", title: "Light", symbol: "sun.max", accessory: .check(style == 1)),
            Row(id: "appearance:2", title: "Dark", symbol: "moon", accessory: .check(style == 2)),
        ])
        s.appendSections(["Wallpaper"])
        var wall = [Row(id: "wallpaper:choose", title: WallpaperStore.isSet ? "Change Wallpaper…" : "Choose Wallpaper…", subtitle: WallpaperStore.isSet ? WallpaperStore.name : "Shown behind new chats and the sessions list", symbol: "photo")]
        if WallpaperStore.isSet {
            let effect = WallpaperStore.effect
            wall.append(Row(id: "wallpaper:effect", title: "Effect", subtitle: "\(WallpaperStore.label(effect)) — \(WallpaperStore.detail(effect))", symbol: "wand.and.stars"))
            wall.append(Row(id: "wallpaper:remove", title: "Remove Wallpaper", symbol: "trash", accessory: .none, destructive: true))
        }
        s.appendItems(wall)
        s.appendSections(["Math"])
        var math = [Row(id: "math:render", title: "Render Math", subtitle: "Off shows LaTeX as its source", symbol: "function", accessory: .toggle(MathSettings.render))]
        if MathSettings.render {
            let face = MathSettings.face
            math.append(Row(id: "math:font", title: "Math Font", subtitle: "\(MathSettings.label(face)) — \(MathSettings.detail(face))", symbol: "textformat"))
            math.append(Row(id: "math:size", title: "Math Size", subtitle: "\(MathSettings.size)% of the text around it", symbol: "textformat.size"))
        }
        s.appendItems(math)
        s.appendSections(["Activity"])
        s.appendItems(AppModel.SessionsView.allCases.map { view in
            Row(id: "view:" + view.rawValue, title: view.title, symbol: view.symbol, accessory: .check(app.sessionsView == view))
        })
        s.appendSections([" "])
        s.appendItems([Row(id: "signout", title: "Sign Out", symbol: "rectangle.portrait.and.arrow.right", accessory: .none, destructive: true)])
        dataSource.apply(s, animatingDifferences: false)
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        collectionView.deselectItem(at: path, animated: true)
        guard let row = dataSource.itemIdentifier(for: path) else { return }
        switch row.id {
        case "machine":
            let alert = UIAlertController(title: "Switch machine?", message: "You'll pick another of your machines.", preferredStyle: .actionSheet)
            alert.addAction(UIAlertAction(title: "Switch", style: .default) { [weak self] _ in self?.app.leaveMachine() })
            alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            alert.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(alert, animated: true)
        case let id where id.hasPrefix("view:"):
            AppModel.SessionsView(rawValue: String(id.dropFirst(5))).map { app.sessionsView = $0 }
        case let id where id.hasPrefix("appearance:"):
            let style = Int(id.dropFirst(11)) ?? 0
            UserDefaults.standard.set(style, forKey: "appearance")
            view.window?.overrideUserInterfaceStyle = UIUserInterfaceStyle(rawValue: style) ?? .unspecified
            reload()
        case "wallpaper:choose":
            WallpaperPicker.present(from: self)
        case "wallpaper:effect":
            let sheet = UIAlertController(title: "Wallpaper Effect", message: nil, preferredStyle: .actionSheet)
            for e in WallpaperStore.allEffects {
                let action = UIAlertAction(title: WallpaperStore.label(e), style: .default) { _ in WallpaperStore.effect = e }
                action.setValue(e == WallpaperStore.effect, forKey: "checked")
                sheet.addAction(action)
            }
            sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            sheet.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(sheet, animated: true)
        case "math:font":
            let sheet = UIAlertController(title: "Math Font", message: nil, preferredStyle: .actionSheet)
            for f in MathSettings.allFaces {
                let action = UIAlertAction(title: MathSettings.label(f), style: .default) { [weak self] _ in
                    MathSettings.face = f
                    self?.reload()
                }
                action.setValue(f == MathSettings.face, forKey: "checked")
                sheet.addAction(action)
            }
            sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            sheet.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(sheet, animated: true)
        case "math:size":
            let sheet = UIAlertController(title: "Math Size", message: nil, preferredStyle: .actionSheet)
            for size in MathSettings.allSizes {
                let action = UIAlertAction(title: "\(size)%", style: .default) { [weak self] _ in
                    MathSettings.size = size
                    self?.reload()
                }
                action.setValue(size == MathSettings.size, forKey: "checked")
                sheet.addAction(action)
            }
            sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            sheet.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(sheet, animated: true)
        case "wallpaper:remove":
            WallpaperStore.remove()
        case "signout":
            let alert = UIAlertController(title: "Sign out?", message: "Local drafts stay on this device.", preferredStyle: .actionSheet)
            alert.addAction(UIAlertAction(title: "Sign Out", style: .destructive) { [weak self] _ in self?.app.signOut() })
            alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            alert.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(alert, animated: true)
        default:
            break
        }
    }
}
