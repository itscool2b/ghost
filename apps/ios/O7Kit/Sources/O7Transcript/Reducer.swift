public import O7Wire
internal import os

private let reducerLog = Logger(subsystem: "codes.o7.mobile", category: "transcript")

// The reducer of `transcript.ts`. The desktop's `Draft` copies each structure at most once per
// batch; a Swift value does the same by itself: the first change to a shared array or map copies
// it, and the rest of the batch changes the copy in place.

/// The title of a tool call that was never announced: the one the chat store gives the call it
/// synthesizes for such a tool when it compacts a turn, so the block reads the same live and from
/// history.
public let unannouncedToolTitle = "Tool"

extension Transcript {
    /// An empty transcript. A history page that cut a turn (`cut_turn_id`) starts inside that turn,
    /// so its first items land in it rather than between turns.
    public static func empty(cut: String? = nil) -> Transcript {
        let segments = cut.map { [turnSegment(key: "turn:\($0)", turnId: $0, promptId: nil, partial: true)] }
        return Transcript(segments: segments ?? [], items: OrderedMap(), tasks: OrderedMap())
    }

    /// Applies entries in seq order. Gives this transcript itself when there are none.
    public func applying(_ entries: [Entry]) -> Transcript {
        if entries.isEmpty { return self }
        var next = self
        next.apply(entries)
        return next
    }

    /// Applies entries in seq order, in place.
    public mutating func apply(_ entries: [Entry]) {
        for entry in entries { apply(entry) }
    }

    /// The live session's tasks (the snapshot's) over the ones history gave: a task whose records
    /// are all in an older page still shows, and a known one takes its newest record.
    public func withTasks(_ snapshot: [BackgroundTask]) -> Transcript {
        if snapshot.isEmpty { return self }
        var next = self
        let turnKey = newestTurnKey(segments)
        for (index, task) in snapshot.enumerated() {
            next.tasks[task.taskId] = mergeTask(
                next.tasks[task.taskId], task, order: Int64(index - snapshot.count), turnKey: turnKey)
        }
        return next
    }

    mutating func apply(_ entry: Entry) {
        switch entry {
        case .gap(let gap):
            let key = "gap:\(gap.seq)"
            place(key, seq: gap.seq, .gap(GapItem(key: key)))
        case .event(let entry):
            apply(entry.event, seq: entry.seq)
        }
    }

    private mutating func apply(_ event: O7Event, seq: Int64) {
        switch event {
        case .turnStarted(let turnId, let promptId):
            // A turn whose end never arrived stays as it is; the new one starts after it.
            push(
                Self.turnSegment(
                    key: freeKey("turn:\(turnId)", seq: seq), turnId: turnId, promptId: promptId,
                    partial: false))

        case .turnEnded(let turnId, let stopReason, let usage, let durationMs, let recovered):
            let end = TurnEnd(stopReason: stopReason, usage: usage, durationMs: durationMs, recovered: recovered)
            if let open = openTurn() {
                setSegment(open) { $0.end = end }
            } else {
                // Its start is not in the loaded history: the end alone still says how it ended.
                var segment = Self.turnSegment(
                    key: freeKey("turn:\(turnId)", seq: seq), turnId: turnId, promptId: nil, partial: true)
                segment.end = end
                push(segment)
            }

        case .userMessageDelta(let messageId, let text):
            let key = "user:\(messageId)"
            let previous = user(key)
            let sent = (previous.map { ($0.sent ?? $0.text).units } ?? []) + text.units
            let (words, memory) = splitMemoryContext(units: sent)
            // A message sent after a change of agent starts with the handoff the agent received.
            let split = splitHandoff(units: words)
            place(
                key, seq: seq,
                .user(
                    UserItem(
                        key: key,
                        text: split?.text ?? String(units: words),
                        memory: memory,
                        handoff: split?.handoff,
                        // Without attached memory or a handoff the message is its own whole text.
                        sent: memory != nil || split != nil ? String(units: sent) : nil,
                        steering: previous?.steering)))

        case .steeringStatus(let messageId, let state, let detail):
            let key = "user:\(messageId)"
            var item = user(key) ?? UserItem(key: key, text: "")
            item.key = key
            item.steering = Steering(state: state, detail: detail)
            place(key, seq: seq, .user(item))

        case .messageDelta(let messageId, let text):
            let key = "message:\(messageId)"
            let previous = message(key)
            place(
                key, seq: seq,
                .message(
                    MessageItem(
                        key: key, text: (previous?.text ?? "") + text,
                        completed: previous?.completed ?? false)))

        case .messageCompleted(let messageId):
            let key = "message:\(messageId)"
            place(key, seq: seq, .message(MessageItem(key: key, text: message(key)?.text ?? "", completed: true)))

        case .thoughtDelta(let thoughtId, let text):
            let key = "thought:\(thoughtId)"
            let previous = thought(key)
            place(
                key, seq: seq,
                .thought(
                    ThoughtItem(
                        key: key, text: (previous?.text ?? "") + text,
                        completed: previous?.completed ?? false, durationMs: previous?.durationMs)))

        case .thoughtCompleted(let thoughtId, let durationMs):
            let key = "thought:\(thoughtId)"
            let previous = thought(key)
            place(
                key, seq: seq,
                .thought(
                    ThoughtItem(
                        key: key, text: previous?.text ?? "", completed: true,
                        durationMs: durationMs ?? previous?.durationMs)))

        case .toolCall(let call):
            let key = "tool:\(call.toolCallId)"
            place(key, seq: seq, .tool(toolFromCall(key: key, call)))

        case .toolUpdate(let update):
            let key = "tool:\(update.toolCallId)"
            let previous = tool(key) ?? unannouncedTool(key: key, id: update.toolCallId)
            place(key, seq: seq, .tool(updateTool(previous, update)))

        case .permissionRequested(let request):
            let key = "permission:\(request.requestId)"
            place(key, seq: seq, .permission(PermissionItem(key: key, request: request, resolution: nil)))

        case .permissionResolved(let requestId, let decision, let by):
            let key = "permission:\(requestId)"
            // A resolution whose request is in an older page has no card to close yet.
            if case .permission(var item) = items[key] {
                item.resolution = PermissionResolution(decision: decision, by: by)
                place(key, seq: seq, .permission(item))
            }

        case .uiRequest(let request):
            let key = "question:\(request.requestId)"
            place(key, seq: seq, .question(QuestionItem(key: key, request: request, resolution: nil)))

        case .uiResolved(let requestId, let answer, let by):
            let key = "question:\(requestId)"
            if case .question(var item) = items[key] {
                item.resolution = QuestionResolution(answer: answer, by: by)
                place(key, seq: seq, .question(item))
            }

        case .planUpdated(let entries):
            setSegment(target(seq: seq)) { $0.plan = entries }

        case .taskUpdated(let task):
            let turnKey = newestTurnKey(segments)
            tasks[task.taskId] = mergeTask(tasks[task.taskId], task, order: seq, turnKey: turnKey)

        case .taskStep(let taskId, let step):
            tasks[taskId] = withStep(tasks[taskId], taskId: taskId, step: step, order: seq)

        case .usageUpdated(let usage):
            // Stored only inside a turn: between turns it is live header state, never history.
            if let open = openTurn() { setSegment(open) { $0.usage = usage } }

        case .limitsUpdated, .sessionUpdated, .promptLifecycle:
            // Live state the chat store keeps (header, queue); never stored, so never history.
            break

        case .notice(let level, let message):
            let key = "notice:\(seq)"
            place(key, seq: seq, .notice(NoticeItem(key: key, level: level, message: message)))

        case .error(let kind, let message, let detail, let recoverable):
            let key = "error:\(seq)"
            place(
                key, seq: seq,
                .error(ErrorItem(key: key, errorKind: kind, message: message, detail: detail, recoverable: recoverable)))

        case .unknown(let type, _):
            // A newer computer's event this copy does not know: skipped. The desktop's reducer is
            // exhaustive and throws instead.
            reducerLog.debug("Skipped an event this copy does not know: \(type, privacy: .public)")
        }
    }

    private func user(_ key: String) -> UserItem? {
        if case .user(let item) = items[key] { return item }
        return nil
    }

    private func message(_ key: String) -> MessageItem? {
        if case .message(let item) = items[key] { return item }
        return nil
    }

    private func thought(_ key: String) -> ThoughtItem? {
        if case .thought(let item) = items[key] { return item }
        return nil
    }

    private func tool(_ key: String) -> ToolItem? {
        if case .tool(let item) = items[key] { return item }
        return nil
    }

    /// Sets an item; a new one joins the segment events land in now.
    private mutating func place(_ key: String, seq: Int64, _ item: Item) {
        let known = items.contains(key)
        items[key] = item
        itemRevisions[ExactKey(key)] = nextRevision()
        if known { return }
        let index = target(seq: seq)
        segments[index].itemKeys.append(key)
        segmentRevisions[index] = nextRevision()
    }

    /// The turn still running at the end, if any.
    private func openTurn() -> Int? {
        guard let last = segments.last, last.turnId != nil, last.end == nil else { return nil }
        return segments.count - 1
    }

    /// Where an event lands: the running turn, the events after the last turn, or a new run of them.
    private mutating func target(seq: Int64) -> Int {
        if let last = segments.last, last.turnId == nil || last.end == nil { return segments.count - 1 }
        return push(Segment(key: freeKey("outside:\(seq)", seq: seq), turnId: nil, promptId: nil, partial: false))
    }

    @discardableResult
    private mutating func push(_ segment: Segment) -> Int {
        segments.append(segment)
        segmentRevisions.append(nextRevision())
        return segments.count - 1
    }

    private mutating func setSegment(_ index: Int, _ change: (inout Segment) -> Void) {
        change(&segments[index])
        segmentRevisions[index] = nextRevision()
    }

    /// `key`, or `key#seq` when a segment already has it (a turn whose end went missing).
    private func freeKey(_ key: String, seq: Int64) -> String {
        segments.contains { $0.key.same(as: key) } ? "\(key)#\(seq)" : key
    }

    static func turnSegment(key: String, turnId: String, promptId: String?, partial: Bool) -> Segment {
        Segment(key: key, turnId: turnId, promptId: promptId, partial: partial)
    }
}

/// An empty transcript (`emptyTranscript`): see `Transcript.empty(cut:)`.
public func emptyTranscript(_ cut: String? = nil) -> Transcript { .empty(cut: cut) }

/// Applies entries in seq order (`applyEntries`). Gives `model` itself when there are none.
public func applyEntries(_ model: Transcript, _ entries: [Entry]) -> Transcript {
    model.applying(entries)
}

/// The reducer for one event: `(model, seq, turnId, event) -> model`.
public func reduce(_ model: Transcript, seq: Int64, turnId: String?, event: O7Event) -> Transcript {
    model.applying([.event(seq: seq, turnId: turnId, event: event)])
}

/// The live session's tasks over the ones history gave: see `Transcript.withTasks(_:)`.
public func withTasks(_ model: Transcript, _ tasks: [BackgroundTask]) -> Transcript {
    model.withTasks(tasks)
}

/// Folds each tool's run of updates into its first one, giving the same transcript with far fewer
/// entries. A tool's place comes from its first event, and an update only overrides the fields it
/// sets, so the later updates' set fields laid over the first update say the same thing. A
/// `tool_call` replaces the whole item, so updates fold only within the stretch after one. A
/// command's streamed output can store tens of thousands of updates, hundreds of megabytes.
public func compactToolUpdates(_ entries: [Entry]) -> [Entry] {
    var folded: [Entry] = []
    var firstUpdate: [ExactKey: Int] = [:]
    for entry in entries {
        guard case .event(let row) = entry else {
            folded.append(entry)
            continue
        }
        if case .toolCall(let call) = row.event { firstUpdate[ExactKey(call.toolCallId)] = nil }
        guard case .toolUpdate(let update) = row.event else {
            folded.append(entry)
            continue
        }
        let id = ExactKey(update.toolCallId)
        if let at = firstUpdate[id], case .event(let first) = folded[at],
            case .toolUpdate(let earlier) = first.event
        {
            folded[at] = .event(seq: first.seq, turnId: first.turnId, event: .toolUpdate(merged(earlier, update)))
        } else {
            firstUpdate[id] = folded.count
            folded.append(entry)
        }
    }
    return folded
}

/// The first update with each field the next one sets laid over it.
private func merged(_ first: ToolUpdate, _ next: ToolUpdate) -> ToolUpdate {
    var out = first
    out.status = next.status ?? first.status
    out.title = next.title ?? first.title
    out.kind = next.kind ?? first.kind
    out.locations = next.locations ?? first.locations
    out.rawInput = next.rawInput ?? first.rawInput
    out.toolName = next.toolName ?? first.toolName
    out.content = next.content ?? first.content
    out.truncated = next.truncated ?? first.truncated
    return out
}

/// A tool call that was never announced: ACP's defaults, with the chat store's title.
private func unannouncedTool(key: String, id: String) -> ToolItem {
    ToolItem(key: key, id: id, toolKind: .other, title: unannouncedToolTitle, status: .pending)
}

private func toolFromCall(key: String, _ call: ToolCall) -> ToolItem {
    ToolItem(
        key: key, id: call.toolCallId, toolKind: call.kind, title: call.title, status: call.status,
        locations: call.locations, rawInput: call.rawInput, toolName: call.toolName,
        content: call.content, truncated: call.truncated)
}

/// ACP's update semantics: nil keeps a field; `content: []` clears the content.
private func updateTool(_ tool: ToolItem, _ update: ToolUpdate) -> ToolItem {
    var out = tool
    out.toolKind = update.kind ?? tool.toolKind
    out.title = update.title ?? tool.title
    out.status = update.status ?? tool.status
    out.locations = update.locations ?? tool.locations
    out.rawInput = update.rawInput ?? tool.rawInput
    out.toolName = update.toolName ?? tool.toolName
    out.content = update.content ?? tool.content
    out.truncated = update.truncated ?? tool.truncated
    return out
}
