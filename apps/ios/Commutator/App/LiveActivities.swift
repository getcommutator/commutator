import ActivityKit
import SwiftUI
import UIKit

/// The agents' Live Activity, as in T3 Code: started here when work is sent
/// from this phone (and none is up), then kept current by the server, which
/// every machine tells about its chats' phases. This hands the server the
/// card's push token and says when the card went away.
@MainActor
final class LiveActivities {
    static let shared = LiveActivities()

    private weak var app: AppModel?
    private var watching = Set<String>()
    private static let enabledKey = "liveActivity.enabled"

    /// The Settings switch (on unless turned off there; the system has its own).
    var enabled: Bool {
        get { UserDefaults.standard.object(forKey: Self.enabledKey) as? Bool ?? true }
        set {
            UserDefaults.standard.set(newValue, forKey: Self.enabledKey)
            if !newValue { endAll() }
        }
    }

    /// iOS lets this app show them (Settings › Commutator › Live Activities).
    var allowed: Bool { ActivityAuthorizationInfo().areActivitiesEnabled }

    /// At launch: keep the cards still up reporting their tokens.
    func install(app: AppModel) {
        self.app = app
        Activity<AgentActivityAttributes>.activities.forEach(watch)
    }

    /// The person just sent work: bring the card up if it isn't.
    func armForLocalWork() {
        guard enabled, allowed, app?.isDemo == false, app?.account != nil else { return }
        let live = Activity<AgentActivityAttributes>.activities.filter { $0.activityState == .active }
        guard live.isEmpty else { return }
        // A placeholder until the server's first push (at once: registering
        // the token sends what every machine is doing).
        let state = AgentActivityAttributes.ContentState(active: 1, attention: 0, rows: [])
        do {
            let activity = try Activity.request(
                attributes: AgentActivityAttributes(),
                content: .init(state: state, staleDate: Date().addingTimeInterval(2 * 60 * 60)),
                pushType: .token
            )
            watch(activity)
        } catch {
            NSLog("live activity not started: \(error)")
        }
    }

    private func watch(_ activity: Activity<AgentActivityAttributes>) {
        guard watching.insert(activity.id).inserted else { return }
        Task { [weak self] in
            for await token in activity.pushTokenUpdates {
                let hex = token.map { String(format: "%02x", $0) }.joined()
                await self?.register(hex)
            }
        }
        Task { [weak self] in
            for await state in activity.activityStateUpdates where state == .ended || state == .dismissed {
                await self?.unregister()
                break
            }
        }
    }

    private func register(_ token: String) async {
        guard let account = app?.account else { return }
        do {
            try await serverSetActivityTarget(
                serverUrl: account.serverUrl, token: account.token, platform: "apns",
                pushToken: token, environment: PushNotifications.apnsEnvironment
            )
        } catch {
            NSLog("live activity target sync failed: \(error)")
        }
    }

    private func unregister() async {
        guard let account = app?.account else { return }
        try? await serverRemoveActivityTarget(serverUrl: account.serverUrl, token: account.token)
    }

    #if DEBUG
    private static let sample = AgentActivityAttributes.ContentState(active: 2, attention: 1, rows: [
        .init(chatId: "a", title: "Wire session notifications to the server", project: "commutator", phase: "input"),
        .init(chatId: "b", title: "Fix the flaky relay test", project: "commutator", phase: "working"),
        .init(chatId: "c", title: "Bump the Android NDK", project: "commutator", phase: "done"),
    ])

    /// `-live-activity-preview`: the Lock Screen card over a dark backdrop, in the app.
    static func preview() -> UIViewController {
        let card = VStack(spacing: 24) {
            LockScreenCard(state: sample, stale: false)
            LockScreenCard(state: .init(active: 0, attention: 0, rows: [sample.rows[2]]), stale: true)
        }
        .background(RoundedRectangle(cornerRadius: 24).fill(.ultraThinMaterial))
        .padding(16)
        .frame(maxHeight: .infinity)
        .background(Color.black)
        .environment(\.colorScheme, .dark)
        return UIHostingController(rootView: card)
    }

    /// `-live-activity-demo`: a card with sample chats, no server (screenshots).
    func demo() {
        let state = AgentActivityAttributes.ContentState(active: 2, attention: 1, rows: [
            .init(chatId: "a", title: "Wire session notifications to the server", project: "commutator", phase: "input"),
            .init(chatId: "b", title: "Fix the flaky relay test", project: "commutator", phase: "working"),
            .init(chatId: "c", title: "Bump the Android NDK", project: "commutator", phase: "done"),
        ])
        do {
            let activity = try Activity.request(attributes: AgentActivityAttributes(), content: .init(state: state, staleDate: nil))
            NSLog("live activity demo: \(activity.id) \(activity.activityState)")
        } catch {
            NSLog("live activity demo failed: \(error) enabled=\(allowed)")
        }
    }
    #endif

    /// Turned off, or signing out: every card goes now.
    func endAll() {
        for activity in Activity<AgentActivityAttributes>.activities {
            Task { await activity.end(nil, dismissalPolicy: .immediate) }
        }
    }
}
