import UIKit

/// Settings & utilities: account and machine, devices, appearance, math, activity.
final class MoreViewController: SettingsListController {
    private let app: AppModel

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Settings"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        themeObserver = NotificationCenter.default.addObserver(forName: ThemeStore.didChange, object: nil, queue: .main) { [weak self] _ in self?.reload() }
        // Devices go on/offline after opening.
        appToken = app.observe { [weak self] in self?.reload() }
    }

    override func toggled(_ id: String, _ on: Bool) {
        if id == "math:render" {
            MathSettings.render = on
            return reload()
        }
    }

    private var themeObserver: NSObjectProtocol?
    private var appToken: AnyObject?

    override func sections() -> [(title: String, rows: [Row])] {
        var out: [(title: String, rows: [Row])] = [
            ("Account", [Row(id: "account", title: app.accountName, subtitle: app.accountDetail, symbol: "person.crop.circle", accessory: .none)]),
        ]
        // The machine this phone drives; the account's others are a tap away.
        if !app.isDemo, let host = app.host {
            out.append(("Machine", [Row(id: "machine", title: host.name, subtitle: "Switch to another machine", symbol: "desktopcomputer")]))
        }
        out.append(("Devices", app.hostOptions.map { Row(id: "device:\($0.id)", title: $0.name, subtitle: $0.online ? "Online" : "Offline", symbol: "desktopcomputer", accessory: .dot($0.online)) }))
        out.append(("Display", [Row(id: "appearance", title: "Appearance", subtitle: AppearanceViewController.summary(traits: traitCollection), symbol: "paintpalette")]))
        var math = [Row(id: "math:render", title: "Render Math", subtitle: "Off shows LaTeX as its source", symbol: "function", accessory: .toggle(MathSettings.render))]
        if MathSettings.render {
            let face = MathSettings.face
            math.append(Row(id: "math:font", title: "Math Font", subtitle: "\(MathSettings.label(face)) — \(MathSettings.detail(face))", symbol: "textformat"))
            math.append(Row(id: "math:size", title: "Math Size", subtitle: "\(MathSettings.size)% of the text around it", symbol: "textformat.size"))
        }
        out.append(("Math", math))
        out.append(("Activity", AppModel.SessionsView.allCases.map { view in
            Row(id: "view:" + view.rawValue, title: view.title, symbol: view.symbol, accessory: .check(app.sessionsView == view))
        }))
        out.append((" ", [Row(id: "signout", title: "Sign Out", symbol: "rectangle.portrait.and.arrow.right", accessory: .none, destructive: true)]))
        return out
    }

    override func didSelect(_ row: Row, at path: IndexPath) {
        switch row.id {
        case "machine":
            let alert = UIAlertController(title: "Switch machine?", message: "You'll pick another of your machines.", preferredStyle: .actionSheet)
            alert.addAction(UIAlertAction(title: "Switch", style: .default) { [weak self] _ in self?.app.leaveMachine() })
            alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            alert.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(alert, animated: true)
        case "appearance":
            navigationController?.pushViewController(AppearanceViewController(), animated: true)
        case let id where id.hasPrefix("view:"):
            AppModel.SessionsView(rawValue: String(id.dropFirst(5))).map { app.sessionsView = $0 }
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
