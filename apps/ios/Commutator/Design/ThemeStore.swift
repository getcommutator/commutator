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
            for window in scene.windows { window.traitOverrides[ThemeRevisionTrait.self] = revision }
        }
        NotificationCenter.default.post(name: didChange, object: nil)
    }

    private static var revision = 0
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
