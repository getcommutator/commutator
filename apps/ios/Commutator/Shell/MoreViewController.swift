import UIKit
import UserNotifications

/// Settings & utilities: account, devices, notifications, appearance, math, activity.
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
        appToken = app.observe { [weak self] in self?.reload() }
        // Back from iOS Settings (notifications may have been allowed there).
        foregroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.refreshNotificationStatus()
        }
    }

    private var foregroundObserver: NSObjectProtocol?

    /// The account's machines (devices running an engine), online first.
    private var devices: [ServerDevice] = []

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        refreshNotificationStatus()
        // ponytail: fetched on each appearance, not live; a device going on/offline shows on the next visit
        guard !app.isDemo else { return }
        Task { @MainActor in
            guard let all = try? await app.machines() else { return }
            devices = all.filter(\.isMachine).sorted { ($0.online ? 0 : 1, $0.name) < ($1.online ? 0 : 1, $1.name) }
            reload()
        }
    }

    // MARK: Notifications

    private var notificationStatus: UNAuthorizationStatus = .notDetermined

    private func refreshNotificationStatus() {
        Task { @MainActor in
            let status = await PushNotifications.shared.authorizationStatus()
            guard status != self.notificationStatus else { return }
            self.notificationStatus = status
            self.reload()
        }
    }

    /// On when this phone wants them and iOS allows them.
    private var notificationsOn: Bool {
        PushNotifications.shared.enabled && (notificationStatus == .authorized || notificationStatus == .provisional || notificationStatus == .ephemeral)
    }

    /// The Notifications row's summary.
    private var notificationSummary: String {
        if notificationStatus == .denied { return "Off in iOS Settings" }
        return notificationsOn ? "On" : "Off"
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
        // The account's machines (its server's list): the one this phone drives checked, a tap on another switches.
        if !devices.isEmpty {
            out.append(("Devices", devices.map { d in
                let driving = d.id == app.host?.id
                let seen = relativeTimeLabel(atMs: d.lastSeen * 1000, nowMs: Int64(Date().timeIntervalSince1970 * 1000))
                return Row(
                    id: "device:\(d.id)",
                    title: d.name,
                    subtitle: driving ? "Connected" : d.online ? "Online" : "Offline · last seen \(seen)",
                    symbol: "desktopcomputer",
                    accessory: driving ? .check(true) : .dot(d.online)
                )
            }))
        }
        var display = [Row(id: "appearance", title: "Appearance", subtitle: AppearanceViewController.summary(traits: traitCollection), symbol: "paintpalette")]
        if !app.isDemo { display.append(Row(id: "notifications", title: "Notifications", subtitle: notificationSummary, symbol: "bell")) }
        out.append(("Display", display))
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
        case let id where id.hasPrefix("device:"):
            guard let d = devices.first(where: { "device:\($0.id)" == id }), d.id != app.host?.id else { return }
            let alert = UIAlertController(title: "Switch to \(d.name)?", message: d.online ? nil : "It's offline: its chats load once it's back.", preferredStyle: .actionSheet)
            alert.addAction(UIAlertAction(title: "Switch", style: .default) { [weak self] _ in self?.app.switchMachine(to: d) })
            alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            alert.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(alert, animated: true)
        case "appearance":
            navigationController?.pushViewController(AppearanceViewController(), animated: true)
        case "notifications":
            navigationController?.pushViewController(NotificationsViewController(app: app), animated: true)
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
