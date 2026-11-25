import O7Marks
import O7Transcript
import O7Wire
import SwiftUI

/// One item of a turn, drawn as its kind is drawn on the desktop.
struct ItemView: View {
    @Environment(\.style) private var style
    let item: RenderedItem
    let chat: ChatStore

    var body: some View {
        switch item.content {
        case .user(let user):
            UserBubble(text: user.text, photos: chat.sentPhotos[user.key] ?? [], steering: user.steering?.state)
        case .message(let message):
            MessageView(message: message)
        case .thought(let thought, let thinking):
            ThoughtRow(thought: thought, thinking: thinking)
        case .tool:
            StepsGroup(tools: [item])
        case .permission(let card):
            PermissionCard(card: card, chat: chat)
        case .question(let card):
            QuestionCard(card: card, chat: chat)
        case .notice(let notice):
            Text(notice.message)
                .o7Font(.meta)
                .foregroundStyle(notice.level == .error ? style.red : notice.level == .warning ? style.amber : style.fg3)
                .frame(maxWidth: .infinity, alignment: .leading)
        case .error(let error):
            ErrorCard(error: error)
        case .gap:
            Text("Part of this turn is still being saved on the computer.")
                .o7Font(.meta)
                .foregroundStyle(style.fg3)
        }
    }
}

/// The person's own words: the theme's paper, on the right.
struct UserBubble: View {
    @Environment(\.style) private var style
    let text: String
    /// The photos it carried, when this phone sent them.
    var photos: [PickedPhoto] = []
    var steering: SteeringState?
    var pending: String?

    var body: some View {
        VStack(alignment: .trailing, spacing: 4) {
            if !photos.isEmpty {
                HStack(spacing: 6) {
                    ForEach(photos) { photo in
                        Image(uiImage: photo.image)
                            .resizable()
                            .scaledToFill()
                            .frame(width: photos.count == 1 ? 180 : 88, height: photos.count == 1 ? 180 : 88)
                            .clipShape(.rect(cornerRadius: 14))
                            .accessibilityLabel("Photo")
                    }
                }
                .opacity(pending == nil ? 1 : 0.6)
            }
            if !text.isEmpty {
                Text(text)
                    .o7Font(.prose)
                    .foregroundStyle(style.onPaper)
                    .textSelection(.enabled)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(style.paper, in: .rect(cornerRadius: 18))
                    .opacity(pending == nil ? 1 : 0.6)
            } else if photos.isEmpty {
                // Images sent from the computer: the history carries words only.
                Label("Images", systemImage: "photo")
                    .o7Font(.meta)
                    .foregroundStyle(style.onPaper)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(style.paper, in: .rect(cornerRadius: 18))
            }
            if let pending {
                Text(pending).o7Font(.small).foregroundStyle(style.fg3)
            } else if let steering, steering != .accepted {
                Text(steeringWords(steering)).o7Font(.small).foregroundStyle(steering == .rejected ? style.red : style.fg3)
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, 44)
    }

    private func steeringWords(_ state: SteeringState) -> String {
        switch state {
        case .pending: "Steering the turn"
        case .accepted: ""
        case .rejected: "The agent did not take this mid-turn"
        case .unconfirmed: "Sent while the turn ran"
        }
    }
}

/// The agent's answer, as markdown; parsed once per change.
private struct MessageView: View {
    let message: MessageItem
    var body: some View {
        MarkdownView(message.text)
            .equatable(message)
    }
}

extension View {
    /// Redraws only when `value` changes.
    fileprivate func equatable<V: Equatable>(_ value: V) -> some View {
        EquatableBox(value: value, content: self).equatable()
    }
}

private struct EquatableBox<V: Equatable, Content: View>: View, Equatable {
    let value: V
    let content: Content
    static func == (lhs: Self, rhs: Self) -> Bool { lhs.value == rhs.value }
    var body: some View { content }
}

/// A thought: "Thinking" while it runs, "Thought for 8s" after; opens to its words, dimmed.
struct ThoughtRow: View {
    @Environment(\.style) private var style
    let thought: ThoughtItem
    let thinking: Bool
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                withAnimation(.snappy) { open.toggle() }
            } label: {
                HStack(spacing: 8) {
                    if thinking {
                        PixelLoader(size: 12, tempo: .thinking)
                    } else {
                        Image(systemName: "brain").font(.footnote).foregroundStyle(style.fg3)
                    }
                    Text(title).o7Font(.meta).foregroundStyle(style.fg3)
                    Image(systemName: open ? "chevron.up" : "chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(style.fg4)
                }
            }
            .buttonStyle(.plain)
            if open {
                Text(thought.text)
                    .o7Font(.chat)
                    .foregroundStyle(style.fg3)
                    .textSelection(.enabled)
                    .transition(.opacity)
            }
        }
    }

    private var title: String {
        if thinking { return "Thinking" }
        guard let duration = thought.durationMs, duration >= 1000 else { return "Thought" }
        return "Thought for \(formatElapsed(Double(duration)))"
    }
}

/// A stretch of tool calls under one header: "Read 3 files and ran 1 command", with a loader while
/// one runs and failures said at its end. It opens to one row per call.
struct StepsGroup: View {
    @Environment(\.style) private var style
    @ScaledMetric(relativeTo: .footnote) private var mark: CGFloat = 14
    let tools: [RenderedItem]
    @State private var open = false
    @State private var shown: ToolItem?

    private var grouped: [GroupedTool] {
        tools.compactMap { item in
            guard case .tool(let tool, let display) = item.content else { return nil }
            return GroupedTool(toolKind: tool.toolKind, display: display)
        }
    }

    var body: some View {
        let result = groupResult(grouped)
        let running = grouped.contains { $0.display == .pending || $0.display == .inProgress }
        VStack(alignment: .leading, spacing: 4) {
            Button {
                if tools.count == 1, case .tool(let tool, _) = tools[0].content {
                    shown = tool
                } else {
                    withAnimation(.snappy) { open.toggle() }
                }
            } label: {
                HStack(spacing: 8) {
                    if running {
                        PixelLoader(size: 12, tempo: .tool)
                    } else {
                        Image(systemName: icon(tools.count == 1 ? grouped.first?.toolKind : nil))
                            .font(.footnote)
                            .foregroundStyle(style.fg3)
                            .frame(width: mark)
                    }
                    Text(header)
                        .o7Font(.meta)
                        .foregroundStyle(style.fg2)
                        .lineLimit(1)
                    if result.tone != .quiet {
                        Text(result.text)
                            .o7Font(.meta)
                            .foregroundStyle(result.tone == .failed ? style.red : style.fg3)
                    }
                    Spacer(minLength: 0)
                    if tools.count > 1 {
                        Image(systemName: open ? "chevron.up" : "chevron.down")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(style.fg4)
                    }
                }
                .padding(.vertical, 4)
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if open {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(tools) { item in
                        if case .tool(let tool, let display) = item.content {
                            ToolRow(tool: tool, display: display) { shown = tool }
                        }
                    }
                }
                .padding(.leading, 6)
                .transition(.opacity)
            }
        }
        .sheet(item: $shown) { tool in
            ToolSheet(tool: tool)
                .presentationDetents([.medium, .large])
        }
    }

    private var header: String {
        if tools.count == 1, case .tool(let tool, let display) = tools[0].content {
            let phrase = toolPhrase(tool.title, kind: tool.toolKind, display: display)
            return [phrase.verb, phrase.object].compactMap { $0 }.joined(separator: " ")
        }
        return toolsSummary(grouped)
    }
}

/// One call: its kind's symbol, what it did, and how it ended.
struct ToolRow: View {
    @Environment(\.style) private var style
    @ScaledMetric(relativeTo: .footnote) private var mark: CGFloat = 16
    let tool: ToolItem
    let display: ToolDisplayStatus
    let open: () -> Void

    var body: some View {
        let phrase = toolPhrase(tool.title, kind: tool.toolKind, display: display)
        Button(action: open) {
            HStack(spacing: 8) {
                Image(systemName: icon(tool.toolKind))
                    .font(.footnote)
                    .foregroundStyle(style.fg3)
                    .frame(width: mark)
                if let verb = phrase.verb {
                    Text(verb).o7Font(.meta).foregroundStyle(style.fg2)
                }
                Text(phrase.object)
                    .o7Font(phrase.literal ? .mono(12) : .meta)
                    .foregroundStyle(style.fg1)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                if let delta = diffDelta(tool) {
                    Text("+\(delta.added)").o7Font(.small).foregroundStyle(style.green)
                    Text("−\(delta.removed)").o7Font(.small).foregroundStyle(style.red)
                }
                statusMark
            }
            .padding(.vertical, 6)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder private var statusMark: some View {
        switch display {
        case .pending, .inProgress: PixelLoader(size: 10, tempo: .tool)
        case .failed: Image(systemName: "xmark").font(.caption2.weight(.bold)).foregroundStyle(style.red)
        case .stopped: Image(systemName: "stop.fill").font(.caption2).foregroundStyle(style.fg4)
        case .completed: EmptyView()
        }
    }
}

/// The symbol for a tool's kind.
func icon(_ kind: ToolKind?) -> String {
    switch kind {
    case .read: "doc.text"
    case .edit: "pencil"
    case .delete: "trash"
    case .move: "arrow.right.doc.on.clipboard"
    case .search: "magnifyingglass"
    case .execute: "terminal"
    case .think: "brain"
    case .fetch: "globe"
    case .switchMode: "arrow.triangle.swap"
    case .other, .none: "puzzlepiece.extension"
    }
}

extension ToolItem: @retroactive Identifiable {}

/// An error the agent or o7 reported, in red, with its detail behind a tap.
struct ErrorCard: View {
    @Environment(\.style) private var style
    let error: ErrorItem
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(error.message, systemImage: "exclamationmark.triangle.fill")
                .o7Font(.chat)
                .foregroundStyle(style.red)
            if let detail = error.detail, !detail.isEmpty {
                Button(open ? "Hide the detail" : "Show the detail") { withAnimation(.snappy) { open.toggle() } }
                    .o7Font(.small)
                    .foregroundStyle(style.fg3)
                    .buttonStyle(.plain)
                if open {
                    Text(detail).o7Font(.mono(12)).foregroundStyle(style.fg2).textSelection(.enabled)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(style.redTint, in: .rect(cornerRadius: 12))
    }
}
