import UIKit

/// Settings → Appearance: light/dark mode, the light and the dark theme, the wallpaper.
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
        observers = [WallpaperStore.didChange, ThemeStore.didChange].map {
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
        var wall = [Row(id: "wallpaper:choose", title: WallpaperStore.isSet ? "Change Wallpaper…" : "Choose Wallpaper…", subtitle: WallpaperStore.isSet ? WallpaperStore.name : "Shown behind new chats and the sessions list", symbol: "photo")]
        if WallpaperStore.isSet {
            let effect = WallpaperStore.effect
            wall.append(Row(id: "wallpaper:effect", title: "Effect", subtitle: "\(WallpaperStore.label(effect)) — \(WallpaperStore.detail(effect))", symbol: "wand.and.stars"))
            wall.append(Row(id: "wallpaper:remove", title: "Remove Wallpaper", symbol: "trash", accessory: .none, destructive: true))
        }
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
            ("Wallpaper", wall),
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
        case "wallpaper:remove":
            WallpaperStore.remove()
        default:
            break
        }
    }
}

/// The built-in themes of one appearance, by family, with their colours; a tap applies one.
final class ThemePickerViewController: SettingsListController {
    private let dark: Bool

    init(dark: Bool) {
        self.dark = dark
        super.init(nibName: nil, bundle: nil)
        title = dark ? "Dark Theme" : "Light Theme"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func sections() -> [(title: String, rows: [Row])] {
        let current = dark ? ThemeStore.darkId : ThemeStore.lightId
        return ThemeStore.families.compactMap { family in
            let rows = family.variants.filter { $0.dark == dark }.map {
                Row(id: "variant:\($0.id)", title: $0.name, symbol: "circle", accessory: .check($0.id == current), swatches: $0.swatches)
            }
            return rows.isEmpty ? nil : (family.name, rows)
        }
    }

    override func didSelect(_ row: Row, at path: IndexPath) {
        let id = String(row.id.dropFirst("variant:".count))
        if dark { ThemeStore.select(darkId: id) } else { ThemeStore.select(lightId: id) }
        reload()
    }
}
