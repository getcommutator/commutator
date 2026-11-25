import UIKit

/// The selected light and dark themes (desktop built-ins, resolved by the core) and their
/// palettes. Stored on this device only. `Palette` reads from here, so a change repaints
/// everything that resolves its colours dynamically.
enum ThemeStore {
    static let didChange = Notification.Name("ThemeStore.didChange")
    static let defaultLight = "commutator-light"
    static let defaultDark = "commutator-dark"

    static var lightId: String { UserDefaults.standard.string(forKey: "theme.light") ?? defaultLight }
    static var darkId: String { UserDefaults.standard.string(forKey: "theme.dark") ?? defaultDark }

    /// The built-in families (desktop order), for the pickers.
    static let families: [ThemeFamilyInfo] = themeFamilies()

    /// The family a variant belongs to ("Dracula", "Commutator"), if it's built in.
    static func family(of variantId: String) -> ThemeFamilyInfo? {
        families.first { $0.variants.contains { $0.id == variantId } }
    }

    static func variant(_ id: String) -> ThemeVariantInfo? {
        families.lazy.flatMap(\.variants).first { $0.id == id }
    }

    private static let lock = NSLock()
    private static var palettes: (light: PhonePalette, dark: PhonePalette)?
    /// A role's index in `PhonePalette.roles`.
    private static let roleIndex: [ColorRole: Int] = Dictionary(
        uniqueKeysWithValues: colorRoles().enumerated().map { ($1, $0) }
    )

    /// The selected palette for `dark` (dynamic colours call this on every resolve).
    static func palette(dark: Bool) -> PhonePalette {
        lock.lock()
        defer { lock.unlock() }
        if palettes == nil { palettes = loadPalettes() }
        return dark ? palettes!.dark : palettes!.light
    }

    static func color(_ role: ColorRole, dark: Bool) -> UInt32 {
        palette(dark: dark).roles[roleIndex[role] ?? 0]
    }

    /// Reads the selection and hands it to the core (its visuals and terminal paint with it).
    static func load() {
        lock.lock()
        palettes = loadPalettes()
        lock.unlock()
    }

    private static func loadPalettes() -> (light: PhonePalette, dark: PhonePalette) {
        setThemeSelection(lightId: lightId, darkId: darkId)
        return (themePalette(variantId: lightId, dark: false), themePalette(variantId: darkId, dark: true))
    }

    /// Picks a theme for light and/or dark and repaints every window.
    static func select(lightId: String? = nil, darkId: String? = nil) {
        if let lightId { UserDefaults.standard.set(lightId, forKey: "theme.light") }
        if let darkId { UserDefaults.standard.set(darkId, forKey: "theme.dark") }
        load()
        revision += 1
        for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
            for window in scene.windows {
                window.traitOverrides[ThemeRevisionTrait.self] = revision
                window.repaintThemedText()
                // Other tabs and covered screens aren't in the window: reach them through the
                // controllers, or their labels and large titles keep the old theme.
                window.rootViewController.map(repaint)
            }
        }
        NotificationCenter.default.post(name: didChange, object: nil)
    }

    private static var revision = 0

    private static func repaint(_ vc: UIViewController) {
        if vc.isViewLoaded, vc.view.window == nil { vc.view.repaintThemedText() }
        if let nav = vc as? UINavigationController, nav.navigationBar.window == nil { nav.navigationBar.repaintThemedText() }
        vc.children.forEach(repaint)
        (vc as? UITabBarController)?.viewControllers?.forEach { if $0.parent == nil { repaint($0) } }
        vc.presentedViewController.map(repaint)
    }
}

extension UIView {
    /// Labels and navigation bars resolve their colours once, when they're set, and setting an
    /// equal value is skipped, so a theme change (the colour objects don't change, only what
    /// they resolve to) would keep the old theme's text. Set them again, nil first.
    func repaintThemedText() {
        if let label = self as? UILabel, let text = label.attributedText {
            label.attributedText = nil
            label.attributedText = text
        } else if let bar = self as? UINavigationBar {
            let title = bar.titleTextAttributes, large = bar.largeTitleTextAttributes
            bar.titleTextAttributes = nil
            bar.largeTitleTextAttributes = nil
            bar.titleTextAttributes = title
            bar.largeTitleTextAttributes = large
        }
        for sub in subviews { sub.repaintThemedText() }
    }
}

/// Bumped on every theme change. It affects colour appearance, so dynamic colours re-resolve;
/// views that restyle on `userInterfaceStyle` also listen for it (`Palette.colorTraits`).
struct ThemeRevisionTrait: UITraitDefinition {
    static let defaultValue = 0
    static let affectsColorAppearance = true
}

extension UIColor {
    /// 0xAARRGGBB.
    convenience init(argb: UInt32) {
        self.init(
            red: CGFloat((argb >> 16) & 0xFF) / 255,
            green: CGFloat((argb >> 8) & 0xFF) / 255,
            blue: CGFloat(argb & 0xFF) / 255,
            alpha: CGFloat((argb >> 24) & 0xFF) / 255
        )
    }
}
