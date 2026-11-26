import UIKit
import UserNotifications

/// Settings → Notifications: session notifications (the master switch, then one per kind)
/// and the agents' Live Activity.
final class NotificationsViewController: SettingsListController {
    private let app: AppModel

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Notifications"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        // Back from iOS Settings (notifications may have been allowed there).
        foregroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.refreshStatus()
        }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        refreshStatus()
    }

    private var foregroundObserver: NSObjectProtocol?
    private var status: UNAuthorizationStatus = .notDetermined

    private func refreshStatus() {
        Task { @MainActor in
            let status = await PushNotifications.shared.authorizationStatus()
            guard status != self.status else { return }
            self.status = status
            self.reload()
        }
    }

    /// On when this phone wants them and iOS allows them.
    private var on: Bool {
        PushNotifications.shared.enabled && (status == .authorized || status == .provisional || status == .ephemeral)
    }

    override func sections() -> [(title: String, rows: [Row])] {
        var rows = [Row(id: "notify:enabled", title: "Session Notifications", subtitle: status == .denied ? "Turned off in iOS Settings" : "When a session finishes, needs you or fails", symbol: "bell", accessory: .toggle(on))]
        if on {
            rows += PushNotifications.Kind.allCases.map { kind in
                let symbol: String = switch kind {
                case .done: "checkmark.circle"
                case .input: "questionmark.bubble"
                case .failed: "exclamationmark.triangle"
                }
                return Row(id: "notify:\(kind.rawValue)", title: kind.title, symbol: symbol, accessory: .toggle(PushNotifications.shared.isOn(kind)))
            }
        }
        let live = LiveActivities.shared
        let activity = Row(
            id: "liveActivity", title: "Live Activity",
            subtitle: live.allowed ? "Agents on the Lock Screen" : "Turned off in iOS Settings",
            symbol: "platter.filled.top.iphone", accessory: .toggle(live.enabled && live.allowed)
        )
        return [("Sessions", rows), ("Live Activity", [activity])]
    }

    override func toggled(_ id: String, _ on: Bool) {
        if id == "liveActivity" {
            if on, !LiveActivities.shared.allowed {
                // Only the Settings app can turn them back on.
                if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                return reload()
            }
            LiveActivities.shared.enabled = on
            return reload()
        }
        let push = PushNotifications.shared
        if id == "notify:enabled" {
            guard on else {
                push.enabled = false
                return reload()
            }
            if status == .denied {
                // Only the Settings app can turn them back on.
                let alert = UIAlertController(title: "Notifications are off", message: "Allow notifications for Commutator in iOS Settings.", preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "Not Now", style: .cancel) { [weak self] _ in self?.reload() })
                alert.addAction(UIAlertAction(title: "Open Settings", style: .default) { [weak self] _ in
                    if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                    self?.reload()
                })
                return present(alert, animated: true)
            }
            push.enabled = true
            Task { @MainActor in
                await push.requestPermission()
                self.status = await push.authorizationStatus()
                self.reload()
            }
            return
        }
        if let kind = PushNotifications.Kind(rawValue: String(id.dropFirst("notify:".count))) {
            push.set(kind, on)
        }
    }
}
