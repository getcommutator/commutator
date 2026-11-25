import SwiftUI

// The card's views: drawn by the widget extension, and by the app in DEBUG
// (`-live-activity-preview`) to look at them.

struct LockScreenCard: View {
    let state: AgentActivityAttributes.ContentState
    let stale: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: "terminal.fill")
                Text(state.headline).font(.subheadline.weight(.semibold))
                Spacer()
            }
            ForEach(state.rows) { row in
                AgentRowView(row: row, compact: false)
            }
            if stale { StaleNote() }
        }
        .padding(14)
    }
}

struct AgentRowView: View {
    let row: AgentActivityAttributes.Row
    let compact: Bool

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: AgentPhase.symbol(row.phase))
                .foregroundStyle(AgentPhase.color(row.phase))
                .font(.caption)
            VStack(alignment: .leading, spacing: 0) {
                Text(row.title).font(.caption.weight(.medium)).lineLimit(1)
                if !compact, let project = row.project {
                    Text(project).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 4)
            Text(AgentPhase.label(row.phase))
                .font(.caption2.weight(.semibold))
                .foregroundStyle(AgentPhase.color(row.phase))
        }
    }
}

/// Past the stale date: nothing heard for a long while.
struct StaleNote: View {
    var body: some View {
        Text("Out of date — open Commutator")
            .font(.caption2)
            .foregroundStyle(.secondary)
    }
}
