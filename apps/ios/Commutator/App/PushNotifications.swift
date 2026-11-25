import UIKit
import UserNotifications

/// Session notifications, like the desktop's: a run finished, a session is
/// waiting on you, a run failed. The machine's engine decides and the server
/// sends them (APNs, `apps/commutator-server/src/push.rs`); this asks for
/// permission, hands the server this phone's token, stays quiet while the app
/// is open (the desktop's "only in the background"), and opens the session a
/// notification is about.
@MainActor
final class PushNotifications: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PushNotifications()

    private weak var app: AppModel?
    private var token: String?
    /// A tapped notification's chat, until the app can show it.
    private var pendingChat: String?
    /// Opens a chat once the shell is up (set by the scene).
    var openChat: ((String) -> Bool)?

    private static let askedKey = "notifications.asked"

    // MARK: Lifecycle

    func install(app: AppModel) {
        self.app = app
        UNUserNotificationCenter.current().delegate = self
    }

    /// Signed in (or launched signed in): re-register if already allowed.
    /// Tokens can change between launches; the server keeps the latest.
    func signedIn() {
        Task {
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            if Self.allowed(settings.authorizationStatus) {
                UIApplication.shared.registerForRemoteNotifications()
            }
        }
    }

    /// The first time a session is started from this phone: the moment the
    /// ask makes sense ("tell me when it's done").
    func askAfterFirstSession() {
        guard app?.isDemo == false, !UserDefaults.standard.bool(forKey: Self.askedKey) else { return }
        UserDefaults.standard.set(true, forKey: Self.askedKey)
        Task {
            let center = UNUserNotificationCenter.current()
            let granted = (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
            if granted { UIApplication.shared.registerForRemoteNotifications() }
        }
    }

    private static func allowed(_ status: UNAuthorizationStatus) -> Bool {
        status == .authorized || status == .provisional || status == .ephemeral
    }

    /// Signing out removes this phone on the server, its push target with it.
    func signingOut() {
        token = nil
        UIApplication.shared.unregisterForRemoteNotifications()
    }

    // MARK: Token

    func didRegister(deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        self.token = token
        guard let account = app?.account, app?.isDemo == false else { return }
        // ponytail: every kind on; the system's per-app switch turns them off.
        // Per-kind toggles go in Settings when someone asks for them.
        Task {
            do {
                try await serverSetPushTarget(
                    serverUrl: account.serverUrl, token: account.token, pushToken: token,
                    environment: Self.apnsEnvironment, done: true, input: true, failed: true
                )
            } catch {
                NSLog("push target sync failed: \(error)")
            }
        }
    }

    func didFailToRegister(_ error: Error) {
        NSLog("push registration failed: \(error)")
    }

    /// Which APNs this build's tokens belong to: development-signed builds
    /// (Xcode) say so in their provisioning profile; App Store / TestFlight
    /// builds carry none and are production.
    static let apnsEnvironment: String = {
        #if targetEnvironment(simulator)
        return "sandbox"
        #else
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
              let data = try? Data(contentsOf: url),
              let text = String(data: data, encoding: .isoLatin1)
        else { return "production" }
        return text.range(of: "<key>aps-environment</key>\\s*<string>development</string>", options: .regularExpression) != nil
            ? "sandbox" : "production"
        #endif
    }()

    // MARK: Delivery

    // Completion-handler forms, on the main queue: the async variants
    // complete off the main thread, which UIKit asserts on (crash on tap).

    /// In the app: no banner (the list and the session already show it).
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([])
    }

    /// Tapped: open that session.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let chatId = response.notification.request.content.userInfo["chatId"] as? String
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let chatId { self.open(chatId) }
            }
            completionHandler()
        }
    }

    private func open(_ chatId: String) {
        if openChat?(chatId) != true { pendingChat = chatId }
    }

    /// The shell is up: open a chat tapped before it was.
    func shellReady() {
        guard let chat = pendingChat else { return }
        pendingChat = nil
        _ = openChat?(chat)
    }
}
