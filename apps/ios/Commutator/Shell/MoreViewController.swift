import UIKit
import UserNotifications

/// Settings & utilities: account, devices, appearance, chat, notifications, activity.
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
        mathObserver = NotificationCenter.default.addObserver(forName: MathSettings.didChange, object: nil, queue: .main) { [weak self] _ in self?.reload() }
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
        loadUsage()
    }

    // MARK: Usage

    /// The connected machine's active agent logins that report plan usage.
    private var usage: [DeviceAccountInfo] = []

    /// Last known usage first (no network), then a fresh probe.
    private func loadUsage() {
        guard let client = app.client, let host = app.host?.id else { return }
        Task { @MainActor in
            for force in [false, true] {
                guard let all = try? await client.deviceAgentAccounts(deviceId: host, forceUsage: force) else { return }
                usage = all.filter { $0.active && (!$0.usage.isEmpty || $0.usageError != nil) }
                reload()
            }
        }
    }

    private func usageRow(_ a: DeviceAccountInfo) -> Row {
        let agent = agentLoginRules(harness: a.harness).name
        let title = [agent.isEmpty ? a.harness : agent, a.email ?? a.displayName].compactMap { $0 }.joined(separator: " · ")
        let resets = RelativeDateTimeFormatter()
        resets.unitsStyle = .short
        let windows = a.usage.map { w in
            let pct = "\(w.label) \(Int((w.usedFraction * 100).rounded()))%"
            guard let at = w.resetsAtMs else { return pct }
            return pct + ", resets " + resets.localizedString(for: Date(timeIntervalSince1970: Double(at) / 1000), relativeTo: Date())
        }
        let worst = a.usage.map(\.usedFraction).max() ?? 0
        // Same thresholds as the desktop's Accounts meters.
        let tint = worst >= 0.95 ? Palette.danger : worst >= 0.80 ? Palette.warning : Palette.accent
        return Row(id: "usage:\(a.id)", title: title, subtitle: a.usageError ?? windows.joined(separator: " · "), symbol: "gauge.with.dots.needle.50percent", tint: tint, accessory: .none)
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

    private var themeObserver: NSObjectProtocol?
    private var mathObserver: NSObjectProtocol?
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
        if !usage.isEmpty { out.append(("Usage", usage.map(usageRow))) }
        var display = [
            Row(id: "appearance", title: "Appearance", subtitle: AppearanceViewController.summary(traits: traitCollection), symbol: "paintpalette"),
            Row(id: "chat", title: "Chat", subtitle: ChatSettingsViewController.summary, symbol: "text.bubble"),
        ]
        if !app.isDemo { display.append(Row(id: "notifications", title: "Notifications", subtitle: notificationSummary, symbol: "bell")) }
        out.append(("Display", display))
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
        case "chat":
            navigationController?.pushViewController(ChatSettingsViewController(), animated: true)
        case "notifications":
            navigationController?.pushViewController(NotificationsViewController(app: app), animated: true)
        case let id where id.hasPrefix("view:"):
            AppModel.SessionsView(rawValue: String(id.dropFirst(5))).map { app.sessionsView = $0 }
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
