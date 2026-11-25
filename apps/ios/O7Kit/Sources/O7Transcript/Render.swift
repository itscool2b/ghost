public import O7Wire

// The transcript as the screen shows it (`transcript.ts`, from `hasProcess` on), as plain data.

/// Whether a session in this status has an agent process.
public func hasProcess(_ status: SessionStatus) -> Bool {
    switch status {
    case .starting, .ready, .running, .cancelling, .stopping: true
    case .stopped, .failed, .authRequired: false
    }
}

/// What a tool's status mark shows: its own status, or `stopped` when nothing will finish it.
public enum ToolDisplayStatus: String, Sendable, Hashable, Codable, CaseIterable {
    case pending
    case inProgress = "in_progress"
    case completed, failed, stopped

    public init(_ status: ToolStatus) {
        switch status {
        case .pending: self = .pending
        case .inProgress: self = .inProgress
        case .completed: self = .completed
        case .failed: self = .failed
        }
    }
}

/// A tool still pending or running shows as stopped when its turn ended cancelled, interrupted or
/// failed, or when the session has no process; otherwise it keeps what the agent reported (a
/// background task may still update it).
public func toolDisplayStatus(_ tool: ToolItem, end: TurnEnd?, live: Bool) -> ToolDisplayStatus {
    if tool.status != .pending && tool.status != .inProgress { return ToolDisplayStatus(tool.status) }
    if !live { return .stopped }
    switch end?.stopReason {
    case .cancelled, .interrupted, .error: return .stopped
    default: return ToolDisplayStatus(tool.status)
    }
}

/// Why a turn ended, as its footer says it.
public func stopReasonLabel(_ end: TurnEnd) -> String {
    switch end.stopReason {
    case .endTurn: "Done"
    case .maxTokens: "Stopped at the output limit"
    case .maxTurnRequests: "Stopped at the step limit"
    case .refusal: "The agent declined"
    case .cancelled: "Stopped"
    case .interrupted: end.recovered ? "Interrupted when o7 closed" : "Interrupted"
    case .error: "Failed"
    case .other: "Ended"
    }
}

/// A thought shorter than this, with no words, is not worth a row.
private let quietThoughtMs: Int64 = 1000

/// Whether a thought gets a row: while the agent thinks it, or once done when it has words or took
/// a second or more. One with neither (an agent that keeps its reasoning to itself, a blink) stays
/// in the model only.
public func shownThought(_ thought: ThoughtItem, thinking: Bool) -> Bool {
    thinking || !JS.trim(thought.text.units).isEmpty || (thought.durationMs ?? 0) >= quietThoughtMs
}

/// A thought is being thought while it is not done in a turn still running on a live session.
func thinkingNow(_ thought: ThoughtItem, end: TurnEnd?, live: Bool) -> Bool {
    !thought.completed && end == nil && live
}

/// An item as the transcript shows it: tools carry the status their mark shows, and thoughts
/// whether the agent is thinking them now.
public struct RenderedItem: Sendable, Hashable, Identifiable {
    public enum Content: Sendable, Hashable {
        case user(UserItem)
        case message(MessageItem)
        case thought(ThoughtItem, thinking: Bool)
        case tool(ToolItem, display: ToolDisplayStatus)
        case permission(PermissionItem)
        case question(QuestionItem)
        case notice(NoticeItem)
        case error(ErrorItem)
        case gap(GapItem)
    }

    public var content: Content
    /// The item's revision in the transcript: unchanged while the reducer has not touched it.
    public var revision: UInt64

    public init(_ content: Content, revision: UInt64 = 0) {
        self.content = content
        self.revision = revision
    }

    /// The model item this shows.
    public var item: Item {
        switch content {
        case .user(let item): .user(item)
        case .message(let item): .message(item)
        case .thought(let item, _): .thought(item)
        case .tool(let item, _): .tool(item)
        case .permission(let item): .permission(item)
        case .question(let item): .question(item)
        case .notice(let item): .notice(item)
        case .error(let item): .error(item)
        case .gap(let item): .gap(item)
        }
    }

    public var key: String { item.key }
    public var kind: ItemKind { item.kind }
    public var id: String { key }

    /// What a tool's mark shows; nil for every other kind.
    public var display: ToolDisplayStatus? {
        if case .tool(_, let display) = content { return display }
        return nil
    }

    /// Whether the agent thinks a thought now; nil for every other kind.
    public var thinking: Bool? {
        if case .thought(_, let thinking) = content { return thinking }
        return nil
    }
}

extension RenderedItem: Codable {
    /// The item's own fields, with `display` for a tool and `thinking` for a thought.
    public init(from decoder: any Decoder) throws {
        let item = try Item(from: decoder)
        let container = try decoder.container(keyedBy: AnyKey.self)
        switch item {
        case .user(let item): content = .user(item)
        case .message(let item): content = .message(item)
        case .thought(let item):
            content = .thought(item, thinking: try container.decodeIfPresent(Bool.self, forKey: AnyKey("thinking")) ?? false)
        case .tool(let item):
            let display = try container.decodeIfPresent(ToolDisplayStatus.self, forKey: AnyKey("display"))
            content = .tool(item, display: display ?? ToolDisplayStatus(item.status))
        case .permission(let item): content = .permission(item)
        case .question(let item): content = .question(item)
        case .notice(let item): content = .notice(item)
        case .error(let item): content = .error(item)
        case .gap(let item): content = .gap(item)
        }
        revision = 0
    }

    public func encode(to encoder: any Encoder) throws {
        try item.encode(to: encoder)
        var container = encoder.container(keyedBy: AnyKey.self)
        switch content {
        case .tool(_, let display): try container.encode(display, forKey: AnyKey("display"))
        case .thought(_, let thinking): try container.encode(thinking, forKey: AnyKey("thinking"))
        default: break
        }
    }
}

public struct RenderedSegment: Sendable, Hashable, Identifiable {
    public var key: String
    public var turnId: String?
    public var partial: Bool
    public var items: [RenderedItem]
    public var plan: [PlanEntry]?
    public var usage: UsageInfo?
    public var end: TurnEnd?
    /// Changes whenever anything this segment shows may have: the segment, any of its items, or
    /// whether the session is live. Equal revisions mean an equal rendering, so a turn's view can
    /// skip redrawing. A hidden thought's change also gives a new one, which only costs a redraw.
    public var revision: UInt64

    public var id: String { key }

    public init(
        key: String, turnId: String?, partial: Bool, items: [RenderedItem], plan: [PlanEntry]? = nil,
        usage: UsageInfo? = nil, end: TurnEnd? = nil, revision: UInt64 = 0
    ) {
        self.key = key
        self.turnId = turnId
        self.partial = partial
        self.items = items
        self.plan = plan
        self.usage = usage
        self.end = end
        self.revision = revision
    }
}

extension RenderedSegment: Codable {
    enum CodingKeys: String, CodingKey {
        case key, turnId, partial, items, plan, usage, end
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        key = try container.decode(String.self, forKey: .key)
        turnId = try container.decodeIfPresent(String.self, forKey: .turnId)
        partial = try container.decodeIfPresent(Bool.self, forKey: .partial) ?? false
        items = try container.decodeIfPresent([RenderedItem].self, forKey: .items) ?? []
        plan = try container.decodeIfPresent([PlanEntry].self, forKey: .plan)
        usage = try container.decodeIfPresent(UsageInfo.self, forKey: .usage)
        end = try container.decodeIfPresent(TurnEnd.self, forKey: .end)
        revision = 0
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(key, forKey: .key)
        try container.encode(turnId, forKey: .turnId)
        try container.encode(partial, forKey: .partial)
        try container.encode(items, forKey: .items)
        try container.encode(plan, forKey: .plan)
        try container.encode(usage, forKey: .usage)
        try container.encode(end, forKey: .end)
    }
}

extension Transcript {
    /// A segment's items the screen shows, in order: every kind, thoughts only where they say
    /// something.
    fileprivate func shownItems(_ segment: Segment, live: Bool) -> [(Item, UInt64)] {
        segment.itemKeys.compactMap { key in
            guard let item = items[key] else { return nil }
            if case .thought(let thought) = item,
                !shownThought(thought, thinking: thinkingNow(thought, end: segment.end, live: live))
            {
                return nil
            }
            return (item, revision(ofItem: key))
        }
    }

    /// The revision the segment at `index` renders with: the newest of its own and its items',
    /// with whether the session is live in the lowest bit.
    func renderRevision(_ index: Int, live: Bool) -> UInt64 {
        var newest = revision(ofSegment: index)
        for key in segments[index].itemKeys { newest = Swift.max(newest, revision(ofItem: key)) }
        return newest << 1 | (live ? 1 : 0)
    }

    func renderSegment(_ index: Int, live: Bool) -> RenderedSegment {
        let segment = segments[index]
        let rendered = shownItems(segment, live: live).map { item, revision in
            let content: RenderedItem.Content =
                switch item {
                case .user(let item): .user(item)
                case .message(let item): .message(item)
                case .thought(let thought):
                    .thought(thought, thinking: thinkingNow(thought, end: segment.end, live: live))
                case .tool(let tool): .tool(tool, display: toolDisplayStatus(tool, end: segment.end, live: live))
                case .permission(let item): .permission(item)
                case .question(let item): .question(item)
                case .notice(let item): .notice(item)
                case .error(let item): .error(item)
                case .gap(let item): .gap(item)
                }
            return RenderedItem(content, revision: revision)
        }
        return RenderedSegment(
            key: segment.key, turnId: segment.turnId, partial: segment.partial, items: rendered,
            plan: segment.plan, usage: segment.usage, end: segment.end,
            revision: renderRevision(index, live: live))
    }
}

/// The transcript as the screen shows it, as plain data: what "compacted history and live stream
/// render the same" compares. `live`: the session has a process.
public func renderTranscript(_ model: Transcript, live: Bool) -> [RenderedSegment] {
    model.segments.indices.map { model.renderSegment($0, live: live) }
}

/// `renderTranscript` that reuses each rendered segment whose revision did not change, so a frame
/// rebuilds only the turn it touched (`renderCache` on the desktop).
public struct RenderCache: Sendable {
    private var previous: [ExactKey: RenderedSegment] = [:]

    public init() {}

    public mutating func render(_ model: Transcript, live: Bool) -> [RenderedSegment] {
        var next: [ExactKey: RenderedSegment] = [:]
        let rendered = model.segments.indices.map { index in
            let key = ExactKey(model.segments[index].key)
            let segment: RenderedSegment
            if let cached = previous[key], cached.revision == model.renderRevision(index, live: live) {
                segment = cached
            } else {
                segment = model.renderSegment(index, live: live)
            }
            next[key] = segment
            return segment
        }
        previous = next
        return rendered
    }
}

/// The newest error card in the transcript, whose actions are the ones that still apply.
public func lastErrorKey(_ model: Transcript) -> String? {
    for segment in model.segments.reversed() {
        for key in segment.itemKeys.reversed() {
            if case .error = model.items[key] { return key }
        }
    }
    return nil
}

/// Whether the transcript holds a turn, so a start can resume an earlier session.
public func hasTurns(_ model: Transcript) -> Bool {
    model.segments.contains { $0.turnId != nil }
}

extension Transcript {
    public var lastErrorKey: String? { O7Transcript.lastErrorKey(self) }
    public var hasTurns: Bool { O7Transcript.hasTurns(self) }
}
