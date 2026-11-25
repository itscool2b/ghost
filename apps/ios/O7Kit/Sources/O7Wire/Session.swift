public import Foundation

/// Live events of the followed chat, numbered from `seqStart` (`frame` event).
public struct EventBatch: Codable, Hashable, Sendable {
    public var chatId: String
    public var seqStart: Int64
    public var events: [O7Event]
    enum CodingKeys: String, CodingKey {
        case events
        case chatId = "chat_id", seqStart = "seq_start"
    }
    public init(chatId: String, seqStart: Int64, events: [O7Event]) {
        self.chatId = chatId
        self.seqStart = seqStart
        self.events = events
    }
}

/// One stored event of a chat's history.
public struct HistoryItem: Codable, Hashable, Sendable {
    public var seq: Int64
    public var turnId: String?
    public var at: Int64
    public var event: O7Event
    enum CodingKeys: String, CodingKey {
        case seq, at, event
        case turnId = "turn_id"
    }
    public init(seq: Int64, turnId: String?, at: Int64, event: O7Event) {
        self.seq = seq
        self.turnId = turnId
        self.at = at
        self.event = event
    }
}

/// Which agent ran a turn, for chats handed from one agent to another.
public struct TurnAgent: Codable, Hashable, Sendable {
    public var turnId: String
    public var agentId: String
    public var agentName: String
    enum CodingKeys: String, CodingKey {
        case turnId = "turn_id", agentId = "agent_id", agentName = "agent_name"
    }
}

/// A page of a chat's stored history, oldest first.
public struct HistoryPage: Codable, Hashable, Sendable {
    public var chatId: String
    public var items: [HistoryItem]
    /// Older history exists before this page.
    public var hasMore: Bool
    public var beforeSeq: Int64?
    /// The turn this page starts in the middle of, if it does.
    public var cutTurnId: String?
    public var unsavedFrom: Int64?
    public var skipped: Int64
    public var turnAgents: [TurnAgent]?

    enum CodingKeys: String, CodingKey {
        case items, skipped
        case chatId = "chat_id", hasMore = "has_more", beforeSeq = "before_seq"
        case cutTurnId = "cut_turn_id", unsavedFrom = "unsaved_from", turnAgents = "turn_agents"
    }

    public init(
        chatId: String, items: [HistoryItem], hasMore: Bool, beforeSeq: Int64? = nil,
        cutTurnId: String? = nil, unsavedFrom: Int64? = nil, skipped: Int64 = 0,
        turnAgents: [TurnAgent]? = nil
    ) {
        self.chatId = chatId
        self.items = items
        self.hasMore = hasMore
        self.beforeSeq = beforeSeq
        self.cutTurnId = cutTurnId
        self.unsavedFrom = unsavedFrom
        self.skipped = skipped
        self.turnAgents = turnAgents
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        chatId = try c.decode(String.self, forKey: .chatId)
        items = try c.decodeIfPresent([HistoryItem].self, forKey: .items) ?? []
        hasMore = try c.decodeIfPresent(Bool.self, forKey: .hasMore) ?? false
        beforeSeq = try c.decodeIfPresent(Int64.self, forKey: .beforeSeq)
        cutTurnId = try c.decodeIfPresent(String.self, forKey: .cutTurnId)
        unsavedFrom = try c.decodeIfPresent(Int64.self, forKey: .unsavedFrom)
        skipped = try c.decodeIfPresent(Int64.self, forKey: .skipped) ?? 0
        turnAgents = try c.decodeIfPresent([TurnAgent].self, forKey: .turnAgents)
    }
}

/// A message waiting behind the running turn.
public struct QueuedPrompt: Codable, Hashable, Sendable, Identifiable {
    public var promptId: String
    public var preview: String
    public var held: Bool
    public var createdAt: Int64
    public var id: String { promptId }
    enum CodingKeys: String, CodingKey {
        case preview, held
        case promptId = "prompt_id", createdAt = "created_at"
    }
}

public struct ErrorInfo: Codable, Hashable, Sendable {
    public var kind: ErrorKind
    public var message: String
    public var detail: String?
}

/// A chat's state when it is opened: its summary, its session, its queue.
public struct ChatSnapshot: Codable, Hashable, Sendable {
    public var chat: ChatSummary
    public var session: SessionInfo
    public var queue: [QueuedPrompt]
    public var openTurnId: String?
    public var usage: UsageInfo?
    public var limits: [LimitWindow]?
    public var lastError: ErrorInfo?
    /// A sign-in the agent needs, answered on the computer.
    public var auth: JSONValue?
    public var storage: JSONValue?
    public var tasks: [BackgroundTask]?

    enum CodingKeys: String, CodingKey {
        case chat, session, queue, usage, limits, auth, storage, tasks
        case openTurnId = "open_turn_id", lastError = "last_error"
    }

    public init(chat: ChatSummary, session: SessionInfo) {
        self.chat = chat
        self.session = session
        queue = []
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        chat = try c.decode(ChatSummary.self, forKey: .chat)
        session = try c.decode(SessionInfo.self, forKey: .session)
        queue = try c.decodeIfPresent([QueuedPrompt].self, forKey: .queue) ?? []
        openTurnId = try c.decodeIfPresent(String.self, forKey: .openTurnId)
        usage = try c.decodeIfPresent(UsageInfo.self, forKey: .usage)
        limits = try c.decodeIfPresent([LimitWindow].self, forKey: .limits)
        lastError = try c.decodeIfPresent(ErrorInfo.self, forKey: .lastError)
        auth = try c.decodeIfPresent(JSONValue.self, forKey: .auth)
        storage = try c.decodeIfPresent(JSONValue.self, forKey: .storage)
        tasks = try c.decodeIfPresent([BackgroundTask].self, forKey: .tasks)
    }
}

/// What `open` returns: the chat's state and newest history, and where live events go on.
public struct ChatAttach: Codable, Hashable, Sendable {
    public var subscriptionId: String
    public var nextSeq: Int64
    public var snapshot: ChatSnapshot
    public var history: HistoryPage
    enum CodingKeys: String, CodingKey {
        case snapshot, history
        case subscriptionId = "subscription_id", nextSeq = "next_seq"
    }
    public init(subscriptionId: String, nextSeq: Int64, snapshot: ChatSnapshot, history: HistoryPage) {
        self.subscriptionId = subscriptionId
        self.nextSeq = nextSeq
        self.snapshot = snapshot
        self.history = history
    }
}
