// A finished turn's work, folded under one "Worked for" row so the final answer stands alone
// (`workFold.ts`): every item between the person's message and the agent's final answer, the
// interim messages, reasoning, tool calls, answered cards and notices among them. Reasoning alone
// is no work to fold: its own row is folded already.
//
// A turn folds only when it ended normally on an answer, after some work, and nothing in that work
// must stay in view: no error, no request still waiting for an answer, and no message of the
// person's own (a steer). The answer is the run of messages that ends the turn. A turn too long for
// one history page loads cut (`partial`) and folds the work that loaded.

public struct WorkFold: Sendable, Hashable, Codable {
    /// The first folded item, by its index in the segment's items.
    public var from: Int
    /// The first item of the final answer: the fold ends just before it.
    public var to: Int
    /// The tool calls folded, or everything but reasoning when there are none: "12 steps". Nil for
    /// a cut turn, whose loaded part would undercount.
    public var steps: Int?
}

private func staysInView(_ item: RenderedItem) -> Bool {
    switch item.content {
    case .error, .user, .gap: true
    case .permission(let card): card.resolution == nil
    case .question(let card): card.resolution == nil
    default: false
    }
}

public func workFold(_ segment: RenderedSegment) -> WorkFold? {
    guard segment.turnId != nil, segment.end?.stopReason == .endTurn else { return nil }
    let items = segment.items
    // A cut turn's prompt did not load: a message of the person's in it is a steer, kept in view.
    let from = segment.partial ? 0 : (items.firstIndex { $0.kind != .user } ?? -1)
    if from < 0 { return nil }
    var to = items.count
    while to > from && items[to - 1].kind == .message { to -= 1 }
    // No answer at the end, or nothing but the answer.
    if to == items.count || to <= from { return nil }
    let answered = items[to...].contains { item in
        if case .message(let message) = item.content { return !JS.trim(message.text.units).isEmpty }
        return false
    }
    if !answered { return nil }
    let work = items[from..<to]
    let steps = work.filter { $0.kind != .thought }
    if !steps.contains(where: { $0.kind != .message }) || work.contains(where: staysInView) { return nil }
    let tools = steps.filter { $0.kind == .tool }.count
    return WorkFold(from: from, to: to, steps: segment.partial ? nil : (tools != 0 ? tools : steps.count))
}

/// The files the turn's tool calls changed, in the order they were first changed: what the card
/// under the final answer lists. Calls that failed changed nothing and are left out.
public func changedFiles(_ segment: RenderedSegment) -> [String] {
    var seen = ExactSet()
    var files: [String] = []
    for item in segment.items {
        guard case .tool(let tool, let display) = item.content, display != .failed,
            [.edit, .delete, .move].contains(tool.toolKind)
        else { continue }
        for location in tool.locations where !location.path.isEmpty && !seen.contains(location.path) {
            seen.insert(location.path)
            files.append(location.path)
        }
    }
    return files
}

/// "Worked for 1 min 42 s · 12 steps", without a time when the turn did not record one, and
/// without a count when only part of the turn loaded.
public func workedLabel(steps: Int?, durationMs: Int64?, format: (Double) -> String) -> String {
    let time = durationMs.map { "Worked for \(format(Double($0)))" } ?? "Worked"
    guard let steps else { return time }
    return "\(time) · \(steps) \(steps == 1 ? "step" : "steps")"
}
