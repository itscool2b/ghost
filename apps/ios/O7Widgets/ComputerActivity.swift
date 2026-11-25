import ActivityKit
import O7Marks
import O7Push
import O7Theme
import SwiftUI
import WidgetKit

/// The Lock Screen and the Dynamic Island while work runs on a computer: counts, the leading
/// chat's line, and a timer. Its updates arrive sealed; one that cannot be opened shows o7 only.
struct ComputerActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: O7ActivityAttributes.self) { context in
            LockScreenActivity(state: context.state, name: name(context.attributes))
                .activityBackgroundTint(Color.black.opacity(0.6))
                .activitySystemActionForegroundColor(.white)
                .widgetURL(URL(string: "o7://inbox"))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Mark(state: context.state).padding(.leading, 4)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    if let since = context.state.since {
                        Text(timerInterval: since...Date.distantFuture, countsDown: false)
                            .monospacedDigit()
                            .font(.caption)
                            .multilineTextAlignment(.trailing)
                            .frame(minWidth: 44, maxWidth: 90, alignment: .trailing)
                    }
                }
                DynamicIslandExpandedRegion(.center) {
                    Text(summary(context.state, name: name(context.attributes)))
                        .font(.subheadline.weight(.semibold))
                        .lineLimit(1)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    if context.state.opened, let lead = context.state.lead {
                        Text(lead).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                    }
                }
            } compactLeading: {
                Mark(state: context.state)
            } compactTrailing: {
                Text(compact(context.state)).monospacedDigit().font(.caption.weight(.semibold))
            } minimal: {
                Mark(state: context.state)
            }
            .widgetURL(URL(string: "o7://inbox"))
        }
    }

    private func name(_ attributes: O7ActivityAttributes) -> String {
        PushOpener().hostName(device: attributes.device) ?? "o7"
    }
}

private func summary(_ state: O7ActivityAttributes.ContentState, name: String) -> String {
    guard state.opened else { return "o7" }
    var parts: [String] = []
    if state.asking > 0 { parts.append("\(state.asking) need\(state.asking == 1 ? "s" : "") you") }
    if state.working > 0 { parts.append("\(state.working) working") }
    return "\(name): " + (parts.isEmpty ? "all done" : parts.joined(separator: " · "))
}

private func compact(_ state: O7ActivityAttributes.ContentState) -> String {
    guard state.opened else { return "" }
    return state.asking > 0 ? "\(state.asking)" : "\(state.working)"
}

/// The pixel loader's still frame while work runs, a dot when something waits: Live Activities
/// do not animate.
private struct Mark: View {
    let state: O7ActivityAttributes.ContentState

    var body: some View {
        if state.asking > 0 {
            Circle().fill(Color.orange).frame(width: 10, height: 10)
        } else {
            PixelLoader(size: 14, animated: false)
                .environment(\.o7Palette, Palettes.palette(.claude, dark: true))
        }
    }
}

private struct LockScreenActivity: View {
    let state: O7ActivityAttributes.ContentState
    let name: String

    var body: some View {
        HStack(spacing: 12) {
            Mark(state: state)
            VStack(alignment: .leading, spacing: 2) {
                Text(summary(state, name: name)).font(.subheadline.weight(.semibold))
                if state.opened, let lead = state.lead {
                    Text(lead).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if let since = state.since {
                Text(timerInterval: since...Date.distantFuture, countsDown: false)
                    .monospacedDigit()
                    .font(.caption)
                    .multilineTextAlignment(.trailing)
                    .frame(minWidth: 44, maxWidth: 90, alignment: .trailing)
            }
        }
        .padding(14)
    }
}
