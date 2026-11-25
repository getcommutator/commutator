import ActivityKit
import SwiftUI
import WidgetKit

@main
struct CommutatorLiveActivityBundle: WidgetBundle {
    var body: some Widget {
        AgentLiveActivity()
    }
}

/// The agents' card: a headline and up to five chats on the Lock Screen; the
/// count or what needs you in the Dynamic Island.
struct AgentLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: AgentActivityAttributes.self) { context in
            LockScreenCard(state: context.state, stale: context.isStale)
                .widgetURL(AgentActivityLink.url(context.state.focus?.chatId))
                .activityBackgroundTint(nil)
        } dynamicIsland: { context in
            let state = context.state
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Label(state.chip, systemImage: AgentPhase.symbol(state.chipPhase))
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(AgentPhase.color(state.chipPhase))
                        .padding(.leading, 4)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text("Commutator")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .padding(.trailing, 4)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(state.rows.prefix(3)) { row in
                            AgentRowView(row: row, compact: true)
                        }
                        if context.isStale { StaleNote() }
                    }
                    .padding(.horizontal, 4)
                }
            } compactLeading: {
                Image(systemName: "terminal.fill")
                    .foregroundStyle(.secondary)
            } compactTrailing: {
                Text(state.chip)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(AgentPhase.color(state.chipPhase))
            } minimal: {
                Image(systemName: AgentPhase.symbol(state.chipPhase))
                    .foregroundStyle(AgentPhase.color(state.chipPhase))
            }
            .widgetURL(AgentActivityLink.url(state.focus?.chatId))
        }
    }
}

