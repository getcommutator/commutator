import ActivityKit
import SwiftUI

/// What the account's agents are doing, as one Live Activity (the Lock
/// Screen and the Dynamic Island). The app starts it when work is sent from
/// this phone; commutator-server keeps it current with `liveactivity` pushes
/// whose `content-state` is [`ContentState`] (`push/activity.rs`, `State`).
/// Shared by the app and the widget extension, which draws it.
struct AgentActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        /// Chats working or waiting on you.
        var active: Int
        /// Chats waiting on you.
        var attention: Int
        /// The first few, those that need you first.
        var rows: [Row]
    }

    struct Row: Codable, Hashable, Identifiable {
        var chatId: String
        var title: String
        var project: String?
        /// `working`, `input`, `failed` or `done`.
        var phase: String

        var id: String { chatId }
    }
}

extension AgentActivityAttributes.ContentState {
    /// "2 agents working · 1 needs you", "All done".
    var headline: String {
        if active == 0 { return rows.contains { $0.phase == "failed" } ? "Finished, with a failure" : "All done" }
        let working = "\(active) agent\(active == 1 ? "" : "s") active"
        return attention > 0 ? "\(working) · \(attention) need\(attention == 1 ? "s" : "") you" : working
    }

    /// The chat a tap opens: the first that needs you, else the first.
    var focus: AgentActivityAttributes.Row? { rows.first }

    /// The one word for the compact Dynamic Island.
    var chip: String {
        if attention > 0 { return "Input" }
        if rows.contains(where: { $0.phase == "failed" }) { return "Failed" }
        return active > 0 ? "\(active)" : "Done"
    }

    var chipPhase: String {
        if attention > 0 { return "input" }
        if rows.contains(where: { $0.phase == "failed" }) { return "failed" }
        return active > 0 ? "working" : "done"
    }
}

/// How each phase reads.
enum AgentPhase {
    static func label(_ phase: String) -> String {
        switch phase {
        case "working": "Working"
        case "input": "Needs you"
        case "failed": "Failed"
        default: "Done"
        }
    }

    static func symbol(_ phase: String) -> String {
        switch phase {
        case "working": "circle.dotted.circle"
        case "input": "questionmark.bubble.fill"
        case "failed": "exclamationmark.triangle.fill"
        default: "checkmark.circle.fill"
        }
    }

    static func color(_ phase: String) -> Color {
        switch phase {
        case "working": .blue
        case "input": .orange
        case "failed": .red
        default: .green
        }
    }
}

/// Where a tap on the card goes: `commutator://chat/<id>`.
enum AgentActivityLink {
    static func url(_ chatId: String?) -> URL? {
        guard let chatId, let id = chatId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) else {
            return URL(string: "commutator://chats")
        }
        return URL(string: "commutator://chat/\(id)")
    }
}
