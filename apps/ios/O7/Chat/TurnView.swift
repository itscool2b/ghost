import O7Marks
import O7Transcript
import O7Wire
import SwiftUI

/// One turn: the person's message, the agent's work and its answer. A finished turn's work folds
/// under one "Worked for" row so the answer stands alone, as on the desktop (`workFold`).
struct TurnView: View {
    @Environment(\.style) private var style
    let segment: RenderedSegment
    let chat: ChatStore
    @State private var unfolded = false

    var body: some View {
        let fold = workFold(segment)
        VStack(alignment: .leading, spacing: 10) {
            if let fold, !unfolded {
                ForEach(runs(Array(segment.items[..<fold.from]))) { run in RunView(run: run, chat: chat) }
                WorkedRow(
                    label: workedLabel(steps: fold.steps, durationMs: segment.end?.durationMs, format: formatElapsed),
                    files: changedFiles(segment)
                ) {
                    withAnimation(.smooth) { unfolded = true }
                }
                ForEach(runs(Array(segment.items[fold.to...]))) { run in RunView(run: run, chat: chat) }
            } else {
                if fold != nil {
                    Button {
                        withAnimation(.smooth) { unfolded = false }
                    } label: {
                        Label("Fold the work", systemImage: "chevron.up")
                            .o7Font(.meta)
                            .foregroundStyle(style.fg3)
                    }
                    .buttonStyle(.plain)
                }
                ForEach(runs(segment.items)) { run in RunView(run: run, chat: chat) }
            }
            if let plan = segment.plan, !plan.isEmpty, segment.end == nil {
                PlanCard(entries: plan)
            }
            if let end = segment.end, end.stopReason != .endTurn {
                Text(stopReasonLabel(end))
                    .o7Font(.meta)
                    .foregroundStyle(end.stopReason == .error ? style.red : style.fg3)
            } else if let end = segment.end, let line = footer(end, segment.usage) {
                Text(line)
                    .o7Font(.small)
                    .monospacedDigit()
                    .foregroundStyle(style.fg4)
            }
        }
    }

    private func footer(_ end: TurnEnd, _ usage: UsageInfo?) -> String? {
        var parts: [String] = []
        if let duration = end.durationMs { parts.append("Done in \(formatElapsed(Double(duration)))") }
        if let used = usage?.contextUsed, let size = usage?.contextSize, size > 0 {
            parts.append("\(Int((Double(used) / Double(size) * 100).rounded()))% context")
        }
        if let cost = usage?.cost, let amount = cost.amount, amount > 0 {
            parts.append(amount.formatted(.currency(code: cost.currency).precision(.fractionLength(2))))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// Items in order, with each stretch of tool calls gathered into one run, as the desktop
    /// gathers them under one header.
    private func runs(_ items: [RenderedItem]) -> [Run] {
        var runs: [Run] = []
        for item in items {
            if case .tool = item.content, case .tools(var tools) = runs.last?.kind {
                tools.append(item)
                runs[runs.count - 1] = Run(kind: .tools(tools))
            } else if case .tool = item.content {
                runs.append(Run(kind: .tools([item])))
            } else {
                runs.append(Run(kind: .item(item)))
            }
        }
        return runs
    }
}

struct Run: Identifiable {
    enum Kind {
        case item(RenderedItem)
        case tools([RenderedItem])
    }
    let kind: Kind
    var id: String {
        switch kind {
        case .item(let item): item.key
        case .tools(let tools): "tools:" + (tools.first?.key ?? "")
        }
    }
}

private struct RunView: View {
    let run: Run
    let chat: ChatStore

    var body: some View {
        switch run.kind {
        case .tools(let tools): StepsGroup(tools: tools)
        case .item(let item): ItemView(item: item, chat: chat)
        }
    }
}

/// "Worked for 3 min 12 s · 12 steps", and the files the turn changed; a tap unfolds the work.
private struct WorkedRow: View {
    @Environment(\.style) private var style
    let label: String
    let files: [String]
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 8) {
                    Image(systemName: "checkmark.circle")
                        .foregroundStyle(style.fg3)
                    Text(label)
                        .o7Font(.meta)
                        .monospacedDigit()
                        .foregroundStyle(style.fg2)
                    Image(systemName: "chevron.down")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(style.fg4)
                    Spacer(minLength: 0)
                }
                if !files.isEmpty {
                    Text(files.prefix(3).map { ($0 as NSString).lastPathComponent }.joined(separator: ", ")
                        + (files.count > 3 ? " and \(files.count - 3) more" : ""))
                        .o7Font(.small)
                        .foregroundStyle(style.fg3)
                        .lineLimit(1)
                }
            }
            .padding(.vertical, 8)
            .padding(.horizontal, 12)
            .background(style.card, in: .rect(cornerRadius: 12))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityHint("Shows the steps")
    }
}

/// The agent's plan for the turn, as a checklist.
struct PlanCard: View {
    @Environment(\.style) private var style
    @ScaledMetric(relativeTo: .subheadline) private var mark: CGFloat = 16
    let entries: [PlanEntry]

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Plan").o7Font(.ui(13, weight: .medium)).foregroundStyle(style.fg2)
            ForEach(Array(entries.enumerated()), id: \.offset) { _, entry in
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Group {
                        switch entry.status {
                        case .completed:
                            Image(systemName: "checkmark.circle.fill").foregroundStyle(style.green)
                        case .inProgress:
                            PixelLoader(size: 12, tempo: .tool)
                        case .pending:
                            Image(systemName: "circle").foregroundStyle(style.fg4)
                        }
                    }
                    .frame(width: mark)
                    Text(entry.content)
                        .o7Font(.chat)
                        .foregroundStyle(entry.status == .completed ? style.fg3 : style.fg1)
                        .strikethrough(entry.status == .completed, color: style.fg4)
                }
            }
        }
        .padding(14)
        .background(style.card, in: .rect(cornerRadius: 16))
    }
}
