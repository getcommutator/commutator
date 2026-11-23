import UIKit

/// The sign-in and machine screens' pieces, in the app's own language:
/// capsule buttons and the brand mark.
enum Controls {
    /// The filled capsule (text color on background): the one way forward.
    static func primaryButton(_ title: String, symbol: String? = nil, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.filled()
        c.title = title
        c.baseBackgroundColor = Palette.text
        c.baseForegroundColor = Palette.background
        c.cornerStyle = .capsule
        c.contentInsets = NSDirectionalEdgeInsets(top: 15, leading: 20, bottom: 15, trailing: 20)
        if let symbol {
            c.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .semibold))
            c.imagePadding = 8
        }
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansSemibold, 17)
            return a
        }
        return UIButton(configuration: c, primaryAction: UIAction { _ in action() })
    }

    /// A text-only button ("Sign out").
    static func plainButton(_ title: String, action: @escaping () -> Void) -> UIButton {
        var c = UIButton.Configuration.plain()
        c.title = title
        c.baseForegroundColor = Palette.secondary
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansMedium, 15)
            return a
        }
        return UIButton(configuration: c, primaryAction: UIAction { _ in action() })
    }

    static var vardogerMark: UIImage? {
        UIImage(named: "vardoger-logo")?.withRenderingMode(.alwaysTemplate)
    }

    static func symbol(_ name: String, _ size: CGFloat = 17, weight: UIImage.SymbolWeight = .medium) -> UIImage? {
        UIImage(systemName: name, withConfiguration: UIImage.SymbolConfiguration(pointSize: size, weight: weight))
    }
}
