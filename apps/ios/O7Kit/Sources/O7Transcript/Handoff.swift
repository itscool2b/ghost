// A handoff as the chat shows it (`handoff.ts`). The text is what the next agent receives, written
// by `crates/o7-core/src/conversation.rs`; this reads it back into its parts: the conversation, the
// working folder, the commands that ran, the last one still failing with its first error, and the
// changes. Anything that does not read as a handoff gives nil and shows as plain text.

/// The line Rust puts between a handoff and the person's own words in the prompt it sends.
public let handoffPromptMark = "\n\nCurrent user prompt\n"
private let header = "Visible conversation handoff\n".units
private let omittedLine = "[Some earlier context is omitted to fit the handoff budget.]".units
private let promptMark = handoffPromptMark.units
private let commandsLine = "Commands run (newest last)".units
private let failingLine = "Last failing command".units
private let firstErrorLine = "First error".units

public struct HandoffTurn: Sendable, Hashable, Codable {
    /// `Number` of the turn's digits, as JavaScript reads them (a huge one is a rounded double).
    public var ordinal: Double
    public var agent: String
    /// How a turn that did not finish ended ("interrupted", "failed"); nil when it finished.
    public var ended: String?
    public var user: String
    public var answer: String
}

public enum HandoffCommandState: String, Sendable, Hashable, Codable {
    case ok, failed, running
}

public struct HandoffCommand: Sendable, Hashable, Codable {
    /// Its place in the list the handoff sent (the same command may run more than once).
    public var position: Int
    public var turn: Double
    /// `exit 101`, `failed`, `ok` or `running`.
    public var outcome: String
    public var command: String
    /// Where it ran, when that was not the working folder.
    public var cwd: String?
    public var state: HandoffCommandState
}

public struct HandoffFailure: Sendable, Hashable, Codable {
    public var command: HandoffCommand
    public var error: String
}

public struct ParsedHandoff: Sendable, Hashable, Codable {
    /// A folder move written before the handoff ("Working folder changes from … to …").
    public var preface: String?
    public var omitted: Bool
    public var turns: [HandoffTurn]
    public var folder: String?
    public var commands: [HandoffCommand]
    /// Commands that ran before the listed ones.
    public var earlier: Double
    public var failure: HandoffFailure?
    public var files: String
    public var diffStat: String
}

private func commandState(_ outcome: String) -> HandoffCommandState {
    if outcome == "running" { return .running }
    if outcome == "ok" || outcome == "exit 0" { return .ok }
    return .failed
}

/// One or more ASCII digits from `index`, and where they end.
private func digits(_ text: Units, from index: Int) -> Int? {
    var end = index
    while end < text.count && JS.isDigit(text[end]) { end += 1 }
    return end > index ? end : nil
}

/// `/^\[turn (\d+)\] (exit -?\d+|failed|ok|running) \| (.*)$/`
private func command(_ line: Units, _ position: Int) -> HandoffCommand? {
    let open = "[turn ".units
    guard JS.startsWith(line, open), let turnEnd = digits(line, from: open.count) else { return nil }
    let turn = Array(line[open.count..<turnEnd])
    guard JS.matches(line, "] ".units, at: turnEnd) else { return nil }
    let outcomeStart = turnEnd + 2
    var outcomeEnd: Int?
    if JS.matches(line, "exit ".units, at: outcomeStart) {
        var at = outcomeStart + 5
        if at < line.count && line[at] == 0x2D { at += 1 }
        outcomeEnd = digits(line, from: at)
    } else {
        for word in ["failed", "ok", "running"] where JS.matches(line, word.units, at: outcomeStart) {
            outcomeEnd = outcomeStart + word.utf16.count
            break
        }
    }
    guard let outcomeEnd, JS.matches(line, " | ".units, at: outcomeEnd) else { return nil }
    let rest = Array(line[(outcomeEnd + 3)...])
    // `.` stops at a line terminator, and `$` is the end of the line.
    guard !rest.contains(where: JS.isLineTerminator) else { return nil }
    let outcome = String(units: line[outcomeStart..<outcomeEnd])
    return HandoffCommand(
        position: position, turn: JS.toNumber(String(units: turn)), outcome: outcome,
        command: String(units: rest), cwd: nil, state: commandState(outcome))
}

/// The turns of the conversation, each from its `\nTurn N (agent[, ended])\nUser:\n` head to the
/// next: `/\nTurn (\d+) \(([^)\n]*)\)\nUser:\n/g`.
private func turns(_ block: Units) -> [HandoffTurn] {
    struct Head {
        let index: Int
        let end: Int
        let ordinal: Units
        let label: Units
    }
    var heads: [Head] = []
    let lead = "\nTurn ".units
    let close = ")\nUser:\n".units
    var from = 0
    while true {
        let index = JS.indexOf(block, lead, from: from)
        if index < 0 { break }
        let numberStart = index + lead.count
        if let numberEnd = digits(block, from: numberStart),
            JS.matches(block, " (".units, at: numberEnd)
        {
            let labelStart = numberEnd + 2
            var labelEnd = labelStart
            while labelEnd < block.count && block[labelEnd] != 0x29 && block[labelEnd] != 0x0A {
                labelEnd += 1
            }
            if JS.matches(block, close, at: labelEnd) {
                let end = labelEnd + close.count
                heads.append(
                    Head(
                        index: index, end: end, ordinal: Array(block[numberStart..<numberEnd]),
                        label: Array(block[labelStart..<labelEnd])))
                from = end
                continue
            }
        }
        from = index + 1
    }
    let finalAnswer = "\nFinal answer:\n".units
    return heads.enumerated().map { offset, head in
        let end = offset + 1 < heads.count ? heads[offset + 1].index : block.count
        let body = JS.slice(block, head.end, end)
        let split = JS.lastIndexOf(body, finalAnswer)
        let parts = JS.split(head.label, ", ".units)
        let rest = parts.dropFirst()
        return HandoffTurn(
            ordinal: JS.toNumber(String(units: head.ordinal)),
            agent: String(units: parts[0]),
            ended: rest.isEmpty ? nil : String(units: JS.join(Array(rest), ", ".units)),
            user: String(units: JS.trim(split >= 0 ? JS.slice(body, 0, split) : body)),
            answer: split >= 0 ? String(units: JS.trim(JS.slice(body, split + finalAnswer.count))) : "")
    }
}

/// The parts of a handoff, or nil when the text is not one the chat can lay out.
public func parseHandoff(_ string: String) -> ParsedHandoff? {
    let text = string.units
    let at = JS.indexOf(text, header)
    if at < 0 { return nil }
    let prefaceText = JS.trim(JS.slice(text, 0, at))
    let changesMark = "\nCurrent file changes\n".units
    let statMark = "\nGit diff --stat\n".units
    let changesAt = JS.lastIndexOf(text, changesMark)
    let statAt = JS.lastIndexOf(text, statMark)
    if changesAt < 0 || statAt < changesAt { return nil }
    let body = JS.slice(text, at + header.count, changesAt)
    // The header's second line, then the conversation, then the work (newer handoffs only).
    let workMark = "\nWorking folder\n".units
    let workAt = JS.lastIndexOf(body, workMark)
    let conversation = workAt >= 0 ? JS.slice(body, 0, workAt) : body
    let work = workAt >= 0 ? JS.slice(body, workAt + workMark.count) : nil
    var folder: String?
    var commands: [HandoffCommand] = []
    var earlier = 0.0
    var failure: HandoffFailure?
    if let work {
        let lines = JS.split(work, [0x0A])
        folder = String(units: lines[0])
        enum Section { case none, commands, failing, error }
        var section = Section.none
        var failing: HandoffCommand?
        var error: [Units] = []
        for line in lines.dropFirst() {
            if section == .error {
                error.append(line)
                continue
            }
            if line == commandsLine {
                section = .commands
            } else if line == failingLine {
                section = .failing
            } else if line == firstErrorLine && section == .failing {
                section = .error
            } else if section == .commands {
                if let hidden = earlierCommands(line) {
                    earlier = JS.toNumber(String(units: hidden))
                } else if let cwd = commandFolder(line), let last = commands.last {
                    var moved = last
                    moved.cwd = String(units: cwd)
                    commands[commands.count - 1] = moved
                } else if let read = command(line, commands.count) {
                    commands.append(read)
                }
            } else if section == .failing && failing == nil {
                failing = command(line, -1)
            }
        }
        if let failing {
            failure = HandoffFailure(
                command: failing, error: String(units: JS.trim(JS.join(error, [0x0A]))))
        }
    }
    let files = JS.slice(text, changesAt + changesMark.count, statAt)
    let diffStat = withoutHandoffEnd(JS.slice(text, statAt + statMark.count))
    return ParsedHandoff(
        preface: prefaceText.isEmpty ? nil : String(units: prefaceText),
        omitted: JS.includes(conversation, omittedLine),
        turns: turns(conversation),
        folder: folder,
        commands: commands,
        earlier: earlier,
        failure: failure,
        files: String(units: JS.trim(files)),
        diffStat: String(units: JS.trim(diffStat)))
}

/// `/^\[(\d+) earlier commands are not listed\.\]$/`: the count.
private func earlierCommands(_ line: Units) -> Units? {
    guard line.first == 0x5B, let end = digits(line, from: 1) else { return nil }
    let tail = " earlier commands are not listed.]".units
    guard JS.matches(line, tail, at: end), end + tail.count == line.count else { return nil }
    return Array(line[1..<end])
}

/// `/^ {2}in (.*)$/`: the folder.
private func commandFolder(_ line: Units) -> Units? {
    let lead = "  in ".units
    guard JS.startsWith(line, lead) else { return nil }
    let rest = Array(line[lead.count...])
    return rest.contains(where: JS.isLineTerminator) ? nil : rest
}

/// `text.replace(/\nEnd of handoff\s*$/, "")`: the closing line and the whitespace after it.
private func withoutHandoffEnd(_ text: Units) -> Units {
    let end = "\nEnd of handoff".units
    // Only an occurrence followed by nothing but whitespace matches, so it is the last one.
    let at = JS.lastIndexOf(text, end)
    guard at >= 0, text[(at + end.count)...].allSatisfy(JS.isSpace) else { return text }
    return Array(text[..<at])
}

/// A sent message whose prompt began with a handoff: the handoff, and the words the person typed.
public struct HandoffSplit: Sendable, Hashable, Codable {
    public var handoff: String
    public var text: String
}

/// The handoff and the person's words of a sent message, or nil for any other message.
public func splitHandoff(_ sent: String) -> HandoffSplit? {
    splitHandoff(units: sent.units)
}

func splitHandoff(units sent: Units) -> HandoffSplit? {
    guard JS.includes(sent, header) else { return nil }
    let mark = JS.indexOf(sent, promptMark)
    if mark < 0 || !JS.includes(JS.slice(sent, 0, mark), "\nEnd of handoff".units) { return nil }
    return HandoffSplit(
        handoff: String(units: JS.slice(sent, 0, mark)),
        text: String(units: JS.slice(sent, mark + promptMark.count)))
}
