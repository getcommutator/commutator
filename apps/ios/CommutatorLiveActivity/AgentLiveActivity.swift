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
            ThemedCard(context: context)
        } dynamicIsland: { context in
            let state = context.state
            let accent = Color(hex32: context.attributes.accent)
            let tint = AgentPhase.tint(state.chipPhase, accent: accent)
            return DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    HStack(spacing: 6) {
                        PhaseMark(phase: state.chipPhase, accent: accent)
                        Text(state.chip)
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(tint)
                    }
                    .padding(.leading, 4)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    WorkingGlyph(accent: tint)
                        .frame(width: 16, height: 16)
                        .padding(.trailing, 4)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(state.rows.prefix(3)) { row in
                            AgentRowView(row: row, compact: true, accent: Color(hex32: context.attributes.accent))
                        }
                        let more = max(0, state.rows.count - 3) + state.hidden
                        if more > 0 { MoreNote(count: more) }
                        if context.isStale { StaleNote() }
                    }
                    .padding(.horizontal, 4)
                }
            } compactLeading: {
                WorkingGlyph(accent: tint)
                    .frame(width: 16, height: 16)
            } compactTrailing: {
                // Only working agents: the loader and how many; else the mark of what's up.
                HStack(spacing: 4) {
                    PhaseMark(phase: state.chipPhase, accent: accent)
                    if state.chipPhase == "working" {
                        Text("\(state.active)")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(tint)
                    }
                }
            } minimal: {
                PhaseMark(phase: state.chipPhase, accent: accent)
            }
            .widgetURL(AgentActivityLink.url(state.focus?.chatId))
        }
    }
}


/// The Lock Screen card in the theme's colours for the system's appearance
/// (the widget's colour scheme, not the app's).
struct ThemedCard: View {
    let context: ActivityViewContext<AgentActivityAttributes>
    @Environment(\.colorScheme) private var scheme

    var body: some View {
        let a = context.attributes
        let dark = scheme == .dark
        let accent = Color(hex32: dark ? a.accent : (a.accentLight ?? a.accent))
        let background = (dark ? a.background : a.backgroundLight).map { Color(hex32: $0) }
        LockScreenCard(state: context.state, stale: context.isStale, accent: accent)
            .widgetURL(AgentActivityLink.url(context.state.focus?.chatId))
            .activityBackgroundTint(background)
    }
}
