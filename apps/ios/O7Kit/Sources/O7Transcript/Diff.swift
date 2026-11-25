// A line diff for tool results ("Edited src/main.rs +4 -1" and its expanded view) and changes
// (`apps/desktop/src/lib/diff.ts`).
//
// Longest common subsequence over lines, which is exact and plenty fast for the edits agents make.
// The lines both texts start and end with are set aside first, so only the stretch between is
// compared. A stretch past `maxDiffLines` is not diffed line by line; it shows as a plain
// replacement, marked truncated, with exact added and removed counts still given. Lines compare
// code unit by code unit, as JavaScript compares them, not by Swift's canonical equivalence.

public struct DiffLine: Sendable, Hashable, Codable {
    public enum Op: String, Sendable, Hashable, Codable {
        case same, add, remove
    }

    public var op: Op
    public var text: String

    public init(op: Op, text: String) {
        self.op = op
        self.text = text
    }
}

public struct LineDiff: Sendable, Hashable, Codable {
    public var lines: [DiffLine]
    public var added: Int
    public var removed: Int
    /// True when the input was too large to diff and `lines` is a shortened replacement view.
    public var truncated: Bool
}

/// The longest changed stretch, on either side, compared line by line.
public let maxDiffLines = 2000
/// How many lines a replacement view lists, half from each side.
private let replacementLines = 400

/// Lines of `text`, without the empty entry a trailing newline would add.
public func splitLines(_ text: String) -> [String] {
    if text.isEmpty { return [] }
    var lines = text.utf8.split(separator: 0x0A, omittingEmptySubsequences: false).map {
        String(decoding: $0, as: UTF8.self)
    }
    if lines.last?.isEmpty == true { lines.removeLast() }
    return lines
}

public func diffLines(_ oldText: String?, _ newText: String) -> LineDiff {
    let before = splitLines(oldText ?? "")
    let after = splitLines(newText)
    // Each distinct line as a number, so lines compare exactly and cheaply.
    var ids: [ExactKey: Int] = [:]
    func id(_ line: String) -> Int {
        let key = ExactKey(line)
        if let known = ids[key] { return known }
        ids[key] = ids.count
        return ids.count - 1
    }
    let old = before.map(id)
    let new = after.map(id)

    // Lines both texts start or end with are unchanged; only the stretch between is compared.
    var head = 0
    while head < old.count && head < new.count && old[head] == new[head] { head += 1 }
    var tail = 0
    while tail < old.count - head && tail < new.count - head
        && old[old.count - 1 - tail] == new[new.count - 1 - tail]
    {
        tail += 1
    }
    let oldMiddle = Array(old[head..<(old.count - tail)])
    let newMiddle = Array(new[head..<(new.count - tail)])

    if oldMiddle.count > maxDiffLines || newMiddle.count > maxDiffLines {
        let lines =
            before.prefix(replacementLines / 2).map { DiffLine(op: .remove, text: $0) }
            + after.prefix(replacementLines / 2).map { DiffLine(op: .add, text: $0) }
        return LineDiff(lines: lines, added: after.count, removed: before.count, truncated: true)
    }

    // lengths[i][j]: the LCS length of oldMiddle[i...] and newMiddle[j...].
    let cols = newMiddle.count + 1
    var lengths = [UInt16](repeating: 0, count: (oldMiddle.count + 1) * cols)
    var i = oldMiddle.count - 1
    while i >= 0 {
        var j = newMiddle.count - 1
        while j >= 0 {
            lengths[i * cols + j] =
                oldMiddle[i] == newMiddle[j]
                ? lengths[(i + 1) * cols + j + 1] + 1
                : Swift.max(lengths[(i + 1) * cols + j], lengths[i * cols + j + 1])
            j -= 1
        }
        i -= 1
    }

    var lines = before[..<head].map { DiffLine(op: .same, text: $0) }
    var added = 0
    var removed = 0
    i = 0
    var j = 0
    while i < oldMiddle.count || j < newMiddle.count {
        if i < oldMiddle.count && j < newMiddle.count && oldMiddle[i] == newMiddle[j] {
            lines.append(DiffLine(op: .same, text: before[head + i]))
            i += 1
            j += 1
        } else if i < oldMiddle.count
            && (j >= newMiddle.count || lengths[(i + 1) * cols + j] >= lengths[i * cols + j + 1])
        {
            // On a tie the removal goes first, as in Git: a changed line reads old, then new.
            lines.append(DiffLine(op: .remove, text: before[head + i]))
            removed += 1
            i += 1
        } else if j < newMiddle.count {
            lines.append(DiffLine(op: .add, text: after[head + j]))
            added += 1
            j += 1
        }
    }
    lines += after[(after.count - tail)...].map { DiffLine(op: .same, text: $0) }
    return LineDiff(lines: lines, added: added, removed: removed, truncated: false)
}

/// A diff line with its numbers in the old and the new text, and a key unique in its diff.
public struct NumberedLine: Sendable, Hashable, Codable {
    public var op: DiffLine.Op
    public var text: String
    public var oldLine: Int?
    public var newLine: Int?
    public var key: String
}

public func numberLines(_ lines: [DiffLine]) -> [NumberedLine] {
    var oldLine = 0
    var newLine = 0
    return lines.map { line in
        var before: Int?
        var after: Int?
        if line.op != .add {
            oldLine += 1
            before = oldLine
        }
        if line.op != .remove {
            newLine += 1
            after = newLine
        }
        let key = "\(before.map(String.init) ?? "null"):\(after.map(String.init) ?? "null"):\(line.op.rawValue)"
        return NumberedLine(op: line.op, text: line.text, oldLine: before, newLine: after, key: key)
    }
}

/// The changed lines of a hunk, without their context, as a revert counts them (`LineHunk`).
public struct LineHunk: Sendable, Hashable, Codable {
    public var oldStart: Int
    public var oldLines: Int
    public var newStart: Int
    public var newLines: Int

    enum CodingKeys: String, CodingKey {
        case oldStart = "old_start", oldLines = "old_lines", newStart = "new_start", newLines = "new_lines"
    }
}

/// A stretch of a whole file's diff: unchanged lines folded away, or a change and its context.
public enum DiffPart: Sendable, Hashable {
    case fold(from: Int, to: Int)
    case change(from: Int, to: Int, hunk: LineHunk, added: Int, removed: Int)
}

extension DiffPart: Codable {
    enum CodingKeys: String, CodingKey {
        case kind, from, to, hunk, added, removed
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let from = try container.decode(Int.self, forKey: .from)
        let to = try container.decode(Int.self, forKey: .to)
        if try container.decode(String.self, forKey: .kind) == "fold" {
            self = .fold(from: from, to: to)
        } else {
            self = .change(
                from: from, to: to, hunk: try container.decode(LineHunk.self, forKey: .hunk),
                added: try container.decode(Int.self, forKey: .added),
                removed: try container.decode(Int.self, forKey: .removed))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .fold(let from, let to):
            try container.encode("fold", forKey: .kind)
            try container.encode(from, forKey: .from)
            try container.encode(to, forKey: .to)
        case .change(let from, let to, let hunk, let added, let removed):
            try container.encode("change", forKey: .kind)
            try container.encode(from, forKey: .from)
            try container.encode(to, forKey: .to)
            try container.encode(hunk, forKey: .hunk)
            try container.encode(added, forKey: .added)
            try container.encode(removed, forKey: .removed)
        }
    }
}

/// A whole file's diff as its changes, each with up to `context` unchanged lines on either side,
/// and the unchanged stretches between them. Changes closer than twice the context and one line
/// show as one, and a fold never hides fewer than two lines.
public func changeParts(_ rows: [NumberedLine], context: Int) -> [DiffPart] {
    var groups: [(first: Int, last: Int)] = []
    for (index, row) in rows.enumerated() where row.op != .same {
        if let last = groups.last, index - last.last - 1 <= 2 * context + 1 {
            groups[groups.count - 1].last = index
        } else {
            groups.append((index, index))
        }
    }
    var parts: [DiffPart] = []
    var cursor = 0
    for (index, group) in groups.enumerated() {
        var from = Swift.max(cursor, group.first - context)
        if from - cursor < 2 { from = cursor }
        if from > cursor { parts.append(.fold(from: cursor, to: from)) }
        var to = Swift.min(rows.count, group.last + context + 1)
        if index == groups.count - 1 && rows.count - to < 2 { to = rows.count }
        var oldStart = 0
        var newStart = 0
        for row in rows[..<group.first] {
            if row.op != .add { oldStart += 1 }
            if row.op != .remove { newStart += 1 }
        }
        var oldLines = 0
        var newLines = 0
        var added = 0
        var removed = 0
        for row in rows[group.first...group.last] {
            if row.op != .add { oldLines += 1 }
            if row.op != .remove { newLines += 1 }
            if row.op == .add { added += 1 }
            if row.op == .remove { removed += 1 }
        }
        parts.append(
            .change(
                from: from, to: to,
                hunk: LineHunk(oldStart: oldStart, oldLines: oldLines, newStart: newStart, newLines: newLines),
                added: added, removed: removed))
        cursor = to
    }
    if cursor < rows.count { parts.append(.fold(from: cursor, to: rows.count)) }
    return parts
}
