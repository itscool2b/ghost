// What a tool call shows besides its output (`toolDetails.ts`): its input, the links a web search
// found, and the lines an edit changed.

/// One input the call was given: its name and value, as text.
public struct InputField: Sendable, Hashable, Codable {
    public var key: String
    public var value: String
    /// Set on its own lines (code, JSON, long or multi-line text) rather than beside its name.
    public var block: Bool
}

private let maxFields = 20
private let maxValueChars = 2000
/// An edit's texts, which its diff shows.
private let inTheDiff = ExactSet(["old_string", "new_string", "old_text", "new_text", "content", "edits"])

/// The call's input worth showing: everything it was given, except what the call already shows (an
/// edit's texts in its diff; Claude's to-do list in the turn's plan), and nothing when the title
/// already says the one thing it was given. Fields come in code unit order of their names: the
/// wire's JSON objects reach the app without their order (see JSValue.swift).
public func toolInput(_ tool: ToolItem) -> [InputField] {
    guard tool.toolName?.same(as: "TodoWrite") != true, let raw = tool.rawInput,
        case .object(let input) = JSValue(raw)
    else { return [] }
    let diffs = tool.content.contains { if case .diff = $0 { true } else { false } }
    // A path the call names in its locations line shows there already.
    let located = ExactSet(tool.locations.map(\.path))
    var fields: [InputField] = []
    for (key, value) in input.entries {
        if fields.count >= maxFields { break }
        let name = String(units: key)
        if value == .null || value == .string([]) || (diffs && inTheDiff.contains(name)) { continue }
        if case .string(let text) = value, located.contains(String(units: text)) { continue }
        let text: Units = if case .string(let text) = value { text } else { value.stringified(indent: 2) }
        let clipped = text.count > maxValueChars ? JS.slice(text, 0, maxValueChars) + "…".units : text
        let structured: Bool =
            switch value {
            case .object, .array: true
            default: false
            }
        fields.append(
            InputField(
                key: name, value: String(units: clipped),
                block: structured || clipped.contains(0x0A) || clipped.count > 80))
    }
    // One input the title already says (a path, a query) adds nothing.
    if fields.count == 1, !fields[0].block, JS.includes(tool.title.units, fields[0].value.units) { return [] }
    return fields
}

/// A JSON text set out over lines with two-space indents (an MCP server's reply, mostly), or nil
/// when it is not JSON, already set out that way, or too large to reformat.
public func prettyJson(_ text: String) -> String? {
    let trimmed = JS.trim(text.units)
    if trimmed.count < 2 || trimmed.count > 200_000 { return nil }
    let pair = [trimmed[0], trimmed[trimmed.count - 1]]
    if pair != "{}".units && pair != "[]".units { return nil }
    guard let parsed = JSValue.parse(trimmed) else { return nil }
    let pretty = parsed.stringified(indent: 2)
    return pretty == trimmed ? nil : String(units: pretty)
}

/// The JSON array starting at `start` in `text`, read to its matching bracket.
private func arrayAt(_ text: Units, _ start: Int) -> Units? {
    var depth = 0
    var quoted = false
    var index = start
    while index < text.count {
        let char = text[index]
        if quoted {
            if char == 0x5C {
                index += 1
            } else if char == 0x22 {
                quoted = false
            }
        } else if char == 0x22 {
            quoted = true
        } else if char == 0x5B || char == 0x7B {
            depth += 1
        } else if char == 0x5D || char == 0x7D {
            depth -= 1
            if depth == 0 { return JS.slice(text, start, index + 1) }
        }
        index += 1
    }
    return nil
}

public struct SearchLink: Sendable, Hashable, Codable {
    public var title: String
    public var url: String
    public var host: String
}

/// `markdown.ts`'s `safeHref`: only web links become links; anything else (javascript:, file:,
/// data:) stays text. `/^https?:\/\/[^\s]+$/i` on the trimmed text.
public func safeHref(_ href: String) -> String? {
    let trimmed = JS.trim(href.units)
    guard JS.startsWithIgnoringAsciiCase(trimmed, "http".units) else { return nil }
    var at = 4
    if at < trimmed.count && JS.foldAscii(trimmed[at]) == 0x73 { at += 1 }
    guard JS.matches(trimmed, "://".units, at: at) else { return nil }
    let rest = trimmed[(at + 3)...]
    guard !rest.isEmpty, !rest.contains(where: JS.isSpace) else { return nil }
    return String(units: trimmed)
}

/// The pages a web search found, from Claude Code's WebSearch result, which lists them after
/// `Links:` as JSON: `Links: [{"title":"…","url":"https://…"}, …]`. Only http and https.
public func searchLinks(_ tool: ToolItem) -> [SearchLink] {
    guard tool.toolName?.same(as: "WebSearch") == true else { return [] }
    let text = JS.join(
        tool.content.map { content in
            if case .text(let text, _) = content { text.units } else { [] }
        }, [0x0A])
    var links: [SearchLink] = []
    var from = 0
    let label = "Links:".units
    while true {
        let at = JS.indexOf(text, label, from: from)
        if at < 0 { break }
        let open = JS.indexOf(text, [0x5B], from: at)
        from = at + 6
        if open < 0 { break }
        guard let json = arrayAt(text, open) else { break }
        // Not the list Claude Code writes, when it does not parse: the text below still shows.
        guard case .array(let entries) = JSValue.parse(json) else { continue }
        for entry in entries {
            guard let href = entry["url"]?.stringValue, let url = safeHref(href),
                !links.contains(where: { $0.url.same(as: url) })
            else { continue }
            let title = entry["title"]?.stringValue.flatMap { JS.trim($0.units).isEmpty ? nil : $0 } ?? url
            // `new URL` throws for a host it cannot read, which ends this list.
            guard let host = URLHost.hostname(url) else { break }
            let bare = host.hasPrefix("www.") ? String(host.dropFirst(4)) : host
            links.append(SearchLink(title: String(units: JS.trim(title.units)), url: url, host: bare))
        }
    }
    return Array(links.prefix(20))
}

/// A WebSearch result's text without its `Links:` JSON, which shows as links above it.
public func withoutSearchLinks(_ text: String) -> String {
    var out = text.units
    let label = "Links:".units
    while true {
        let at = JS.indexOf(out, label)
        if at < 0 { return String(units: out) }
        let open = JS.indexOf(out, [0x5B], from: at)
        guard open >= 0, let json = arrayAt(out, open) else { return String(units: out) }
        out = JS.trim(JS.trimEnd(JS.slice(out, 0, at)) + [0x0A, 0x0A] + JS.trimStart(JS.slice(out, open + json.count)))
    }
}

/// The lines an edit added and removed.
public struct DiffDelta: Sendable, Hashable, Codable {
    public var added: Int
    public var removed: Int
}

/// The lines the call's edits added and removed, or nil when it has no diff to count.
public func diffDelta(_ tool: ToolItem) -> DiffDelta? {
    var added = 0
    var removed = 0
    var counted = false
    for content in tool.content {
        guard case .diff(_, let oldText, let newText, let truncated) = content else { continue }
        // A diff too large for the event keeps empty texts until it is loaded: nothing to count.
        if truncated != nil && newText.isEmpty && (oldText ?? "").isEmpty { continue }
        let diff = diffLines(oldText, newText)
        if diff.truncated { return nil }
        added += diff.added
        removed += diff.removed
        counted = true
    }
    return counted ? DiffDelta(added: added, removed: removed) : nil
}
