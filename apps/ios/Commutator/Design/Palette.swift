import UIKit

/// App palette: the selected theme's colours (`ThemeStore`, resolved in the core from the
/// desktop's themes), light or dark by the trait. Colors are paint-only — nothing here affects
/// layout.
enum Palette {
    static func dynamic(_ light: UInt32, _ dark: UInt32, alpha: CGFloat = 1) -> UIColor {
        UIColor { traits in
            UIColor(hex: traits.userInterfaceStyle == .dark ? dark : light, alpha: alpha)
        }
    }

    /// A colour of the selected theme, for the trait's light/dark appearance.
    static func themed(_ pick: @escaping (PhonePalette) -> UInt32) -> UIColor {
        UIColor { UIColor(argb: pick(ThemeStore.palette(dark: $0.userInterfaceStyle == .dark))) }
    }

    /// A theme colour at `alpha`.
    static func themed(alpha: CGFloat, _ pick: @escaping (PhonePalette) -> UInt32) -> UIColor {
        UIColor { UIColor(argb: pick(ThemeStore.palette(dark: $0.userInterfaceStyle == .dark))).withAlphaComponent(alpha) }
    }

    /// What a view that restyles by hand (layer colours, cached images) listens to: the
    /// appearance and the theme.
    static let colorTraits: [UITrait] = [UITraitUserInterfaceStyle.self, ThemeRevisionTrait.self]

    static let background = themed { $0.app.background }
    /// The open session's row in the iPad sidebar.
    static let rowActive = themed { $0.app.rowActive }
    static let elevated = themed { $0.app.elevated }
    static let text = themed { $0.app.text }
    static let secondary = themed { $0.app.secondary }
    static let tertiary = themed { $0.app.tertiary }
    static let hairline = themed { $0.app.hairline }
    static let accent = themed { $0.app.accent }
    /// A switch's on track: the dark accent is too light for the white thumb.
    static let toggleOn = themed { $0.app.toggleOn }
    /// Labels and glyphs on an accent fill.
    static let onAccent = themed { $0.app.onAccent }
    /// Translucent control fill that reads on glass in both modes.
    static let controlFill = themed { $0.app.controlFill }
    static let accentSoft = themed { $0.app.accentSoft }
    static let danger = themed { $0.app.danger }
    static let success = themed { $0.app.success }
    static let warning = themed { $0.app.warning }
    static let userBubble = themed { $0.app.userBubble }
    static let codeBackground = themed { $0.app.codeBackground }
    static let codeBorder = themed { $0.app.codeBorder }
    static let chip = themed { $0.app.chip }

    /// Project tones — the desktop's monogram palette (project_icon.rs):
    /// slate, blue, violet, rose, amber, emerald, teal, orange. Indexed by the
    /// core's `project_color_index` (FNV-1a of the project path).
    static let projectDots: [UIColor] = [
        dynamic(0x475569, 0x94A3B8), dynamic(0x2563EB, 0x93C5FD), dynamic(0x7C3AED, 0xC4B5FD),
        dynamic(0xBE123C, 0xFDA4AF), dynamic(0xA16207, 0xFCD34D), dynamic(0x047857, 0x6EE7B7),
        dynamic(0x0F766E, 0x5EEAD4), dynamic(0xC2410C, 0xFDBA74),
    ]

    /// Desktop sidebar "subline": `text_muted` @ 0.5 (project label, branch).
    static let subline = themed { $0.app.subline }

    static func dual(_ light: UIColor, _ dark: UIColor) -> UIColor {
        UIColor { $0.userInterfaceStyle == .dark ? dark : light }
    }

    /// A transcript role in the selected theme.
    static func color(_ role: ColorRole) -> UIColor {
        UIColor { UIColor(argb: ThemeStore.color(role, dark: $0.userInterfaceStyle == .dark)) }
    }
}

extension UIColor {
    convenience init(hex: UInt32, alpha: CGFloat = 1) {
        self.init(
            red: CGFloat((hex >> 16) & 0xFF) / 255,
            green: CGFloat((hex >> 8) & 0xFF) / 255,
            blue: CGFloat(hex & 0xFF) / 255,
            alpha: alpha
        )
    }
}

/// Dynamic Type for fixed-height UI: one factor per content size category
/// (rows stay fixed-height, so lists still skip self-sizing).
enum TypeScale {
    static var factor: CGFloat {
        min(1.6, max(0.85, UIFontMetrics(forTextStyle: .body).scaledValue(for: 17) / 17))
    }

    static func size(_ base: CGFloat) -> CGFloat { (base * factor).rounded(.toNearestOrAwayFromZero) }
}
