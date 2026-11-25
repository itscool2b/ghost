public import Foundation

/// The protocol version this app speaks (`wire.rs` `VERSION`).
public let protocolVersion = 1

/// One request to the computer: an operation's name and its fields (`wire.rs` `Op`). A request
/// the computer does not know ends the session there, so the app sends an operation only when
/// the computer listed its feature when the session opened.
public struct RemoteOp: Sendable, Hashable {
    public let name: String
    public let params: [String: JSONValue]
    /// The feature the computer must list for this op, or nil for the first version's ops.
    public let feature: String?

    public init(_ name: String, _ params: [String: JSONValue] = [:], feature: String? = nil) {
        self.name = name
        self.params = params
        self.feature = feature
    }

    /// The request's JSON with its number, as sealed.
    public func json(id: UInt32) throws -> String {
        var object = params
        object["id"] = .number(Double(id))
        object["op"] = .string(name)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return String(decoding: try encoder.encode(JSONValue.object(object)), as: UTF8.self)
    }

    static func text(_ value: String?) -> JSONValue { value.map(JSONValue.string) ?? .null }

    // The first version's operations.
    public static func pair(name: String, kind: String, version: String, model: String) -> RemoteOp {
        RemoteOp("pair", [
            "name": .string(name),
            "client": .object([
                "kind": .string(kind), "version": .string(version), "model": .string(model),
            ]),
        ])
    }
    public static let overview = RemoteOp("overview")
    public static func open(_ chatId: String) -> RemoteOp { RemoteOp("open", ["chat_id": .string(chatId)]) }
    public static let close = RemoteOp("close")
    public static func history(_ chatId: String, before: Int64) -> RemoteOp {
        RemoteOp("history", ["chat_id": .string(chatId), "before_seq": .number(Double(before))])
    }
    public static func send(_ chatId: String, text: String, attachments: [String] = []) -> RemoteOp {
        var params: [String: JSONValue] = ["chat_id": .string(chatId), "text": .string(text)]
        if !attachments.isEmpty { params["attachments"] = .array(attachments.map(JSONValue.string)) }
        return RemoteOp("send", params, feature: attachments.isEmpty ? nil : "attachments")
    }
    public static func interrupt(_ chatId: String) -> RemoteOp {
        RemoteOp("interrupt", ["chat_id": .string(chatId)])
    }
    public static func permission(_ chatId: String, request: String, option: String?) -> RemoteOp {
        RemoteOp("permission", [
            "chat_id": .string(chatId), "request_id": .string(request), "option_id": text(option),
        ])
    }
    public static func answer(_ chatId: String, request: String, answer: UiAnswer?) throws -> RemoteOp {
        RemoteOp("answer", [
            "chat_id": .string(chatId), "request_id": .string(request),
            "answer": try answer.map(JSONValue.of) ?? .null,
        ])
    }
    public static func create(folderId: String, agentId: String?, text: String) -> RemoteOp {
        RemoteOp("create", [
            "folder_id": .string(folderId), "agent_id": RemoteOp.text(agentId), "text": .string(text),
        ])
    }
    public static let forget = RemoteOp("forget")
    public static func options(_ chatId: String) -> RemoteOp {
        RemoteOp("options", ["chat_id": .string(chatId)], feature: "pickers")
    }
    public static func setModel(_ chatId: String, model: String) -> RemoteOp {
        RemoteOp("set_model", ["chat_id": .string(chatId), "model": .string(model)], feature: "pickers")
    }
    public static func setThinking(_ chatId: String, level: String) -> RemoteOp {
        RemoteOp("set_thinking", ["chat_id": .string(chatId), "level": .string(level)], feature: "pickers")
    }
    public static func switchAgent(_ chatId: String, agentId: String) -> RemoteOp {
        RemoteOp("switch_agent", ["chat_id": .string(chatId), "agent_id": .string(agentId)], feature: "pickers")
    }
    // The iPhone app's own.
    public static let inbox = RemoteOp("inbox", feature: "inbox")
    public static func search(_ query: String, limit: Int = 50) -> RemoteOp {
        RemoteOp("search", ["query": .string(query), "limit": .number(Double(limit))], feature: "search")
    }
}

/// How the computer looks: its theme and appearance, as Settings > Appearance has them.
public struct Look: Codable, Hashable, Sendable {
    public var theme: String
    /// "system", "light" or "dark".
    public var appearance: String
    public init(theme: String, appearance: String) {
        self.theme = theme
        self.appearance = appearance
    }
    public static let fallback = Look(theme: "claude", appearance: "system")
}

/// The computer's first sealed message of a session: it proved it holds the key.
public struct Ready: Codable, Hashable, Sendable {
    public var hostName: String
    public var paired: Bool
    /// What it offers beyond the first version's operations.
    public var features: [String]
    public var look: Look?
    /// The site its relay answers on, for an app whose site moved.
    public var site: String?

    enum CodingKeys: String, CodingKey {
        case paired, features, look, site
        case hostName = "host_name"
    }

    public init(hostName: String, paired: Bool, features: [String], look: Look?, site: String?) {
        self.hostName = hostName
        self.paired = paired
        self.features = features
        self.look = look
        self.site = site
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        hostName = try c.decodeIfPresent(String.self, forKey: .hostName) ?? ""
        paired = try c.decodeIfPresent(Bool.self, forKey: .paired) ?? false
        features = try c.decodeIfPresent([String].self, forKey: .features) ?? []
        // A look this copy cannot read is no look: the app keeps its own.
        look = try? c.decodeIfPresent(Look.self, forKey: .look)
        site = try c.decodeIfPresent(String.self, forKey: .site)
    }
}

/// A pairing became a device: its id and key, sent once inside the sealed session.
public struct Paired: Codable, Sendable {
    public var deviceId: String
    public var deviceKey: String
    public var features: [String]?
    enum CodingKeys: String, CodingKey {
        case features
        case deviceId = "device_id", deviceKey = "device_key"
    }
}

/// One thing a chat lets the person choose, as the app names it.
public struct Choice: Codable, Hashable, Sendable, Identifiable {
    public var value: String
    public var name: String
    public var id: String { value }
    public init(value: String, name: String) {
        self.value = value
        self.name = name
    }
}

/// A chat's models, thinking levels and access modes, and what it is set to.
public struct Choices: Codable, Hashable, Sendable {
    public var models: [Choice]
    public var model: String?
    public var thinking: [Choice]
    public var thinkingLevel: String?
    public var access: [Choice]?
    public var accessMode: String?
    enum CodingKeys: String, CodingKey {
        case models, model, thinking, access
        case thinkingLevel = "thinking_level", accessMode = "access_mode"
    }
    public init(
        models: [Choice], model: String?, thinking: [Choice], thinkingLevel: String?,
        access: [Choice]? = nil, accessMode: String? = nil
    ) {
        self.models = models
        self.model = model
        self.thinking = thinking
        self.thinkingLevel = thinkingLevel
        self.access = access
        self.accessMode = accessMode
    }
}

/// What `create` returns: the new chat, and its first message taken.
public struct Created: Codable, Sendable {
    public var chat: WorkspaceChat
    public var sent: PromptAck?
    /// The chat was made and its first message was not sent: why, in the computer's words.
    public var unsent: String?
}

/// What `switch_agent` returns: the chat the handoff continues in.
public struct Switched: Codable, Sendable {
    public var chatId: String
    enum CodingKeys: String, CodingKey { case chatId = "chat_id" }
}

/// An empty reply.
public struct Empty: Codable, Sendable {
    public init() {}
}

/// Everything the Inbox shows in one read (`inbox`): what asks, what works, what just ended.
public struct InboxSnapshot: Codable, Hashable, Sendable {
    public struct Request: Codable, Hashable, Sendable {
        public var id: String
        /// "permission" or "question".
        public var kind: String
        public var title: String
        public var options: [PermissionOption]?
        public var prompt: UiPrompt?
    }
    public struct Asking: Codable, Hashable, Sendable {
        public var chatId: String
        /// Nil when the chat's newest history shows no pending request yet.
        public var request: Request?
        enum CodingKeys: String, CodingKey {
            case request
            case chatId = "chat_id"
        }
    }
    public struct Working: Codable, Hashable, Sendable {
        public var chatId: String
        public var line: String?
        public var since: Int64?
        enum CodingKeys: String, CodingKey {
            case line, since
            case chatId = "chat_id"
        }
    }
    public struct Recent: Codable, Hashable, Sendable {
        public var chatId: String
        public var endedAt: Int64?
        public var durationMs: Int64?
        public var line: String?
        enum CodingKeys: String, CodingKey {
            case line
            case chatId = "chat_id", endedAt = "ended_at", durationMs = "duration_ms"
        }
    }
    public var asking: [Asking]
    public var working: [Working]
    public var recent: [Recent]

    public init(asking: [Asking] = [], working: [Working] = [], recent: [Recent] = []) {
        self.asking = asking
        self.working = working
        self.recent = recent
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: AnyKey.self)
        asking = (try? c.maybe("asking", [Asking].self)) ?? []
        working = (try? c.maybe("working", [Working].self)) ?? []
        recent = (try? c.maybe("recent", [Recent].self)) ?? []
    }
}

/// What `search` found.
public struct SearchHits: Codable, Hashable, Sendable {
    public struct Hit: Codable, Hashable, Sendable {
        public var chatId: String
        public var title: String?
        public var snippet: String?
        public var at: Int64?
        enum CodingKeys: String, CodingKey {
            case title, snippet, at
            case chatId = "chat_id"
        }
    }
    public var hits: [Hit]
}

/// A request the computer refused, in its own words.
public struct RemoteRefusal: Codable, Hashable, Sendable, Error {
    public var code: String
    public var message: String
    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }
}

/// A sealed message from the computer, read just far enough to route it.
public enum HostMessage: Sendable {
    /// The answer to request `re`: its data as JSON, or a refusal.
    case reply(re: UInt32, result: Result<Data, RemoteRefusal>)
    case ready(Ready)
    case chat(ChatStatusUpdate)
    case frame(EventBatch)
    /// The app fell behind on a chat's live events: open it again.
    case lost(chatId: String)
    /// Another workspace opened on the computer: start over from the overview.
    case reset
    /// An event this copy does not know.
    case other(String)

    struct Head: Decodable {
        let re: UInt32?
        let ok: Bool?
        let ev: String?
        let error: RemoteRefusal?
    }

    struct DataPart: Decodable { let data: JSONValue? }
    struct UpdatePart: Decodable { let update: ChatStatusUpdate }
    struct BatchPart: Decodable { let batch: EventBatch }
    struct LostPart: Decodable {
        let chatId: String
        enum CodingKeys: String, CodingKey { case chatId = "chat_id" }
    }

    /// Reads an opened message. A reply's data stays raw JSON until its caller types it.
    public static func parse(_ text: String) throws -> HostMessage {
        let bytes = Data(text.utf8)
        let decoder = JSONDecoder()
        let head = try decoder.decode(Head.self, from: bytes)
        if let re = head.re {
            if head.ok == true {
                let data = try decoder.decode(DataPart.self, from: bytes).data ?? .object([:])
                return .reply(re: re, result: .success(try JSONEncoder().encode(data)))
            }
            let refusal = head.error ?? RemoteRefusal(code: "failed", message: "The computer refused.")
            return .reply(re: re, result: .failure(refusal))
        }
        switch head.ev {
        case "ready": return .ready(try decoder.decode(Ready.self, from: bytes))
        case "chat": return .chat(try decoder.decode(UpdatePart.self, from: bytes).update)
        case "frame": return .frame(try decoder.decode(BatchPart.self, from: bytes).batch)
        case "lost": return .lost(chatId: try decoder.decode(LostPart.self, from: bytes).chatId)
        case "reset": return .reset
        default: return .other(head.ev ?? "")
        }
    }
}
