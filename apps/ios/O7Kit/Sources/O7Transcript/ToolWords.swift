public import O7Wire

// How tool calls read in the transcript (`toolWords.ts`, docs/design/visual-language.md). A call's
// own line says its verb in the tense of its state: "Searching for useState" while it runs,
// "Searched for useState" once done, and the agent's own imperative ("Search for useState") while
// it waits, failed or was stopped, beside that status. A stretch of calls reads as one sentence of
// counts in the same tenses: "Read 3 files and searched for 2 patterns", or "Reading 3 files…"
// while any runs.

/// A verb's past and progressive forms, by its base form.
private let forms: [String: (past: String, progressive: String)] = [
    "add": ("added", "adding"), "analyze": ("analyzed", "analyzing"),
    "apply": ("applied", "applying"), "build": ("built", "building"),
    "call": ("called", "calling"), "check": ("checked", "checking"),
    "clean": ("cleaned", "cleaning"), "clone": ("cloned", "cloning"),
    "commit": ("committed", "committing"), "compare": ("compared", "comparing"),
    "compile": ("compiled", "compiling"), "copy": ("copied", "copying"),
    "count": ("counted", "counting"), "create": ("created", "creating"),
    "delete": ("deleted", "deleting"), "deploy": ("deployed", "deploying"),
    "download": ("downloaded", "downloading"), "edit": ("edited", "editing"),
    "enter": ("entered", "entering"), "execute": ("executed", "executing"),
    "exit": ("exited", "exiting"), "explore": ("explored", "exploring"),
    "extract": ("extracted", "extracting"), "fetch": ("fetched", "fetching"),
    "find": ("found", "finding"), "fix": ("fixed", "fixing"),
    "format": ("formatted", "formatting"), "generate": ("generated", "generating"),
    "get": ("got", "getting"), "glob": ("globbed", "globbing"),
    "grep": ("grepped", "grepping"), "inspect": ("inspected", "inspecting"),
    "install": ("installed", "installing"), "kill": ("killed", "killing"),
    "lint": ("linted", "linting"), "list": ("listed", "listing"),
    "load": ("loaded", "loading"), "look": ("looked", "looking"),
    "make": ("made", "making"), "merge": ("merged", "merging"),
    "move": ("moved", "moving"), "open": ("opened", "opening"),
    "patch": ("patched", "patching"), "print": ("printed", "printing"),
    "pull": ("pulled", "pulling"), "push": ("pushed", "pushing"),
    "query": ("queried", "querying"), "read": ("read", "reading"),
    "remove": ("removed", "removing"), "rename": ("renamed", "renaming"),
    "replace": ("replaced", "replacing"), "restart": ("restarted", "restarting"),
    "review": ("reviewed", "reviewing"), "run": ("ran", "running"),
    "save": ("saved", "saving"), "search": ("searched", "searching"),
    "show": ("showed", "showing"), "start": ("started", "starting"),
    "stop": ("stopped", "stopping"), "switch": ("switched", "switching"),
    "test": ("tested", "testing"), "update": ("updated", "updating"),
    "upload": ("uploaded", "uploading"), "use": ("used", "using"),
    "verify": ("verified", "verifying"), "view": ("viewed", "viewing"),
    "write": ("wrote", "writing"),
]

/// Words a verb carries along before its object ("Search for x", "Search the web for x").
private let particles = ["the web for", "tools for", "for", "up"]

/// `/^[a-z./~][^\s]*(?:\s|$)/`: a shell command as agents title one, starting like a program or a
/// path, never a sentence. After the first character the rest always matches (a run of non-space
/// ends at a space or at the end), so only the first character decides.
private func looksLikeCommand(_ text: Units) -> Bool {
    guard let first = text.first else { return false }
    return JS.isAsciiLower(first) || first == 0x2E || first == 0x2F || first == 0x7E
}

/// `/^(?:the|a|an|all|any|each|every|my|our|some|this|that|these|those)\s/i`: "Run the layout
/// checks" names its work in words; "Run cargo test" names a command.
private let proseWords = [
    "the", "a", "an", "all", "any", "each", "every", "my", "our", "some", "this", "that", "these",
    "those",
].map(\.units)

private func startsWithProse(_ text: Units) -> Bool {
    proseWords.contains { word in
        JS.startsWithIgnoringAsciiCase(text, word) && word.count < text.count && JS.isSpace(text[word.count])
    }
}

/// `value.replace(/^`([^`]+)`$/, "$1")`: one pair of backticks around the whole value comes off.
private func unwrap(_ value: Units) -> Units {
    guard value.count >= 3, value.first == 0x60, value.last == 0x60 else { return value }
    let inner = value[1..<(value.count - 1)]
    return inner.contains(0x60) ? value : Array(inner)
}

/// `/^[A-Z][a-z]+$/`
private func isCapitalizedWord(_ text: Units) -> Bool {
    guard text.count >= 2, JS.isAsciiUpper(text[0]) else { return false }
    return text.dropFirst().allSatisfy(JS.isAsciiLower)
}

/// A title split into its leading verb phrase and the object the call acts on.
public struct ToolTitle: Sendable, Hashable, Codable {
    /// The verb in the agent's own form ("Search"), or nil when the title starts with none.
    public var verb: String?
    /// Words between the verb and the object ("for", "the web for").
    public var particle: String
    public var object: String
    /// The title is a bare command ("pnpm test"); it reads with "Run", "Running" or "Ran".
    public var command: Bool
}

public func parseTitle(_ title: String, kind: ToolKind) -> ToolTitle {
    let text = unwrap(JS.trim(title.units))
    let space = JS.indexOf(text, [0x20])
    let first = space > 0 ? Array(text[..<space]) : text
    // The test of the first word is ASCII, so its lowercase is too.
    if isCapitalizedWord(first), forms[String(units: first).lowercased()] != nil {
        var rest = space > 0 ? JS.trim(JS.slice(text, space + 1)) : []
        var particle: Units = []
        for words in particles.map(\.units) where JS.startsWithIgnoringAsciiCase(rest, words + [0x20]) {
            particle = Array(rest[..<words.count])
            rest = JS.trim(JS.slice(rest, words.count + 1))
            break
        }
        if !rest.isEmpty {
            return ToolTitle(
                verb: String(units: first), particle: String(units: particle),
                object: String(units: unwrap(rest)), command: false)
        }
    }
    if kind == .execute && looksLikeCommand(text) {
        return ToolTitle(verb: nil, particle: "", object: String(units: unwrap(text)), command: true)
    }
    return ToolTitle(verb: nil, particle: "", object: String(units: text), command: false)
}

/// A base verb in the tense of a call's state, keeping its capital. The verbs are ASCII.
private func inflect(_ verb: String, _ display: ToolDisplayStatus) -> String {
    guard let form = forms[verb.lowercased()] else { return verb }
    let word =
        switch display {
        case .completed: form.past
        case .inProgress: form.progressive
        default: verb.lowercased()
        }
    return verb.utf16.first.map(JS.isAsciiUpper) == true ? JS.capitalized(word) : word
}

/// How a call's line reads: the verb phrase in sans, then the object, in mono when it is code.
public struct ToolPhrase: Sendable, Hashable, Codable {
    public var verb: String?
    public var object: String
    /// A path, a pattern, a host or a command: set in Geist Mono.
    public var literal: Bool
}

/// `/^([a-z][\w.-]*):\s+(\S.*)$/` on the trimmed title: a tool's own name and its argument.
private func namedTool(_ title: Units) -> (Units, Units)? {
    guard let first = title.first, JS.isAsciiLower(first) else { return nil }
    var at = 1
    while at < title.count && (JS.isWord(title[at]) || title[at] == 0x2E || title[at] == 0x2D) { at += 1 }
    guard at < title.count, title[at] == 0x3A else { return nil }
    let name = Array(title[..<at])
    at += 1
    let spaceStart = at
    while at < title.count && JS.isSpace(title[at]) { at += 1 }
    guard at > spaceStart, at < title.count else { return nil }
    let argument = Array(title[at...])
    guard !argument.contains(where: JS.isLineTerminator) else { return nil }
    return (name, argument)
}

public func toolPhrase(_ title: String, kind: ToolKind, display: ToolDisplayStatus) -> ToolPhrase {
    // "write_file: aura-perm.txt": a tool's own name and its argument.
    if let (name, argument) = namedTool(JS.trim(title.units)) {
        return ToolPhrase(verb: String(units: name), object: String(units: unwrap(argument)), literal: true)
    }
    let parsed = parseTitle(title, kind: kind)
    if parsed.command {
        return ToolPhrase(verb: inflect("Run", display), object: parsed.object, literal: true)
    }
    guard let verb = parsed.verb else {
        return ToolPhrase(verb: nil, object: parsed.object, literal: false)
    }
    let phrase = [inflect(verb, display), parsed.particle].filter { !$0.isEmpty }.joined(separator: " ")
    // A pattern is code even with spaces in it; a command after "Run" too. Otherwise one token is.
    let object = parsed.object.units
    let literal =
        !JS.hasSpace(object)
        || (kind == .search && parsed.particle == "for")
        || (kind == .execute && verb == "Run" && looksLikeCommand(object) && !startsWithProse(object))
    return ToolPhrase(verb: phrase, object: parsed.object, literal: literal)
}

/// What a stretch of calls did, by kind, as count phrases: past, progressive, noun, nouns.
private func kindPhrase(_ kind: ToolKind) -> (past: String, progressive: String, one: String, many: String) {
    switch kind {
    case .read: ("read", "reading", "file", "files")
    case .edit: ("edited", "editing", "file", "files")
    case .delete: ("deleted", "deleting", "file", "files")
    case .move: ("moved", "moving", "file", "files")
    case .search: ("searched for", "searching for", "pattern", "patterns")
    case .execute: ("ran", "running", "command", "commands")
    case .think: ("worked through", "working through", "step", "steps")
    case .fetch: ("browsed", "browsing", "page", "pages")
    case .switchMode: ("changed mode", "changing mode", "", "")
    case .other: ("used", "using", "tool", "tools")
    }
}

/// "A", "A and B", "A, B and C".
private func sentence(_ parts: [String]) -> String {
    if parts.count <= 1 { return parts.first ?? "" }
    return "\(parts.dropLast().joined(separator: ", ")) and \(parts[parts.count - 1])"
}

/// One call of a stretch, as its header counts it.
public struct GroupedTool: Sendable, Hashable, Codable {
    public var toolKind: ToolKind
    public var display: ToolDisplayStatus

    public init(toolKind: ToolKind, display: ToolDisplayStatus) {
        self.toolKind = toolKind
        self.display = display
    }
}

private func running(_ tool: GroupedTool) -> Bool {
    tool.display == .pending || tool.display == .inProgress
}

/// A stretch of calls as one sentence: "Read 3 files, searched for 2 patterns and ran 1 command",
/// in the progressive while any call still runs. Kinds keep the order they first appeared in.
public func toolsSummary(_ tools: [GroupedTool]) -> String {
    let live = tools.contains(where: running)
    var counts: [(ToolKind, Int)] = []
    for tool in tools {
        if let at = counts.firstIndex(where: { $0.0 == tool.toolKind }) {
            counts[at].1 += 1
        } else {
            counts.append((tool.toolKind, 1))
        }
    }
    let parts = counts.map { kind, count in
        let phrase = kindPhrase(kind)
        let verb = live ? phrase.progressive : phrase.past
        if phrase.one.isEmpty { return count == 1 ? verb : "\(verb) \(count) times" }
        return "\(verb) \(count) \(count == 1 ? phrase.one : phrase.many)"
    }
    return JS.capitalized(sentence(parts))
}

/// What a stretch's header says at its end: failures and stops show; the rest is for readers.
public struct GroupResult: Sendable, Hashable, Codable {
    public enum Tone: String, Sendable, Hashable, Codable {
        case failed, stopped, quiet
    }

    public var text: String
    public var tone: Tone
}

public func groupResult(_ tools: [GroupedTool]) -> GroupResult {
    let failed = tools.filter { $0.display == .failed }.count
    let stopped = tools.filter { $0.display == .stopped }.count
    let single = tools.count == 1
    if failed > 0 { return GroupResult(text: single ? "Failed" : "\(failed) failed", tone: .failed) }
    if stopped > 0 { return GroupResult(text: single ? "Stopped" : "\(stopped) stopped", tone: .stopped) }
    if tools.contains(where: running) { return GroupResult(text: "Running", tone: .quiet) }
    return GroupResult(text: "Done", tone: .quiet)
}
