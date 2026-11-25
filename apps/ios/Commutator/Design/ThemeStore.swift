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
                refreshTitles(in: window)
            }
        }
        NotificationCenter.default.post(name: didChange, object: nil)
    }

    private static var revision = 0

    /// Navigation bars resolve their title colours once, when the attributes are set, so a theme
    /// change would keep the old theme's title: set them again (nil first, or an equal value is skipped).
    private static func refreshTitles(in view: UIView) {
        if let bar = view as? UINavigationBar {
            let title = bar.titleTextAttributes, large = bar.largeTitleTextAttributes
            bar.titleTextAttributes = nil
            bar.largeTitleTextAttributes = nil
            bar.titleTextAttributes = title
            bar.largeTitleTextAttributes = large
        }
        for sub in view.subviews { refreshTitles(in: sub) }
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
