public import GhostWire

// The words the chat screens use for protocol values (`present.ts`): pure, so the goldens check
// them directly.

public func statusWord(_ status: SessionStatus) -> String {
    switch status {
    case .stopped: "Stopped"
    case .starting: "Starting"
    case .ready: "Ready"
    case .running: "Working"
    case .cancelling: "Stopping the turn"
    case .stopping: "Stopping"
    case .failed: "Couldn't start"
    case .authRequired: "Sign-in needed"
    }
}

/// Nil for a folder that is fine.
public func folderStateLabel(_ state: FolderState) -> String? {
    switch state {
    case .ok: nil
    case .missing: "folder missing"
    case .notAllowed: "folder not allowed"
    }
}

public func autonomyLabel(_ autonomy: Autonomy) -> String {
    switch autonomy {
    case .fullAuto: "Full auto"
    case .autoEdit: "Auto-edit"
    case .ask: "Ask"
    }
}

public let autonomies: [Autonomy] = [.fullAuto, .autoEdit, .ask]

/// Who closed a card or question, when it was not the user's own answer.
private func closedBy(_ by: ResolvedBy) -> String? {
    switch by {
    case .user: nil
    case .auto: "Auto-approved"
    case .agent: "Withdrawn by the agent"
    case .turn: "Closed when the turn ended"
    case .system: "Closed when the agent stopped"
    case .other: "Closed"
    }
}

/// What a resolved permission card says: who closed it, or the choice the user made.
public func permissionResolutionLabel(_ resolution: PermissionResolution, options: [PermissionOption]) -> String {
    let closed = closedBy(resolution.by)
    if let closed, resolution.by != .auto { return closed }
    guard case .selected(let optionId, _) = resolution.decision else { return closed ?? "Dismissed" }
    let label = options.first { $0.optionId.same(as: optionId) }?.label ?? optionId
    return closed.map { "\($0): \(label)" } ?? "You chose: \(label)"
}

/// The answer a question got, in words. An answer of a kind this copy does not know reads as
/// nothing (the desktop gives `undefined`).
public func answerLabel(_ answer: UiAnswer, prompt: UiPrompt) -> String {
    switch answer {
    case .confirm(let yes):
        if case .link = prompt { return yes ? "Opened in the browser" : "Declined" }
        return yes ? "Yes" : "No"
    case .form(let accept, let values):
        if !accept { return "Declined" }
        let fields: [UiField] = if case .form(_, _, let fields) = prompt { UiField.fields(fields) } else { [] }
        guard case .array(let items) = values else { return "Submitted" }
        let shown = items.map { item -> String in
            let id = item["id"]?.string ?? ""
            let field = fields.first { $0.id.same(as: id) }
            let value = item["value"].flatMap(UiValue.init(json:))
            let text: String
            switch value {
            case .text(let words): text = words
            case .number(let number): text = number.map(JS.numberString) ?? "null"
            case .boolean(let flag): text = flag ? "Yes" : "No"
            case .choices(let selected):
                text = selected.map { index in
                    if case .choice(let options, _, _, _, _) = field?.kind, options.indices.contains(index) {
                        return options[index]
                    }
                    return "Choice \(index + 1)"
                }.joined(separator: ", ")
            case nil: text = ""
            }
            return "\(field?.label ?? id): \(text)"
        }
        return shown.isEmpty ? "Submitted" : shown.joined(separator: "; ")
    case .select(let index):
        if case .select(_, let options) = prompt, options.indices.contains(index) { return options[index] }
        return "Choice \(index + 1)"
    case .input(let text):
        return text
    case .questions(let answers):
        return answers.map { item in
            var question: UiQuestion?
            if case .questions(_, let questions) = prompt {
                question = questions.first { $0.id.same(as: item.questionId) }
            }
            var values = item.selected.map { index in
                question.flatMap { $0.options.indices.contains(index) ? $0.options[index].label : nil }
                    ?? "Choice \(index + 1)"
            }
            if let text = item.text { values.append(text) }
            let name = [question?.question, question?.header].compactMap { $0 }.first { !$0.isEmpty } ?? item.questionId
            return "\(name): \(values.joined(separator: ", "))"
        }.joined(separator: "; ")
    case .unknown:
        return ""
    }
}

public func questionResolutionLabel(_ resolution: QuestionResolution, prompt: UiPrompt) -> String {
    if resolution.by == .user {
        return resolution.answer.map { "You answered: \(answerLabel($0, prompt: prompt))" } ?? "Dismissed"
    }
    if let answer = resolution.answer { return answerLabel(answer, prompt: prompt) }
    return closedBy(resolution.by) ?? "Closed"
}

/// The buttons an error card offers.
public struct ErrorActions: Sendable, Hashable, Codable {
    /// Start the agent again (`chat_start`), continuing its session.
    public var retry: Bool
    /// Review a visible handoff into a fresh native session when continuing the old one failed.
    public var startFresh: Bool
    /// Review recreating the chat's app-owned worktree folder, deleted outside the app.
    public var recreate: Bool
}

/// Start failures offer Retry; turn errors (the session stays ready) and internal ones do not.
public func isStartFailure(_ kind: ErrorKind) -> Bool {
    switch kind {
    case .agentUnavailable, .folderMissing, .folderNotAllowed, .spawnFailed, .startTimeout,
        .protocolMismatch, .agentExited:
        true
    case .authRequired, .usageLimit, .agentError, .protocolError, .storage, .internal, .other:
        false
    }
}

/// Only the newest error card acts, and only while the session has no process: an old card's Retry
/// would start an agent for a problem that is gone. Start fresh shows when there is an earlier
/// session that a resume may have failed to continue. Recreate folder shows when the start failed
/// because the folder is missing and that folder is the chat's own worktree, deleted by hand.
public func errorActions(
    _ kind: ErrorKind, newest: Bool, live: Bool, resumable: Bool, recreatable: Bool? = nil
) -> ErrorActions {
    let retry = isStartFailure(kind) && newest && !live
    return ErrorActions(
        retry: retry, startFresh: retry && resumable,
        recreate: retry && kind == .folderMissing && recreatable == true)
}

/// A sign-in command the agent offers (`AuthCommand`).
public struct AuthCommand: Sendable, Hashable, Codable {
    public var label: String
    public var argv: [String]
    /// Variables the method sets, as name and value.
    public var env: [[String]]

    public init(label: String, argv: [String], env: [(String, String)]) {
        self.label = label
        self.argv = argv
        self.env = env.map { [$0.0, $0.1] }
    }
}

/// `/^[A-Za-z0-9_@%+=:,./-]+$/` or single quotes: a word as a POSIX shell reads it.
private func shellWord(_ word: String) -> String {
    let plain = !word.isEmpty && word.utf16.allSatisfy { unit in
        JS.isAsciiLetter(unit) || JS.isDigit(unit) || "_@%+=:,./-".utf16.contains(unit)
    }
    if plain { return word }
    return "'" + String(units: JS.join(JS.split(word.units, [0x27]), "'\\''".units)) + "'"
}

/// A sign-in command as the user types it: its variables, then its words.
public func authCommandText(_ command: AuthCommand) -> String {
    let env = command.env.map { pair in "\(pair.first ?? "")=\(shellWord(pair.count > 1 ? pair[1] : ""))" }
    return (env + command.argv.map(shellWord)).joined(separator: " ")
}

/// "1.2k in · 300 out · 800 cached (this turn)".
public func tokensLabel(_ usage: TokenUsage) -> String {
    var parts = ["\(formatCount(usage.inputTokens)) in", "\(formatCount(usage.outputTokens)) out"]
    if let cached = usage.cacheReadTokens, cached > 0 { parts.append("\(formatCount(cached)) cached") }
    if let reasoning = usage.reasoningTokens, reasoning > 0 { parts.append("\(formatCount(reasoning)) reasoning") }
    let scope = usage.scope == .turn ? "this turn" : "this session"
    return "\(parts.joined(separator: " · ")) (\(scope))"
}

/// "Context 12k of 200k", or nil when unknown.
public func contextLabel(_ usage: UsageInfo?) -> String? {
    guard let usage, let used = usage.contextUsed else { return nil }
    let size = usage.contextSize.map { " of \(formatCount($0))" } ?? ""
    return "Context \(formatCount(used))\(size)"
}

/// The session's cost so far, or nil when unknown.
public func costLabel(_ usage: UsageInfo?) -> String? {
    guard let cost = usage?.cost else { return nil }
    return formatCost(cost.amount, cost.currency)
}

public enum ConfigCategory: String, LenientEnum {
    case model, effort, mode
    case modelConfig = "model_config"
    case other
    public static let fallback = ConfigCategory.other
}

public struct ConfigChoice: Sendable, Hashable, Codable {
    public var value: String
    public var name: String
    public var description: String?
    /// The group the agent listed it under, when its choices were grouped.
    public var group: String?

    public init(value: String, name: String, description: String? = nil, group: String? = nil) {
        self.value = value
        self.name = name
        self.description = description
        self.group = group
    }
}

public enum ConfigKind: Sendable, Hashable {
    case select(current: String, choices: [ConfigChoice])
    case toggle(current: Bool)
}

/// A value for a `ConfigOption`: a choice for a select, a flag for a toggle.
public enum ConfigValue: Sendable, Hashable {
    case choice(String)
    case toggle(Bool)
}

/// One session option the agent offers.
public struct ConfigOption: Sendable, Hashable, Identifiable {
    public var id: String
    public var name: String
    public var description: String?
    public var category: ConfigCategory
    public var kind: ConfigKind
    /// A change waiting for the turn to end.
    public var pending: ConfigValue?

    public init(
        id: String, name: String, description: String? = nil, category: ConfigCategory, kind: ConfigKind,
        pending: ConfigValue? = nil
    ) {
        self.id = id
        self.name = name
        self.description = description
        self.category = category
        self.kind = kind
        self.pending = pending
    }
}

extension ConfigValue: Codable {
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: AnyKey.self)
        if try container.decode(String.self, forKey: AnyKey("type")) == "toggle" {
            self = .toggle(try container.decode(Bool.self, forKey: AnyKey("value")))
        } else {
            self = .choice(try container.decode(String.self, forKey: AnyKey("value")))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: AnyKey.self)
        switch self {
        case .choice(let value):
            try container.encode("choice", forKey: AnyKey("type"))
            try container.encode(value, forKey: AnyKey("value"))
        case .toggle(let value):
            try container.encode("toggle", forKey: AnyKey("type"))
            try container.encode(value, forKey: AnyKey("value"))
        }
    }
}

extension ConfigKind: Codable {
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: AnyKey.self)
        if try container.decode(String.self, forKey: AnyKey("type")) == "toggle" {
            self = .toggle(current: try container.decode(Bool.self, forKey: AnyKey("current")))
        } else {
            self = .select(
                current: try container.decode(String.self, forKey: AnyKey("current")),
                choices: try container.decodeIfPresent([ConfigChoice].self, forKey: AnyKey("choices")) ?? [])
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: AnyKey.self)
        switch self {
        case .select(let current, let choices):
            try container.encode("select", forKey: AnyKey("type"))
            try container.encode(current, forKey: AnyKey("current"))
            try container.encode(choices, forKey: AnyKey("choices"))
        case .toggle(let current):
            try container.encode("toggle", forKey: AnyKey("type"))
            try container.encode(current, forKey: AnyKey("current"))
        }
    }
}

extension ConfigOption: Codable {}

private let categoryOrder: [ConfigCategory] = [.model, .effort, .mode, .modelConfig, .other]

/// Options in the pickers' order: model, effort, mode, model config, other; stable within each.
public func orderedOptions(_ options: [ConfigOption]) -> [ConfigOption] {
    options.enumerated().sorted { a, b in
        let left = categoryOrder.firstIndex(of: a.element.category) ?? -1
        let right = categoryOrder.firstIndex(of: b.element.category) ?? -1
        return left != right ? left < right : a.offset < b.offset
    }.map(\.element)
}

/// Plain names for the thinking levels agents send as bare ids (`xhigh`, `low`).
private let levelNames: [String: String] = [
    "default": "Default", "auto": "Auto", "none": "None", "off": "Off", "minimal": "Minimal",
    "low": "Low", "medium": "Medium", "high": "High", "xhigh": "Extra high", "max": "Max",
]

/// Shorter names for the composer's pill at narrow widths.
private let levelShortNames: [String: String] = ["Extra high": "X-high", "Medium": "Med", "Minimal": "Min"]

/// A thinking level's name: the agent's own when it gave one, else a plain name for its id
/// ("xhigh" reads "Extra high"), else the id with a capital.
public func levelName(value: String, name: String) -> String {
    if !name.same(as: value) { return name }
    let lowered = JS.lowercased(value)
    if let known = levelNames.first(where: { $0.key.same(as: lowered) })?.value { return known }
    return JS.capitalized(name)
}

public func levelName(_ choice: Choice) -> String { levelName(value: choice.value, name: choice.name) }

/// The pill's short form of a level's name.
public func levelShortName(_ name: String) -> String {
    levelShortNames.first { $0.key.same(as: name) }?.value ?? name
}

/// A pending value in words: the choice's name, or on and off.
public func pendingLabel(_ option: ConfigOption, _ value: ConfigValue) -> String {
    switch value {
    case .toggle(let flag): return flag ? "on" : "off"
    case .choice(let chosen):
        if case .select(_, let choices) = option.kind {
            return choices.first { $0.value.same(as: chosen) }?.name ?? chosen
        }
        return chosen
    }
}

/// `/^\/(\S*)$/`: the slash commands' query after a leading "/", or nil when the menu is closed.
public func slashQuery(_ text: String) -> String? {
    let units = text.units
    guard units.first == 0x2F else { return nil }
    let rest = Array(units.dropFirst())
    return rest.contains(where: JS.isSpace) ? nil : String(units: rest)
}

/// "1 minute", "3 minutes".
public func minutesLabel(_ minutes: Int) -> String {
    minutes == 1 ? "1 minute" : "\(minutes) minutes"
}

/// A slash command the agent offers.
public struct SlashCommand: Sendable, Hashable, Codable {
    public var name: String
    public var description: String
    public var inputHint: String?

    enum CodingKeys: String, CodingKey {
        case name, description
        case inputHint = "input_hint"
    }

    public init(name: String, description: String, inputHint: String? = nil) {
        self.name = name
        self.description = description
        self.inputHint = inputHint
    }
}

/// The slash commands whose names contain `query`, those starting with it first.
public func matchingCommands(_ commands: [SlashCommand], query: String) -> [SlashCommand] {
    let needle = JS.lowercased(query).units
    let named = commands.map { (command: $0, name: JS.lowercased($0.name).units) }
    let matches = named.filter { JS.includes($0.name, needle) }
    return matches.filter { JS.startsWith($0.name, needle) }.map(\.command)
        + matches.filter { !JS.startsWith($0.name, needle) }.map(\.command)
}
