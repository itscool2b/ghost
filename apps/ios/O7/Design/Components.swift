import O7Marks
import O7Wire
import SwiftUI

/// How a chat's state reads, in the same words as the browser remote (`remote.tsx` 330-365).
struct StatusWords {
    enum Tone { case plain, attention, failure }
    let words: String
    let tone: Tone
    let mark: AgentState

    init(_ chat: ChatSummary) {
        if chat.needsAttention {
            (words, tone, mark) = ("Needs you", .attention, .waiting)
            return
        }
        switch chat.status {
        case .running: (words, tone, mark) = ("Working", .plain, .tool)
        case .starting: (words, tone, mark) = ("Starting", .plain, .thinking)
        case .cancelling, .stopping: (words, tone, mark) = ("Stopping", .plain, .tool)
        case .ready: (words, tone, mark) = ("Idle", .plain, .idle)
        case .stopped: (words, tone, mark) = ("Stopped", .plain, .idle)
        case .failed: (words, tone, mark) = ("Failed", .failure, .error)
        case .authRequired: (words, tone, mark) = ("Needs sign-in on the computer", .failure, .error)
        }
    }

    func color(_ style: O7Style) -> Color {
        switch tone {
        case .plain: style.fg3
        case .attention: style.accent
        case .failure: style.red
        }
    }
}

/// A line that may hold `code` in backticks, as agents and o7 write step lines: the code in Geist
/// Mono, the rest as it is.
func inline(_ text: String, code: Color) -> AttributedString {
    let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    guard var parsed = try? AttributedString(markdown: text, options: options) else { return AttributedString(text) }
    for run in parsed.runs where run.inlinePresentationIntent?.contains(.code) == true {
        parsed[run.range].font = O7Font.mono(12)
        parsed[run.range].foregroundColor = code
    }
    for run in parsed.runs where run.link != nil {
        parsed[run.range].link = nil
    }
    return parsed
}

/// "now", "4m", "3h", "2d", or a date a month back.
struct Ago: View {
    let millis: Int64
    var body: some View {
        TimelineView(.periodic(from: .now, by: 30)) { context in
            Text(Self.words(millis, now: context.date)).monospacedDigit()
        }
    }

    static func words(_ millis: Int64, now: Date = .now) -> String {
        let seconds = max(0, now.timeIntervalSince1970 - Double(millis) / 1000)
        switch seconds {
        case ..<60: return "now"
        case ..<3600: return "\(Int(seconds / 60))m"
        case ..<86_400: return "\(Int(seconds / 3600))h"
        case ..<(30 * 86_400): return "\(Int(seconds / 86_400))d"
        default:
            return Date(timeIntervalSince1970: Double(millis) / 1000)
                .formatted(.dateTime.month(.abbreviated).day())
        }
    }
}

/// "12s", "4m 2s", "1h 3m".
func elapsedWords(_ milliseconds: Int64) -> String {
    let seconds = max(0, Int(milliseconds / 1000))
    if seconds < 60 { return "\(seconds)s" }
    if seconds < 3600 { return "\(seconds / 60)m \(seconds % 60)s" }
    return "\(seconds / 3600)h \(seconds % 3600 / 60)m"
}

/// A chat in a list: its agent's mark, its title, who and how, when.
struct ChatRow: View {
    @Environment(\.style) private var style
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let chat: WorkspaceChat
    var project: String?
    /// A live line under it: the step in progress, or how the last turn ended.
    var line: String?

    var body: some View {
        let status = StatusWords(chat.chat)
        HStack(alignment: .top, spacing: 12) {
            AgentMark(agent: chat.chat.agentId, state: status.mark, size: 30)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline) {
                    Text(chat.chat.title ?? "Untitled chat")
                        .o7Font(.rowTitle)
                        .foregroundStyle(style.fg1)
                        .lineLimit(2)
                    Spacer(minLength: 8)
                    Ago(millis: chat.chat.updatedAt)
                        .o7Font(.time)
                        .foregroundStyle(style.fg3)
                }
                (Text(chat.chat.agentName) + Text(" · ")
                    + Text(status.words).foregroundStyle(status.color(style))
                    + Text(project.map { " · \($0)" } ?? ""))
                    .o7Font(.meta)
                    .foregroundStyle(style.fg3)
                if let line, !line.isEmpty {
                    Text(inline(line, code: style.fg1))
                        .o7Font(.meta)
                        .foregroundStyle(style.fg2)
                        .lineLimit(dynamicTypeSize.isAccessibilitySize ? 4 : 2)
                }
            }
        }
        .padding(.vertical, 10)
        .contentShape(.rect)
        .accessibilityElement(children: .combine)
    }
}

/// A plain heading over a group of rows, as o7 writes them: no small caps, no labels above it.
struct GroupTitle: View {
    @Environment(\.style) private var style
    let title: String
    var count: Int?
    var body: some View {
        HStack(spacing: 6) {
            Text(title).o7Font(.ui(13, weight: .medium, relativeTo: .subheadline))
            if let count { Text("\(count)").monospacedDigit() }
        }
        .o7Font(.ui(13, relativeTo: .subheadline))
        .foregroundStyle(style.fg3)
        .padding(.top, 14)
        .padding(.bottom, 2)
    }
}

/// A flat card on a solid layer: 16 points round, faint ink, no rim, no shadow.
struct CardBackground: ViewModifier {
    @Environment(\.style) private var style
    var raised = false
    func body(content: Content) -> some View {
        content
            .padding(14)
            .background(raised ? style.raised : style.card, in: .rect(cornerRadius: 16))
    }
}

extension View {
    func card(raised: Bool = false) -> some View { modifier(CardBackground(raised: raised)) }
}

/// The theme's primary action, in glass tinted with the theme's primary colour.
struct ProminentButton: View {
    @Environment(\.style) private var style
    let title: String
    var systemImage: String?
    var busy = false
    var role: ButtonRole?
    let action: () -> Void

    var body: some View {
        Button(role: role, action: action) {
            HStack(spacing: 8) {
                if busy { ProgressView().tint(style.onPrimary) }
                if let systemImage, !busy { Image(systemName: systemImage) }
                Text(title)
            }
            .o7Font(.ui(16, weight: .medium, relativeTo: .body))
            .foregroundStyle(style.onPrimary)
            .frame(maxWidth: .infinity, minHeight: 30)
        }
        .buttonStyle(.glassProminent)
        .tint(style.primary)
        .controlSize(.large)
        .disabled(busy)
    }
}

/// A quiet action beside a prominent one.
struct QuietButton: View {
    @Environment(\.style) private var style
    let title: String
    var systemImage: String?
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                if let systemImage { Image(systemName: systemImage) }
                Text(title)
            }
            .o7Font(.ui(16, weight: .medium, relativeTo: .body))
            .foregroundStyle(style.fg1)
            .frame(maxWidth: .infinity, minHeight: 30)
        }
        .buttonStyle(.glass)
        .controlSize(.large)
    }
}

/// The computer's link, in a few words and a dot.
struct ConnectionLine: View {
    @Environment(\.style) private var style
    let phase: O7Client.ConnectionPhase

    var body: some View {
        HStack(spacing: 6) {
            switch phase {
            case .ready:
                Circle().fill(style.green).frame(width: 7, height: 7)
                Text("Connected")
            case .away:
                Circle().fill(style.amber).frame(width: 7, height: 7)
                Text("Computer is away")
            case .connecting:
                PixelLoader(size: 12, tempo: .thinking)
                Text("Connecting")
            case .ended:
                Circle().fill(style.red).frame(width: 7, height: 7)
                Text("Not paired")
            }
        }
        .o7Font(.meta)
        .foregroundStyle(style.fg3)
        .accessibilityElement(children: .combine)
    }
}

import O7Client
