public import O7Wire
internal import Synchronization

// A chat's transcript (`transcript.ts`), rebuilt from its events in seq order by a pure reducer.
// History pages and live frames go through the same reducer, so a compacted page and the live
// stream it came from render the same.
//
// Turns are grouped by position, from a `turn_started` to its `turn_ended`, never by the turn tag a
// stored row carries: an untagged late event that landed inside a later turn's span renders in that
// turn, in history and live alike. Events between turns form outside segments. Items keep the order
// of their first appearance and are updated in place by id (a message's deltas, a tool's updates, a
// request's resolution), even when the update arrives in a later turn. A turn's plan and usage are
// its last ones, shown with the turn rather than at a position, since compaction keeps only the last
// of each.
//
// Every type here encodes as the desktop's TypeScript object does, so the goldens compare them as
// JSON. Revisions are bookkeeping for the screen and are never encoded.

/// One event at its place in the chat: a history item, or an event of a live frame.
public struct EventEntry: Sendable, Hashable {
    public var seq: Int64
    /// The stored row's turn tag; live frames carry none. Grouping never reads it.
    public var turnId: String?
    public var event: O7Event

    public init(seq: Int64, turnId: String?, event: O7Event) {
        self.seq = seq
        self.turnId = turnId
        self.event = event
    }
}

/// Where saving paused: rows from `seq` up to the attach point are not saved yet, so not shown.
public struct GapEntry: Sendable, Hashable {
    public var seq: Int64
    public init(seq: Int64) { self.seq = seq }
}

public enum Entry: Sendable, Hashable {
    case event(EventEntry)
    case gap(GapEntry)

    public static func event(seq: Int64, turnId: String? = nil, event: O7Event) -> Entry {
        .event(EventEntry(seq: seq, turnId: turnId, event: event))
    }

    public static func gap(seq: Int64) -> Entry { .gap(GapEntry(seq: seq)) }

    public var seq: Int64 {
        switch self {
        case .event(let entry): entry.seq
        case .gap(let gap): gap.seq
        }
    }
}

extension Entry: Codable {
    /// `{ seq, turnId, event }` for an event, `{ seq, gap: true }` for a gap, as in TypeScript.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: AnyKey.self)
        let seq = try container.decode(Int64.self, forKey: AnyKey("seq"))
        if try container.decodeIfPresent(Bool.self, forKey: AnyKey("gap")) == true {
            self = .gap(seq: seq)
        } else {
            self = .event(
                seq: seq,
                turnId: try container.decodeIfPresent(String.self, forKey: AnyKey("turnId")),
                event: try container.decode(O7Event.self, forKey: AnyKey("event")))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: AnyKey.self)
        switch self {
        case .event(let entry):
            try container.encode(entry.seq, forKey: AnyKey("seq"))
            try container.encode(entry.turnId, forKey: AnyKey("turnId"))
            try container.encode(entry.event, forKey: AnyKey("event"))
        case .gap(let gap):
            try container.encode(gap.seq, forKey: AnyKey("seq"))
            try container.encode(true, forKey: AnyKey("gap"))
        }
    }
}

extension HistoryPage {
    /// The page's rows as entries, oldest first, as the web client reads them. The desktop also
    /// marks unsaved rows with `Entry.gap(seq: unsavedFrom)` after them.
    public var entries: [Entry] {
        items.map { .event(seq: $0.seq, turnId: $0.turnId, event: $0.event) }
    }
}

extension EventBatch {
    /// The batch's events as entries, numbered from `seqStart`, without turn tags.
    public var entries: [Entry] {
        events.enumerated().map { .event(seq: seqStart + Int64($0.offset), turnId: nil, event: $0.element) }
    }
}

// MARK: Items

public enum ItemKind: String, Sendable, Hashable, Codable, CaseIterable {
    case user, message, thought, tool, permission, question, notice, error, gap
}

/// A steered message's delivery: whether the running turn took it.
public struct Steering: Sendable, Hashable, Codable {
    public var state: SteeringState
    public var detail: String?

    public init(state: SteeringState, detail: String?) {
        self.state = state
        self.detail = detail
    }
}

public struct UserItem: Sendable, Hashable, Codable {
    public var key: String
    /// What the person sent. Project memory attached after it is in `memory`, not here.
    public var text: String
    /// Project memory o7 attached to this message as reference context, when it attached any.
    public var memory: MemoryContext?
    /// The handoff this message carried to a new agent, before the person's words.
    public var handoff: String?
    /// With `memory` or `handoff`: the whole saved message, which later pieces are appended to.
    public var sent: String?
    public var steering: Steering?

    public init(
        key: String, text: String, memory: MemoryContext? = nil, handoff: String? = nil,
        sent: String? = nil, steering: Steering? = nil
    ) {
        self.key = key
        self.text = text
        self.memory = memory
        self.handoff = handoff
        self.sent = sent
        self.steering = steering
    }
}

/// A pending delivery cannot be claimed after its turn ends or the session is recovered.
public func steeringState(_ item: UserItem, settled: Bool) -> SteeringState? {
    let state = item.steering?.state
    return settled && state == .pending ? .unconfirmed : state
}

public struct MessageItem: Sendable, Hashable, Codable {
    public var key: String
    public var text: String
    public var completed: Bool

    public init(key: String, text: String, completed: Bool) {
        self.key = key
        self.text = text
        self.completed = completed
    }
}

public struct ThoughtItem: Sendable, Hashable, Codable {
    public var key: String
    public var text: String
    public var completed: Bool
    public var durationMs: Int64?

    public init(key: String, text: String, completed: Bool, durationMs: Int64?) {
        self.key = key
        self.text = text
        self.completed = completed
        self.durationMs = durationMs
    }
}

public struct ToolItem: Sendable, Hashable, Codable {
    public var key: String
    public var id: String
    public var toolKind: ToolKind
    public var title: String
    public var status: ToolStatus
    public var locations: [ToolLocation]
    public var rawInput: JSONValue?
    public var toolName: String?
    public var content: [ToolContent]
    /// More than entry text was cut; `blob` holds the original event's JSON ("Show full details").
    public var truncated: Truncation?

    public init(
        key: String, id: String, toolKind: ToolKind, title: String, status: ToolStatus,
        locations: [ToolLocation] = [], rawInput: JSONValue? = nil, toolName: String? = nil,
        content: [ToolContent] = [], truncated: Truncation? = nil
    ) {
        self.key = key
        self.id = id
        self.toolKind = toolKind
        self.title = title
        self.status = status
        self.locations = locations
        self.rawInput = rawInput
        self.toolName = toolName
        self.content = content
        self.truncated = truncated
    }
}

public struct PermissionResolution: Sendable, Hashable, Codable {
    public var decision: PermissionDecision
    public var by: ResolvedBy

    public init(decision: PermissionDecision, by: ResolvedBy) {
        self.decision = decision
        self.by = by
    }
}

public struct PermissionItem: Sendable, Hashable, Codable {
    public var key: String
    public var request: PermissionRequest
    /// Nil while the card waits for an answer.
    public var resolution: PermissionResolution?

    public init(key: String, request: PermissionRequest, resolution: PermissionResolution?) {
        self.key = key
        self.request = request
        self.resolution = resolution
    }
}

public struct QuestionResolution: Sendable, Hashable, Codable {
    /// Nil: dismissed, withdrawn or closed.
    public var answer: UiAnswer?
    public var by: ResolvedBy

    public init(answer: UiAnswer?, by: ResolvedBy) {
        self.answer = answer
        self.by = by
    }
}

public struct QuestionItem: Sendable, Hashable, Codable {
    public var key: String
    public var request: UiRequest
    public var resolution: QuestionResolution?

    public init(key: String, request: UiRequest, resolution: QuestionResolution?) {
        self.key = key
        self.request = request
        self.resolution = resolution
    }
}

public struct NoticeItem: Sendable, Hashable, Codable {
    public var key: String
    public var level: NoticeLevel
    public var message: String

    public init(key: String, level: NoticeLevel, message: String) {
        self.key = key
        self.level = level
        self.message = message
    }
}

public struct ErrorItem: Sendable, Hashable, Codable {
    public var key: String
    public var errorKind: ErrorKind
    public var message: String
    public var detail: String?
    public var recoverable: Bool

    public init(key: String, errorKind: ErrorKind, message: String, detail: String?, recoverable: Bool) {
        self.key = key
        self.errorKind = errorKind
        self.message = message
        self.detail = detail
        self.recoverable = recoverable
    }
}

/// The unsaved-rows marker.
public struct GapItem: Sendable, Hashable, Codable {
    public var key: String
    public init(key: String) { self.key = key }
}

public enum Item: Sendable, Hashable {
    case user(UserItem)
    case message(MessageItem)
    case thought(ThoughtItem)
    case tool(ToolItem)
    case permission(PermissionItem)
    case question(QuestionItem)
    case notice(NoticeItem)
    case error(ErrorItem)
    case gap(GapItem)

    public var key: String {
        switch self {
        case .user(let item): item.key
        case .message(let item): item.key
        case .thought(let item): item.key
        case .tool(let item): item.key
        case .permission(let item): item.key
        case .question(let item): item.key
        case .notice(let item): item.key
        case .error(let item): item.key
        case .gap(let item): item.key
        }
    }

    public var kind: ItemKind {
        switch self {
        case .user: .user
        case .message: .message
        case .thought: .thought
        case .tool: .tool
        case .permission: .permission
        case .question: .question
        case .notice: .notice
        case .error: .error
        case .gap: .gap
        }
    }
}

extension Item: Codable {
    public init(from decoder: any Decoder) throws {
        let kind = try decoder.container(keyedBy: AnyKey.self).decode(ItemKind.self, forKey: AnyKey("kind"))
        switch kind {
        case .user: self = .user(try UserItem(from: decoder))
        case .message: self = .message(try MessageItem(from: decoder))
        case .thought: self = .thought(try ThoughtItem(from: decoder))
        case .tool: self = .tool(try ToolItem(from: decoder))
        case .permission: self = .permission(try PermissionItem(from: decoder))
        case .question: self = .question(try QuestionItem(from: decoder))
        case .notice: self = .notice(try NoticeItem(from: decoder))
        case .error: self = .error(try ErrorItem(from: decoder))
        case .gap: self = .gap(try GapItem(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        switch self {
        case .user(let item): try item.encode(to: encoder)
        case .message(let item): try item.encode(to: encoder)
        case .thought(let item): try item.encode(to: encoder)
        case .tool(let item): try item.encode(to: encoder)
        case .permission(let item): try item.encode(to: encoder)
        case .question(let item): try item.encode(to: encoder)
        case .notice(let item): try item.encode(to: encoder)
        case .error(let item): try item.encode(to: encoder)
        case .gap(let item): try item.encode(to: encoder)
        }
        var container = encoder.container(keyedBy: AnyKey.self)
        try container.encode(kind, forKey: AnyKey("kind"))
    }
}

// MARK: Segments

public struct TurnEnd: Sendable, Hashable, Codable {
    public var stopReason: StopReason
    public var usage: TokenUsage?
    public var durationMs: Int64?
    /// Ended by recovery at startup: o7 quit or crashed while the turn ran.
    public var recovered: Bool

    public init(stopReason: StopReason, usage: TokenUsage? = nil, durationMs: Int64? = nil, recovered: Bool = false) {
        self.stopReason = stopReason
        self.usage = usage
        self.durationMs = durationMs
        self.recovered = recovered
    }
}

/// A turn, or the events between two turns.
public struct Segment: Sendable, Hashable, Codable {
    public var key: String
    /// Nil for the events between turns.
    public var turnId: String?
    public var promptId: String?
    /// A history page cut this turn: its earlier part is in an older page.
    public var partial: Bool
    public var itemKeys: [String]
    public var plan: [PlanEntry]?
    /// The turn's last usage report (context and cost).
    public var usage: UsageInfo?
    /// Nil while the turn runs (and always for the events between turns).
    public var end: TurnEnd?

    public init(
        key: String, turnId: String?, promptId: String?, partial: Bool, itemKeys: [String] = [],
        plan: [PlanEntry]? = nil, usage: UsageInfo? = nil, end: TurnEnd? = nil
    ) {
        self.key = key
        self.turnId = turnId
        self.promptId = promptId
        self.partial = partial
        self.itemKeys = itemKeys
        self.plan = plan
        self.usage = usage
        self.end = end
    }
}

public struct Transcript: Sendable {
    public internal(set) var segments: [Segment]
    public internal(set) var items: OrderedMap<Item>
    /// Background tasks by id: not items, since the dock and the task panel show them (Tasks.swift).
    public internal(set) var tasks: OrderedMap<TaskItem>

    /// Each item's revision: a new one every time the reducer sets it, even to equal content, as
    /// the desktop makes a new object.
    var itemRevisions: [ExactKey: UInt64] = [:]
    /// Each segment's revision, by position: a new one whenever the segment changes.
    var segmentRevisions: [UInt64] = []

    init(segments: [Segment], items: OrderedMap<Item>, tasks: OrderedMap<TaskItem>) {
        self.segments = segments
        self.items = items
        self.tasks = tasks
        segmentRevisions = segments.map { _ in nextRevision() }
    }

    /// The revision the reducer last gave the item with this key; 0 for none.
    public func revision(ofItem key: String) -> UInt64 {
        itemRevisions[ExactKey(key)] ?? 0
    }

    /// The revision of the segment at `index`: it changes whenever the segment does (not its
    /// items; `RenderedSegment.revision` covers those).
    public func revision(ofSegment index: Int) -> UInt64 {
        segmentRevisions.indices.contains(index) ? segmentRevisions[index] : 0
    }
}

extension Transcript: Equatable {
    /// Equal content: revisions are not compared.
    public static func == (lhs: Transcript, rhs: Transcript) -> Bool {
        lhs.segments == rhs.segments && lhs.items == rhs.items && lhs.tasks == rhs.tasks
    }
}

private let revisionCounter = Atomic<UInt64>(0)

/// A revision no other change has had, in this process.
func nextRevision() -> UInt64 {
    revisionCounter.add(1, ordering: .relaxed).newValue
}
