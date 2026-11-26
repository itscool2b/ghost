public import Foundation

// The desktop's chat events (`GhostEvent` in bindings.ts), as a followed chat's frames and its
// history carry them. Field names on the wire are snake_case; every enum reads values it does
// not know as its fallback, and an event type it does not know as `.unknown`.

public enum ToolKind: String, LenientEnum {
    case read, edit, delete, move, search, execute, think, fetch
    case switchMode = "switch_mode"
    case other
    public static let fallback = ToolKind.other
}

public enum ToolStatus: String, LenientEnum {
    case pending
    case inProgress = "in_progress"
    case completed, failed
    public static let fallback = ToolStatus.pending
}

/// Output was cut short: how much there was, and the blob that holds all of it.
public struct Truncation: Codable, Hashable, Sendable {
    public var totalBytes: Int64
    public var blob: String?
    enum CodingKeys: String, CodingKey {
        case blob
        case totalBytes = "total_bytes"
    }
    public init(totalBytes: Int64, blob: String?) {
        self.totalBytes = totalBytes
        self.blob = blob
    }
}

public struct ToolLocation: Codable, Hashable, Sendable {
    public var path: String
    public var line: Int?
    public init(path: String, line: Int?) {
        self.path = path
        self.line = line
    }
}

/// What a tool showed: text, a file's change, or a terminal's output.
public enum ToolContent: Codable, Hashable, Sendable {
    case text(text: String, truncated: Truncation?)
    case diff(path: String, oldText: String?, newText: String, truncated: Truncation?)
    case terminal(output: String, exitCode: Int?, truncated: Truncation?)
    case unknown(JSONValue)

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: AnyKey.self)
        switch try c.maybe("type", String.self) {
        case "text":
            self = .text(text: try c.need("text"), truncated: try c.maybe("truncated"))
        case "diff":
            self = .diff(
                path: try c.need("path"), oldText: try c.maybe("old_text"),
                newText: try c.need("new_text"), truncated: try c.maybe("truncated"))
        case "terminal":
            self = .terminal(
                output: try c.need("output"), exitCode: try c.maybe("exit_code"),
                truncated: try c.maybe("truncated"))
        default:
            self = .unknown(try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        if case .unknown(let raw) = self { return try raw.encode(to: encoder) }
        var c = encoder.container(keyedBy: AnyKey.self)
        switch self {
        case .text(let text, let truncated):
            try c.put("type", "text")
            try c.put("text", text)
            try c.put("truncated", truncated)
        case .diff(let path, let oldText, let newText, let truncated):
            try c.put("type", "diff")
            try c.put("path", path)
            try c.put("old_text", oldText)
            try c.put("new_text", newText)
            try c.put("truncated", truncated)
        case .terminal(let output, let exitCode, let truncated):
            try c.put("type", "terminal")
            try c.put("output", output)
            try c.put("exit_code", exitCode)
            try c.put("truncated", truncated)
        case .unknown: break
        }
    }
}

/// A tool call as first announced (`tool_call`).
public struct ToolCall: Codable, Hashable, Sendable {
    public var toolCallId: String
    public var kind: ToolKind
    public var title: String
    public var status: ToolStatus
    public var locations: [ToolLocation]
    public var rawInput: JSONValue?
    public var toolName: String?
    public var content: [ToolContent]
    public var truncated: Truncation?

    enum CodingKeys: String, CodingKey {
        case kind, title, status, locations, content, truncated
        case toolCallId = "tool_call_id", rawInput = "raw_input", toolName = "tool_name"
    }

    public init(
        toolCallId: String, kind: ToolKind, title: String, status: ToolStatus,
        locations: [ToolLocation] = [], rawInput: JSONValue? = nil, toolName: String? = nil,
        content: [ToolContent] = [], truncated: Truncation? = nil
    ) {
        self.toolCallId = toolCallId
        self.kind = kind
        self.title = title
        self.status = status
        self.locations = locations
        self.rawInput = rawInput
        self.toolName = toolName
        self.content = content
        self.truncated = truncated
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        toolCallId = try c.decode(String.self, forKey: .toolCallId)
        kind = try c.decodeIfPresent(ToolKind.self, forKey: .kind) ?? .other
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        status = try c.decodeIfPresent(ToolStatus.self, forKey: .status) ?? .pending
        locations = try c.decodeIfPresent([ToolLocation].self, forKey: .locations) ?? []
        rawInput = try c.decodeIfPresent(JSONValue.self, forKey: .rawInput)
        toolName = try c.decodeIfPresent(String.self, forKey: .toolName)
        content = try c.decodeIfPresent([ToolContent].self, forKey: .content) ?? []
        truncated = try c.decodeIfPresent(Truncation.self, forKey: .truncated)
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("tool_call_id", toolCallId)
        try c.put("kind", kind)
        try c.put("title", title)
        try c.put("status", status)
        try c.put("locations", locations)
        try c.put("raw_input", rawInput)
        try c.put("tool_name", toolName)
        try c.put("content", content)
        try c.put("truncated", truncated)
    }
}

/// A later change to a tool call (`tool_update`). A nil field keeps the old value; an empty
/// `content` clears it.
public struct ToolUpdate: Codable, Hashable, Sendable {
    public var toolCallId: String
    public var status: ToolStatus?
    public var title: String?
    public var kind: ToolKind?
    public var locations: [ToolLocation]?
    public var rawInput: JSONValue?
    public var toolName: String?
    public var content: [ToolContent]?
    public var truncated: Truncation?

    enum CodingKeys: String, CodingKey {
        case status, title, kind, locations, content, truncated
        case toolCallId = "tool_call_id", rawInput = "raw_input", toolName = "tool_name"
    }

    public init(
        toolCallId: String, status: ToolStatus? = nil, title: String? = nil,
        kind: ToolKind? = nil, locations: [ToolLocation]? = nil, rawInput: JSONValue? = nil,
        toolName: String? = nil, content: [ToolContent]? = nil, truncated: Truncation? = nil
    ) {
        self.toolCallId = toolCallId
        self.status = status
        self.title = title
        self.kind = kind
        self.locations = locations
        self.rawInput = rawInput
        self.toolName = toolName
        self.content = content
        self.truncated = truncated
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("tool_call_id", toolCallId)
        try c.put("status", status)
        try c.put("title", title)
        try c.put("kind", kind)
        try c.put("locations", locations)
        try c.put("raw_input", rawInput)
        try c.put("tool_name", toolName)
        try c.put("content", content)
        try c.put("truncated", truncated)
    }
}

public enum PermissionOptionKind: String, LenientEnum {
    case allowOnce = "allow_once"
    case allowAlways = "allow_always"
    case rejectOnce = "reject_once"
    case rejectAlways = "reject_always"
    case other
    public static let fallback = PermissionOptionKind.other

    /// Allow choices are the prominent ones.
    public var allows: Bool { self == .allowOnce || self == .allowAlways }
}

public struct PermissionOption: Codable, Hashable, Sendable, Identifiable {
    public var optionId: String
    public var kind: PermissionOptionKind
    public var label: String
    public var id: String { optionId }
    enum CodingKeys: String, CodingKey {
        case kind, label
        case optionId = "option_id"
    }
    public init(optionId: String, kind: PermissionOptionKind, label: String) {
        self.optionId = optionId
        self.kind = kind
        self.label = label
    }
}

/// The agent asks before running a tool (`permission_requested`).
public struct PermissionRequest: Codable, Hashable, Sendable {
    public var requestId: String
    public var toolCallId: String?
    public var title: String
    public var kind: ToolKind?
    public var rawInput: JSONValue?
    public var content: [ToolContent]
    public var locations: [ToolLocation]
    public var options: [PermissionOption]
    public var truncated: Truncation?

    enum CodingKeys: String, CodingKey {
        case title, kind, content, locations, options, truncated
        case requestId = "request_id", toolCallId = "tool_call_id", rawInput = "raw_input"
    }

    public init(
        requestId: String, toolCallId: String? = nil, title: String, kind: ToolKind? = nil,
        rawInput: JSONValue? = nil, content: [ToolContent] = [], locations: [ToolLocation] = [],
        options: [PermissionOption], truncated: Truncation? = nil
    ) {
        self.requestId = requestId
        self.toolCallId = toolCallId
        self.title = title
        self.kind = kind
        self.rawInput = rawInput
        self.content = content
        self.locations = locations
        self.options = options
        self.truncated = truncated
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        requestId = try c.decode(String.self, forKey: .requestId)
        toolCallId = try c.decodeIfPresent(String.self, forKey: .toolCallId)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        kind = try c.decodeIfPresent(ToolKind.self, forKey: .kind)
        rawInput = try c.decodeIfPresent(JSONValue.self, forKey: .rawInput)
        content = try c.decodeIfPresent([ToolContent].self, forKey: .content) ?? []
        locations = try c.decodeIfPresent([ToolLocation].self, forKey: .locations) ?? []
        options = try c.decodeIfPresent([PermissionOption].self, forKey: .options) ?? []
        truncated = try c.decodeIfPresent(Truncation.self, forKey: .truncated)
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("request_id", requestId)
        try c.put("tool_call_id", toolCallId)
        try c.put("title", title)
        try c.put("kind", kind)
        try c.put("raw_input", rawInput)
        try c.put("content", content)
        try c.put("locations", locations)
        try c.put("options", options)
        try c.put("truncated", truncated)
    }
}

/// How a permission request ended: an option chosen, or cancelled.
public enum PermissionDecision: Codable, Hashable, Sendable {
    case selected(optionId: String, plan: String?)
    case cancelled

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: AnyKey.self)
        if try c.maybe("type", String.self) == "selected" {
            self = .selected(optionId: try c.need("option_id"), plan: try c.maybe("plan"))
        } else {
            self = .cancelled
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        switch self {
        case .selected(let optionId, let plan):
            try c.put("type", "selected")
            try c.put("option_id", optionId)
            if let plan { try c.put("plan", plan) }
        case .cancelled:
            try c.put("type", "cancelled")
        }
    }
}

public enum ResolvedBy: String, LenientEnum {
    case user, auto, agent, turn, system, other
    public static let fallback = ResolvedBy.other
}

public struct UiQuestionOption: Codable, Hashable, Sendable {
    public var label: String
    public var description: String
    public init(label: String, description: String) {
        self.label = label
        self.description = description
    }
}

public struct UiQuestion: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var header: String
    public var question: String
    public var options: [UiQuestionOption]
    public var multiSelect: Bool
    public var allowFreeText: Bool
    enum CodingKeys: String, CodingKey {
        case id, header, question, options
        case multiSelect = "multi_select", allowFreeText = "allow_free_text"
    }
    public init(
        id: String, header: String, question: String, options: [UiQuestionOption],
        multiSelect: Bool, allowFreeText: Bool
    ) {
        self.id = id
        self.header = header
        self.question = question
        self.options = options
        self.multiSelect = multiSelect
        self.allowFreeText = allowFreeText
    }
}

/// What an agent or an MCP server asks the person (`UiPrompt`).
public enum UiPrompt: Codable, Hashable, Sendable {
    case confirm(message: String)
    case select(message: String, options: [String])
    case input(message: String, placeholder: String?)
    case questions(message: String, questions: [UiQuestion])
    case notify(message: String)
    /// A form or a sign-in: answered on the computer. Their fields are kept as they came.
    case form(message: String, source: String?, fields: JSONValue)
    case link(message: String, source: String?, url: String)
    case unknown(JSONValue)

    public var message: String {
        switch self {
        case .confirm(let message), .select(let message, _), .input(let message, _),
            .questions(let message, _), .notify(let message), .form(let message, _, _),
            .link(let message, _, _):
            return message
        case .unknown(let raw): return raw["message"]?.string ?? ""
        }
    }

    public var type: String {
        switch self {
        case .confirm: "confirm"
        case .select: "select"
        case .input: "input"
        case .questions: "questions"
        case .notify: "notify"
        case .form: "form"
        case .link: "link"
        case .unknown(let raw): raw["type"]?.string ?? "unknown"
        }
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: AnyKey.self)
        let message = try c.maybe("message", String.self) ?? ""
        switch try c.maybe("type", String.self) {
        case "confirm": self = .confirm(message: message)
        case "select": self = .select(message: message, options: try c.maybe("options") ?? [])
        case "input": self = .input(message: message, placeholder: try c.maybe("placeholder"))
        case "questions":
            self = .questions(message: message, questions: try c.maybe("questions") ?? [])
        case "notify": self = .notify(message: message)
        case "form":
            self = .form(
                message: message, source: try c.maybe("source"),
                fields: try c.maybe("fields") ?? .array([]))
        case "link":
            self = .link(message: message, source: try c.maybe("source"), url: try c.need("url"))
        default: self = .unknown(try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        if case .unknown(let raw) = self { return try raw.encode(to: encoder) }
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("type", type)
        try c.put("message", message)
        switch self {
        case .select(_, let options): try c.put("options", options)
        case .input(_, let placeholder): try c.put("placeholder", placeholder)
        case .questions(_, let questions): try c.put("questions", questions)
        case .form(_, let source, let fields):
            try c.put("source", source)
            try c.put("fields", fields)
        case .link(_, let source, let url):
            try c.put("source", source)
            try c.put("url", url)
        default: break
        }
    }
}

public struct UiRequest: Codable, Hashable, Sendable {
    public var requestId: String
    public var prompt: UiPrompt
    enum CodingKeys: String, CodingKey {
        case prompt
        case requestId = "request_id"
    }
    public init(requestId: String, prompt: UiPrompt) {
        self.requestId = requestId
        self.prompt = prompt
    }
}

public struct UiQuestionAnswer: Codable, Hashable, Sendable {
    public var questionId: String
    /// Indices into the question's options, in their shown order.
    public var selected: [Int]
    public var text: String?
    enum CodingKeys: String, CodingKey {
        case selected, text
        case questionId = "question_id"
    }
    public init(questionId: String, selected: [Int], text: String?) {
        self.questionId = questionId
        self.selected = selected
        self.text = text
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("question_id", questionId)
        try c.put("selected", selected)
        try c.put("text", text)
    }
}

/// The person's answer to a [`UiPrompt`].
public enum UiAnswer: Codable, Hashable, Sendable {
    case confirm(yes: Bool)
    case select(index: Int)
    case input(text: String)
    case questions(answers: [UiQuestionAnswer])
    case form(accept: Bool, values: JSONValue)
    case unknown(JSONValue)

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: AnyKey.self)
        switch try c.maybe("type", String.self) {
        case "confirm": self = .confirm(yes: try c.need("yes"))
        case "select": self = .select(index: try c.need("index"))
        case "input": self = .input(text: try c.need("text"))
        case "questions": self = .questions(answers: try c.need("answers"))
        case "form":
            self = .form(accept: try c.need("accept"), values: try c.maybe("values") ?? .array([]))
        default: self = .unknown(try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        if case .unknown(let raw) = self { return try raw.encode(to: encoder) }
        var c = encoder.container(keyedBy: AnyKey.self)
        switch self {
        case .confirm(let yes):
            try c.put("type", "confirm")
            try c.put("yes", yes)
        case .select(let index):
            try c.put("type", "select")
            try c.put("index", index)
        case .input(let text):
            try c.put("type", "input")
            try c.put("text", text)
        case .questions(let answers):
            try c.put("type", "questions")
            try c.put("answers", answers)
        case .form(let accept, let values):
            try c.put("type", "form")
            try c.put("accept", accept)
            try c.put("values", values)
        case .unknown: break
        }
    }
}

public enum PlanStatus: String, LenientEnum {
    case pending
    case inProgress = "in_progress"
    case completed
    public static let fallback = PlanStatus.pending
}

public enum PlanPriority: String, LenientEnum {
    case high, medium, low
    public static let fallback = PlanPriority.medium
}

public struct PlanEntry: Codable, Hashable, Sendable {
    public var content: String
    public var status: PlanStatus
    public var priority: PlanPriority?
    public init(content: String, status: PlanStatus, priority: PlanPriority?) {
        self.content = content
        self.status = status
        self.priority = priority
    }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("content", content)
        try c.put("status", status)
        try c.put("priority", priority)
    }
}

public enum TaskKind: String, LenientEnum {
    case agent, command, other
    public static let fallback = TaskKind.other
}

public enum TaskStatus: String, LenientEnum {
    case running, completed, failed, stopped
    public static let fallback = TaskStatus.stopped
}

public enum TaskStepKind: String, LenientEnum {
    case tool, text
    public static let fallback = TaskStepKind.text
}

/// A background task: a subagent or a background command.
public struct BackgroundTask: Codable, Hashable, Sendable {
    public var taskId: String
    public var kind: TaskKind
    public var title: String
    public var agentType: String?
    public var toolCallId: String?
    public var parentTaskId: String?
    public var backgrounded: Bool
    public var status: TaskStatus
    public var activity: String?
    public var toolUses: Int?
    public var tokens: Int64?
    public var durationMs: Int64?
    public var startedAt: Int64?
    public var endedAt: Int64?
    public var prompt: String?
    public var summary: String?
    public var summaryTruncated: Truncation?

    enum CodingKeys: String, CodingKey {
        case kind, title, backgrounded, status, activity, tokens, prompt, summary
        case taskId = "task_id", agentType = "agent_type", toolCallId = "tool_call_id"
        case parentTaskId = "parent_task_id", toolUses = "tool_uses", durationMs = "duration_ms"
        case startedAt = "started_at", endedAt = "ended_at", summaryTruncated = "summary_truncated"
    }

    public init(
        taskId: String, kind: TaskKind, title: String, agentType: String? = nil,
        toolCallId: String? = nil, parentTaskId: String? = nil, backgrounded: Bool = false,
        status: TaskStatus, activity: String? = nil, toolUses: Int? = nil, tokens: Int64? = nil,
        durationMs: Int64? = nil, startedAt: Int64? = nil, endedAt: Int64? = nil,
        prompt: String? = nil, summary: String? = nil, summaryTruncated: Truncation? = nil
    ) {
        self.taskId = taskId
        self.kind = kind
        self.title = title
        self.agentType = agentType
        self.toolCallId = toolCallId
        self.parentTaskId = parentTaskId
        self.backgrounded = backgrounded
        self.status = status
        self.activity = activity
        self.toolUses = toolUses
        self.tokens = tokens
        self.durationMs = durationMs
        self.startedAt = startedAt
        self.endedAt = endedAt
        self.prompt = prompt
        self.summary = summary
        self.summaryTruncated = summaryTruncated
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        taskId = try c.decode(String.self, forKey: .taskId)
        kind = try c.decodeIfPresent(TaskKind.self, forKey: .kind) ?? .other
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        agentType = try c.decodeIfPresent(String.self, forKey: .agentType)
        toolCallId = try c.decodeIfPresent(String.self, forKey: .toolCallId)
        parentTaskId = try c.decodeIfPresent(String.self, forKey: .parentTaskId)
        backgrounded = try c.decodeIfPresent(Bool.self, forKey: .backgrounded) ?? false
        status = try c.decodeIfPresent(TaskStatus.self, forKey: .status) ?? .stopped
        activity = try c.decodeIfPresent(String.self, forKey: .activity)
        toolUses = try c.decodeIfPresent(Int.self, forKey: .toolUses)
        tokens = try c.decodeIfPresent(Int64.self, forKey: .tokens)
        durationMs = try c.decodeIfPresent(Int64.self, forKey: .durationMs)
        startedAt = try c.decodeIfPresent(Int64.self, forKey: .startedAt)
        endedAt = try c.decodeIfPresent(Int64.self, forKey: .endedAt)
        prompt = try c.decodeIfPresent(String.self, forKey: .prompt)
        summary = try c.decodeIfPresent(String.self, forKey: .summary)
        summaryTruncated = try c.decodeIfPresent(Truncation.self, forKey: .summaryTruncated)
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("task_id", taskId)
        try c.put("kind", kind)
        try c.put("title", title)
        try c.put("agent_type", agentType)
        try c.put("tool_call_id", toolCallId)
        try c.put("parent_task_id", parentTaskId)
        try c.put("backgrounded", backgrounded)
        try c.put("status", status)
        try c.put("activity", activity)
        try c.put("tool_uses", toolUses)
        try c.put("tokens", tokens)
        try c.put("duration_ms", durationMs)
        try c.put("started_at", startedAt)
        try c.put("ended_at", endedAt)
        try c.put("prompt", prompt)
        try c.put("summary", summary)
        try c.put("summary_truncated", summaryTruncated)
    }
}

public struct TaskStep: Codable, Hashable, Sendable {
    public var stepId: String
    public var kind: TaskStepKind
    public var title: String
    public var toolKind: ToolKind?
    public var status: ToolStatus?
    public var text: String?
    public var truncated: Truncation?

    enum CodingKeys: String, CodingKey {
        case kind, title, status, text, truncated
        case stepId = "step_id", toolKind = "tool_kind"
    }

    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("step_id", stepId)
        try c.put("kind", kind)
        try c.put("title", title)
        try c.put("tool_kind", toolKind)
        try c.put("status", status)
        try c.put("text", text)
        try c.put("truncated", truncated)
    }
}

public struct Cost: Codable, Hashable, Sendable {
    public var amount: Double?
    public var currency: String
    public init(amount: Double?, currency: String) {
        self.amount = amount
        self.currency = currency
    }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("amount", amount)
        try c.put("currency", currency)
    }
}

/// Context-window and spend information.
public struct UsageInfo: Codable, Hashable, Sendable {
    public var contextUsed: Int64?
    public var contextSize: Int64?
    public var cost: Cost?
    enum CodingKeys: String, CodingKey {
        case cost
        case contextUsed = "context_used", contextSize = "context_size"
    }
    public init(contextUsed: Int64?, contextSize: Int64?, cost: Cost?) {
        self.contextUsed = contextUsed
        self.contextSize = contextSize
        self.cost = cost
    }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("context_used", contextUsed)
        try c.put("context_size", contextSize)
        try c.put("cost", cost)
    }
}

public enum UsageScope: String, LenientEnum {
    case turn, session
    public static let fallback = UsageScope.session
}

public struct TokenUsage: Codable, Hashable, Sendable {
    public var inputTokens: Int64
    public var outputTokens: Int64
    public var cacheReadTokens: Int64?
    public var cacheWriteTokens: Int64?
    public var reasoningTokens: Int64?
    public var scope: UsageScope
    enum CodingKeys: String, CodingKey {
        case scope
        case inputTokens = "input_tokens", outputTokens = "output_tokens"
        case cacheReadTokens = "cache_read_tokens", cacheWriteTokens = "cache_write_tokens"
        case reasoningTokens = "reasoning_tokens"
    }
    public func encode(to encoder: any Encoder) throws {
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("input_tokens", inputTokens)
        try c.put("output_tokens", outputTokens)
        try c.put("cache_read_tokens", cacheReadTokens)
        try c.put("cache_write_tokens", cacheWriteTokens)
        try c.put("reasoning_tokens", reasoningTokens)
        try c.put("scope", scope)
    }
}

public enum StopReason: String, LenientEnum {
    case endTurn = "end_turn"
    case maxTokens = "max_tokens"
    case maxTurnRequests = "max_turn_requests"
    case refusal, cancelled, interrupted, error, other
    public static let fallback = StopReason.other
}

public struct LimitWindow: Codable, Hashable, Sendable {
    public var label: String
    public var usedPercent: Double?
    public var resetsAt: Int64?
    enum CodingKeys: String, CodingKey {
        case label
        case usedPercent = "used_percent", resetsAt = "resets_at"
    }
    public init(label: String, usedPercent: Double?, resetsAt: Int64?) {
        self.label = label
        self.usedPercent = usedPercent
        self.resetsAt = resetsAt
    }
}

public enum SteeringState: String, LenientEnum {
    case pending, accepted, rejected, unconfirmed
    public static let fallback = SteeringState.unconfirmed
}

public enum PromptState: String, LenientEnum {
    case queued, sending, started, completed, cancelled, failed
    public static let fallback = PromptState.failed
}

public enum NoticeLevel: String, LenientEnum {
    case info, warning, error
    public static let fallback = NoticeLevel.info
}

public enum ErrorKind: String, LenientEnum {
    case agentUnavailable = "agent_unavailable"
    case folderMissing = "folder_missing"
    case folderNotAllowed = "folder_not_allowed"
    case spawnFailed = "spawn_failed"
    case startTimeout = "start_timeout"
    case protocolMismatch = "protocol_mismatch"
    case authRequired = "auth_required"
    case usageLimit = "usage_limit"
    case agentError = "agent_error"
    case agentExited = "agent_exited"
    case protocolError = "protocol_error"
    case storage, `internal`, other
    public static let fallback = ErrorKind.other
}

public enum Autonomy: String, LenientEnum {
    case fullAuto = "full_auto"
    case autoEdit = "auto_edit"
    case ask
    public static let fallback = Autonomy.ask
}

/// A chat's live header state (`SessionInfo`): what the agent is, its model and mode.
public struct SessionInfo: Codable, Hashable, Sendable {
    public var status: SessionStatus
    public var detail: String?
    public var nativeSessionId: String?
    public var agentName: String?
    public var agentVersion: String?
    public var title: String?
    public var model: String?
    public var effort: String?
    public var mode: String?
    public var autonomy: Autonomy
    public var queueHeld: Bool
    public var revision: Int64
    /// Why a queued message waits, kept as it came.
    public var queueWait: JSONValue?
    public var sending: JSONValue?
    public var accountId: String?
    public var usageLimit: JSONValue?
    public var worktreeSetup: JSONValue?

    enum CodingKeys: String, CodingKey {
        case status, detail, title, model, effort, mode, autonomy, revision, sending
        case nativeSessionId = "native_session_id", agentName = "agent_name"
        case agentVersion = "agent_version", queueHeld = "queue_held", queueWait = "queue_wait"
        case accountId = "account_id", usageLimit = "usage_limit", worktreeSetup = "worktree_setup"
    }

    public init(status: SessionStatus, autonomy: Autonomy = .ask) {
        self.status = status
        self.autonomy = autonomy
        queueHeld = false
        revision = 0
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        status = try c.decodeIfPresent(SessionStatus.self, forKey: .status) ?? .stopped
        detail = try c.decodeIfPresent(String.self, forKey: .detail)
        nativeSessionId = try c.decodeIfPresent(String.self, forKey: .nativeSessionId)
        agentName = try c.decodeIfPresent(String.self, forKey: .agentName)
        agentVersion = try c.decodeIfPresent(String.self, forKey: .agentVersion)
        title = try c.decodeIfPresent(String.self, forKey: .title)
        model = try c.decodeIfPresent(String.self, forKey: .model)
        effort = try c.decodeIfPresent(String.self, forKey: .effort)
        mode = try c.decodeIfPresent(String.self, forKey: .mode)
        autonomy = try c.decodeIfPresent(Autonomy.self, forKey: .autonomy) ?? .ask
        queueHeld = try c.decodeIfPresent(Bool.self, forKey: .queueHeld) ?? false
        revision = try c.decodeIfPresent(Int64.self, forKey: .revision) ?? 0
        queueWait = try c.decodeIfPresent(JSONValue.self, forKey: .queueWait)
        sending = try c.decodeIfPresent(JSONValue.self, forKey: .sending)
        accountId = try c.decodeIfPresent(String.self, forKey: .accountId)
        usageLimit = try c.decodeIfPresent(JSONValue.self, forKey: .usageLimit)
        worktreeSetup = try c.decodeIfPresent(JSONValue.self, forKey: .worktreeSetup)
    }
}

/// One event of a chat (`GhostEvent`).
public enum GhostEvent: Codable, Hashable, Sendable {
    case turnStarted(turnId: String, promptId: String?)
    case turnEnded(
        turnId: String, stopReason: StopReason, usage: TokenUsage?, durationMs: Int64?,
        recovered: Bool)
    case userMessageDelta(messageId: String, text: String)
    case steeringStatus(messageId: String, state: SteeringState, detail: String?)
    case messageDelta(messageId: String, text: String)
    case messageCompleted(messageId: String)
    case thoughtDelta(thoughtId: String, text: String)
    case thoughtCompleted(thoughtId: String, durationMs: Int64?)
    case toolCall(ToolCall)
    case toolUpdate(ToolUpdate)
    case permissionRequested(PermissionRequest)
    case permissionResolved(requestId: String, decision: PermissionDecision, by: ResolvedBy)
    case uiRequest(UiRequest)
    case uiResolved(requestId: String, answer: UiAnswer?, by: ResolvedBy)
    case planUpdated(entries: [PlanEntry])
    case taskUpdated(BackgroundTask)
    case taskStep(taskId: String, step: TaskStep)
    case usageUpdated(UsageInfo)
    case limitsUpdated(windows: [LimitWindow])
    case sessionUpdated(SessionInfo)
    case promptLifecycle(promptId: String, state: PromptState, preview: String?, held: Bool?)
    case notice(level: NoticeLevel, message: String)
    case error(kind: ErrorKind, message: String, detail: String?, recoverable: Bool)
    /// An event type a newer desktop sends that this copy does not know: skipped.
    case unknown(type: String, raw: JSONValue)

    /// The wire name of the event.
    public var type: String {
        switch self {
        case .turnStarted: "turn_started"
        case .turnEnded: "turn_ended"
        case .userMessageDelta: "user_message_delta"
        case .steeringStatus: "steering_status"
        case .messageDelta: "message_delta"
        case .messageCompleted: "message_completed"
        case .thoughtDelta: "thought_delta"
        case .thoughtCompleted: "thought_completed"
        case .toolCall: "tool_call"
        case .toolUpdate: "tool_update"
        case .permissionRequested: "permission_requested"
        case .permissionResolved: "permission_resolved"
        case .uiRequest: "ui_request"
        case .uiResolved: "ui_resolved"
        case .planUpdated: "plan_updated"
        case .taskUpdated: "task_updated"
        case .taskStep: "task_step"
        case .usageUpdated: "usage_updated"
        case .limitsUpdated: "limits_updated"
        case .sessionUpdated: "session_updated"
        case .promptLifecycle: "prompt_lifecycle"
        case .notice: "notice"
        case .error: "error"
        case .unknown(let type, _): type
        }
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: AnyKey.self)
        let type = try c.maybe("type", String.self) ?? ""
        switch type {
        case "turn_started":
            self = .turnStarted(turnId: try c.need("turn_id"), promptId: try c.maybe("prompt_id"))
        case "turn_ended":
            self = .turnEnded(
                turnId: try c.need("turn_id"),
                stopReason: try c.maybe("stop_reason") ?? .other,
                usage: try c.maybe("usage"), durationMs: try c.maybe("duration_ms"),
                recovered: try c.maybe("recovered") ?? false)
        case "user_message_delta":
            self = .userMessageDelta(messageId: try c.need("message_id"), text: try c.need("text"))
        case "steering_status":
            self = .steeringStatus(
                messageId: try c.need("message_id"),
                state: try c.maybe("state") ?? .unconfirmed, detail: try c.maybe("detail"))
        case "message_delta":
            self = .messageDelta(messageId: try c.need("message_id"), text: try c.need("text"))
        case "message_completed":
            self = .messageCompleted(messageId: try c.need("message_id"))
        case "thought_delta":
            self = .thoughtDelta(thoughtId: try c.need("thought_id"), text: try c.need("text"))
        case "thought_completed":
            self = .thoughtCompleted(
                thoughtId: try c.need("thought_id"), durationMs: try c.maybe("duration_ms"))
        case "tool_call": self = .toolCall(try ToolCall(from: decoder))
        case "tool_update": self = .toolUpdate(try ToolUpdate(from: decoder))
        case "permission_requested":
            self = .permissionRequested(try PermissionRequest(from: decoder))
        case "permission_resolved":
            self = .permissionResolved(
                requestId: try c.need("request_id"),
                decision: try c.maybe("decision") ?? .cancelled,
                by: try c.maybe("by") ?? .other)
        case "ui_request": self = .uiRequest(try UiRequest(from: decoder))
        case "ui_resolved":
            self = .uiResolved(
                requestId: try c.need("request_id"), answer: try c.maybe("answer"),
                by: try c.maybe("by") ?? .other)
        case "plan_updated": self = .planUpdated(entries: try c.maybe("entries") ?? [])
        case "task_updated": self = .taskUpdated(try BackgroundTask(from: decoder))
        case "task_step":
            self = .taskStep(taskId: try c.need("task_id"), step: try c.need("step"))
        case "usage_updated": self = .usageUpdated(try UsageInfo(from: decoder))
        case "limits_updated": self = .limitsUpdated(windows: try c.maybe("windows") ?? [])
        case "session_updated": self = .sessionUpdated(try SessionInfo(from: decoder))
        case "prompt_lifecycle":
            self = .promptLifecycle(
                promptId: try c.need("prompt_id"), state: try c.maybe("state") ?? .failed,
                preview: try c.maybe("preview"), held: try c.maybe("held"))
        case "notice":
            self = .notice(level: try c.maybe("level") ?? .info, message: try c.need("message"))
        case "error":
            self = .error(
                kind: try c.maybe("kind") ?? .other, message: try c.maybe("message") ?? "",
                detail: try c.maybe("detail"), recoverable: try c.maybe("recoverable") ?? false)
        default:
            self = .unknown(type: type, raw: try JSONValue(from: decoder))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        if case .unknown(_, let raw) = self { return try raw.encode(to: encoder) }
        switch self {
        case .toolCall(let call): try call.encode(to: encoder)
        case .toolUpdate(let update): try update.encode(to: encoder)
        case .permissionRequested(let request): try request.encode(to: encoder)
        case .uiRequest(let request):
            var c = encoder.container(keyedBy: AnyKey.self)
            try c.put("request_id", request.requestId)
            try c.put("prompt", request.prompt)
        case .taskUpdated(let task): try task.encode(to: encoder)
        case .usageUpdated(let usage): try usage.encode(to: encoder)
        case .sessionUpdated(let info): try info.encode(to: encoder)
        default: break
        }
        var c = encoder.container(keyedBy: AnyKey.self)
        try c.put("type", type)
        switch self {
        case .turnStarted(let turnId, let promptId):
            try c.put("turn_id", turnId)
            try c.put("prompt_id", promptId)
        case .turnEnded(let turnId, let stopReason, let usage, let durationMs, let recovered):
            try c.put("turn_id", turnId)
            try c.put("stop_reason", stopReason)
            try c.put("usage", usage)
            try c.put("duration_ms", durationMs)
            try c.put("recovered", recovered)
        case .userMessageDelta(let messageId, let text), .messageDelta(let messageId, let text):
            try c.put("message_id", messageId)
            try c.put("text", text)
        case .steeringStatus(let messageId, let state, let detail):
            try c.put("message_id", messageId)
            try c.put("state", state)
            try c.put("detail", detail)
        case .messageCompleted(let messageId):
            try c.put("message_id", messageId)
        case .thoughtDelta(let thoughtId, let text):
            try c.put("thought_id", thoughtId)
            try c.put("text", text)
        case .thoughtCompleted(let thoughtId, let durationMs):
            try c.put("thought_id", thoughtId)
            try c.put("duration_ms", durationMs)
        case .permissionResolved(let requestId, let decision, let by):
            try c.put("request_id", requestId)
            try c.put("decision", decision)
            try c.put("by", by)
        case .uiResolved(let requestId, let answer, let by):
            try c.put("request_id", requestId)
            try c.put("answer", answer)
            try c.put("by", by)
        case .planUpdated(let entries):
            try c.put("entries", entries)
        case .taskStep(let taskId, let step):
            try c.put("task_id", taskId)
            try c.put("step", step)
        case .limitsUpdated(let windows):
            try c.put("windows", windows)
        case .promptLifecycle(let promptId, let state, let preview, let held):
            try c.put("prompt_id", promptId)
            try c.put("state", state)
            try c.put("preview", preview)
            if let held { try c.put("held", held) }
        case .notice(let level, let message):
            try c.put("level", level)
            try c.put("message", message)
        case .error(let kind, let message, let detail, let recoverable):
            try c.put("kind", kind)
            try c.put("message", message)
            try c.put("detail", detail)
            try c.put("recoverable", recoverable)
        default: break
        }
    }
}
