import GhostMarks
import SwiftUI

/// The message box: a glass capsule on the keyboard. It grows to six lines; while a turn runs it
/// queues the message, and Stop sits beside it.
struct Composer: View {
    @Environment(\.style) private var style
    @Binding var text: String
    @Binding var photos: [PickedPhoto]
    /// The photo library and the camera, presented by the screen.
    @Binding var picking: Bool
    @Binding var shooting: Bool
    let running: Bool
    let waiting: Bool
    let enabled: Bool
    /// The computer takes photos with a message.
    var offersPhotos = false
    /// "Opus 5.5 · High", when the computer lets the person choose.
    var chip: String?
    let onSend: () -> Void
    let onStop: () -> Void
    var onChip: () -> Void = {}
    @FocusState private var focused: Bool
    @Namespace private var glass

    private var empty: Bool { text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && photos.isEmpty }

    var body: some View {
        GlassEffectContainer(spacing: 10) {
            VStack(alignment: .leading, spacing: 8) {
                if let chip {
                    Button(action: onChip) {
                        HStack(spacing: 6) {
                            Image(systemName: "slider.horizontal.3")
                            Text(chip).lineLimit(1)
                        }
                        .ghostFont(.ui(12, weight: .medium))
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 6)
                        .glassEffect(.regular.interactive(), in: .capsule)
                        // The glass stays small; the finger gets the full 44 points.
                        .frame(minHeight: 44)
                        .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .padding(.vertical, -8)
                    .accessibilityLabel("Model and agent: \(chip)")
                }
                VStack(alignment: .leading, spacing: 0) {
                    if !photos.isEmpty {
                        PhotoStrip(photos: $photos)
                    }
                    HStack(alignment: .bottom, spacing: 8) {
                        if offersPhotos {
                            PhotoButton(picking: $picking, shooting: $shooting) { photos.append($0) }
                                .disabled(photos.count >= photoLimit)
                                .padding(.leading, 4)
                                .padding(.bottom, 1)
                        }
                        TextField(running ? "Add to the queue" : "Message the agent", text: $text, axis: .vertical)
                            .ghostFont(.composer)
                            .foregroundStyle(style.fg1)
                            .lineLimit(1...6)
                            .focused($focused)
                            .submitLabel(.return)
                            .padding(.leading, offersPhotos ? 0 : 16)
                            .padding(.vertical, 11)
                            .accessibilityLabel("Message")
                        if running {
                            Button(action: onStop) {
                                Image(systemName: "stop.fill")
                                    .font(.subheadline.weight(.bold))
                                    .frame(width: 36, height: 36)
                            }
                            .buttonStyle(.glassProminent)
                            .buttonBorderShape(.circle)
                            .tint(style.red)
                            .glassEffectID("stop", in: glass)
                            .accessibilityLabel("Stop")
                            .padding(.bottom, 5)
                            .padding(.trailing, empty ? 5 : 0)
                        }
                        if !running || !empty {
                            Button(action: onSend) {
                                Image(systemName: "arrow.up")
                                    .font(.body.weight(.bold))
                                    .foregroundStyle(style.onPrimary)
                                    .frame(width: 36, height: 36)
                            }
                            .buttonStyle(.glassProminent)
                            .buttonBorderShape(.circle)
                            .tint(style.primary)
                            .disabled(empty || !enabled)
                            .glassEffectID("send", in: glass)
                            .accessibilityLabel(running ? "Queue" : "Send")
                            .padding(.trailing, 5)
                            .padding(.bottom, 5)
                        }
                    }
                }
                .glassEffect(
                    .regular.tint(style.accent.opacity(waiting ? 0.22 : 0.08)).interactive(),
                    in: .rect(cornerRadius: 24))
            }
        }
        .animation(.snappy(duration: 0.25), value: running)
        .animation(.snappy(duration: 0.25), value: empty)
        .animation(.snappy(duration: 0.25), value: photos.count)
    }
}

/// While a turn runs and nothing waits on the person: a breathing dot, a verb that changes, and
/// how long it has been.
struct WorkingLine: View {
    @Environment(\.style) private var style
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let since: Date
    private static let verbs = ["Working", "Thinking", "Reading", "Writing", "Checking"]

    var body: some View {
        TimelineView(.periodic(from: since, by: 1)) { context in
            let seconds = Int(context.date.timeIntervalSince(since))
            HStack(spacing: 8) {
                Circle()
                    .fill(style.fg2)
                    .frame(width: 7, height: 7)
                    // Only the dot's light breathes; nothing around it moves with it.
                    .animation(reduceMotion ? nil : .easeInOut(duration: 0.9)) { dot in
                        dot.opacity(reduceMotion ? 1 : (seconds % 2 == 0 ? 1 : 0.35))
                    }
                Text(Self.verbs[(seconds / 4) % Self.verbs.count])
                    .contentTransition(.numericText())
                if seconds >= 1 {
                    Text(elapsedWords(Int64(seconds) * 1000))
                        .monospacedDigit()
                        .contentTransition(.numericText())
                        .foregroundStyle(style.fg3)
                }
            }
            .ghostFont(.meta)
            .foregroundStyle(style.fg2)
        }
        .accessibilityElement(children: .combine)
    }
}
