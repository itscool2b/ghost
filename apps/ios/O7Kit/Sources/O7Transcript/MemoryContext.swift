// The project memory o7 attaches to a message as reference context (`memoryContext.ts`).
//
// `visible_context` in `crates/o7-core/src/chatstore/memory.rs` appends it after everything the
// person sent, on the first message of a session and after the memory changes:
//
//     \n\nProject memory (Aura reference context, revision <id>)\n
//     [Last edited by an agent in <place>, not by the user. …\n]
//     Use this as project background. …\n
//     \n
//     <Markdown>\n
//
// The saved message is that whole text, so the chat reads it back apart: the person's words stay
// the message, and the memory folds behind "Project memory".

public struct MemoryContext: Sendable, Hashable, Codable {
    /// The revision that was attached, or `empty` when the project had none.
    public var revision: String
    /// "Last edited by an agent in …." when an agent, not the person, wrote this revision.
    public var editedBy: String?
    /// The memory as the agent received it.
    public var markdown: String
}

public struct SentText: Sendable, Hashable, Codable {
    /// What the person sent, without the memory attached after it.
    public var text: String
    public var memory: MemoryContext?
}

private let mark = "\n\nProject memory (Aura reference context, revision ".units
private let guidance = "Use this as project background.".units
private let edited = "Last edited by an agent in ".units
private let editedEnd = ", not by the user.".units

/// Splits a sent message into the person's words and the memory attached after them. Only a block
/// with o7's exact header counts, so a message that merely mentions project memory stays whole.
public func splitMemoryContext(_ sent: String) -> SentText {
    let (text, memory) = splitMemoryContext(units: sent.units)
    return SentText(text: memory == nil ? sent : String(units: text), memory: memory)
}

func splitMemoryContext(units sent: Units) -> (Units, MemoryContext?) {
    var at = JS.indexOf(sent, mark)
    while at >= 0 {
        if let memory = parse(JS.slice(sent, at + mark.count)) {
            return (JS.slice(sent, 0, at), memory)
        }
        at = JS.indexOf(sent, mark, from: at + 1)
    }
    return (sent, nil)
}

private func parse(_ block: Units) -> MemoryContext? {
    let close = JS.indexOf(block, ")\n".units)
    if close <= 0 { return nil }
    let revision = JS.slice(block, 0, close)
    if JS.hasSpace(revision) { return nil }
    let rest = JS.slice(block, close + 2)
    let end = JS.indexOf(rest, "\n\n".units)
    if end < 0 { return nil }
    var header = JS.split(JS.slice(rest, 0, end), [0x0A])
    let last = header.removeLast()
    guard JS.startsWith(last, guidance), header.count <= 1 else { return nil }
    let editedLine = header.first
    if let editedLine, !JS.startsWith(editedLine, edited) { return nil }
    // Only its start names who edited it; the rest tells the agent how to weigh it.
    let named = editedLine.map { JS.lastIndexOf($0, editedEnd) } ?? -1
    let body = JS.slice(rest, end + 2)
    var editedBy: String?
    if let editedLine {
        editedBy = named < 0 ? String(units: editedLine) : String(units: JS.slice(editedLine, 0, named)) + "."
    }
    return MemoryContext(
        revision: String(units: revision),
        editedBy: editedBy,
        markdown: String(units: JS.endsWith(body, [0x0A]) ? JS.slice(body, 0, -1) : body))
}
