import SwiftUI

// The card's views: drawn by the widget extension, and by the app in DEBUG
// (`-live-activity-preview`) to look at them.

extension Color {
    /// An ARGB value (0xAARRGGBB) as a colour.
    init(hex32 argb: UInt32) {
        self.init(
            .sRGB,
            red: Double((argb >> 16) & 0xFF) / 255,
            green: Double((argb >> 8) & 0xFF) / 255,
            blue: Double(argb & 0xFF) / 255,
            opacity: Double((argb >> 24) & 0xFF) / 255
        )
    }
}

struct LockScreenCard: View {
    let state: AgentActivityAttributes.ContentState
    let stale: Bool
    var accent: Color = .blue

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                WorkingGlyph(accent: .white)
                    .frame(width: 16, height: 16)
                Text(state.headline).font(.subheadline.weight(.semibold))
                Spacer()
            }
            // Three rows fit the Lock Screen's height; the rest count as "more".
            ForEach(state.rows.prefix(3)) { row in
                AgentRowView(row: row, compact: false, accent: accent)
            }
            let more = max(0, state.rows.count - 3) + state.hidden
            if more > 0 { MoreNote(count: more) }
            if stale { StaleNote() }
        }
        .padding(14)
    }
}

/// One agent: its harness's mark, what it's working on, and its status on
/// the right.
struct AgentRowView: View {
    let row: AgentActivityAttributes.Row
    let compact: Bool
    var accent: Color = .blue

    var body: some View {
        HStack(spacing: 8) {
            HarnessMark(harness: row.harness)
            VStack(alignment: .leading, spacing: 0) {
                Text(row.title).font(.caption.weight(.medium)).lineLimit(1)
                if !compact, let project = row.project {
                    Text(project).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 6)
            PhaseStatus(phase: row.phase, accent: accent)
        }
    }
}

/// The status at the row's right: a spinner while working, then the mark
/// and the word.
struct PhaseStatus: View {
    let phase: String
    var accent: Color = .blue

    var body: some View {
        HStack(spacing: 4) {
            if phase == "working" {
                FrozenSpinner(accent: accent)
                    .frame(width: 12, height: 14)
            } else if AgentPhase.symbol(phase) == "checkmark" {
                CheckGlyph().frame(width: 14, height: 14)
            } else {
                Image(systemName: AgentPhase.symbol(phase))
                    .font(.caption2)
            }
            Text(AgentPhase.label(phase))
                .font(.caption2.weight(.semibold))
        }
        .foregroundStyle(phase == "working" ? accent : AgentPhase.color(phase))
        .lineLimit(1)
        .fixedSize()
    }
}

/// A plain check, no circle: Shepherd's `status-done.svg` (16×16, 1.5 stroke,
/// round ends), as in Mochi's card. A Canvas, like the marks below.
struct CheckGlyph: View {
    var body: some View {
        Canvas { context, size in
            var path = Path()
            path.move(to: CGPoint(x: 13.33, y: 4.33))
            path.addLine(to: CGPoint(x: 6, y: 11.67))
            path.addLine(to: CGPoint(x: 2.67, y: 8.33))
            let s = min(size.width, size.height) / 16
            context.stroke(
                path.applying(CGAffineTransform(scaleX: s, y: s)),
                with: .foreground,
                style: StrokeStyle(lineWidth: 1.5 * s, lineCap: .round, lineJoin: .round)
            )
        }
    }
}

/// The harness's brand mark, drawn with a Canvas: a plain Shape came out as
/// solid squares on the Lock Screen. A terminal glyph for a chat without one.
struct HarnessMark: View {
    let harness: String?

    var body: some View {
        if let harness {
            let mark = BrandMark.forHarness(harness)
            let color: Color = BrandMark.brandTint(for: harness).map { $0 == .black ? .white : Color(uiColor: $0) } ?? .white
            Canvas { context, size in
                let path = BrandMarkShape(mark: mark).path(in: CGRect(origin: .zero, size: size))
                context.fill(path, with: .color(color), style: FillStyle(eoFill: mark.evenOddFill))
            }
            .frame(width: 14, height: 14)
        } else {
            Image(systemName: "terminal.fill")
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(width: 14, height: 14)
        }
    }
}

/// Chats past the rows the card has room for.
struct MoreNote: View {
    let count: Int

    var body: some View {
        Text("+\(count) more")
            .font(.caption2)
            .foregroundStyle(.secondary)
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

/// The app's working loader (the sidebar's 2 × 3 glyph spinner) held on one
/// frame: six accent cells whose opacities follow the spinner's ring, so it
/// doesn't move on the Lock Screen. The dim floor is raised so no cell vanishes.
struct FrozenSpinner: View {
    let accent: Color

    /// Ring position of each cell, row-major (the app's `StatusGlyph.ring`).
    private static let ring = [[0, 1], [5, 2], [4, 3]]

    /// `StatusGlyph.opacity` with a 0.3 floor.
    private static func opacity(_ t: Double) -> Double {
        if t < 0.45 { return 1 + (0.3 - 1) * t / 0.45 }
        return 0.3
    }

    var body: some View {
        Canvas { context, size in
            // The app's geometry: 3.5 pt cells, 1.5 pt gaps, 2 × 3 cells, scaled to fit.
            let k = min(size.width / 8.5, size.height / 13.5)
            let d = 3.5 * k, gap = 1.5 * k
            let ox = (size.width - (d * 2 + gap)) / 2
            let oy = (size.height - (d * 3 + gap * 2)) / 2
            for row in 0..<3 {
                for col in 0..<2 {
                    let t = Double(Self.ring[row][col]) / 6
                    let rect = CGRect(x: ox + CGFloat(col) * (d + gap), y: oy + CGFloat(row) * (d + gap), width: d, height: d)
                    context.opacity = Self.opacity(t)
                    context.fill(Path(ellipseIn: rect), with: .color(accent))
                }
            }
        }
    }
}

/// A phase's mark: the loader while working (theme accent), else its symbol
/// in the phase's colour. For the Dynamic Island.
struct PhaseMark: View {
    let phase: String
    let accent: Color

    var body: some View {
        if phase == "working" {
            FrozenSpinner(accent: accent).frame(width: 12, height: 14)
        } else {
            Image(systemName: AgentPhase.symbol(phase))
                .font(.caption.weight(.bold))
                .foregroundStyle(AgentPhase.color(phase))
        }
    }
}

extension AgentPhase {
    /// The colour a phase paints with: working takes the theme accent.
    static func tint(_ phase: String, accent: Color) -> Color {
        phase == "working" ? accent : color(phase)
    }
}
