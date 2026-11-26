/// Reads highlight.js's HTML strictly: `<span class="...">`, `</span>`, text and the entities
/// highlight.js writes (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#x27;`, plus numeric ones). Any other
/// markup, a stray `<` or `&`, an unknown entity or an unbalanced span fails the whole parse, so
/// nothing but colour ever comes out of the JavaScript side.
enum HighlightHTML {
    /// The decoded text as UTF-16 code units and its runs, or nil when the HTML is not exactly
    /// what highlight.js writes.
    static func parse(_ html: String) -> (text: [UInt16], runs: [HighlightRun])? {
        parse(units: Array(html.utf16))
    }

    static func parse(units: [UInt16]) -> (text: [UInt16], runs: [HighlightRun])? {
        var text: [UInt16] = []
        text.reserveCapacity(units.count)
        var stack: [String] = []
        var runs: [HighlightRun] = []
        var runStart = 0
        var runScopes: [String] = []
        var stackChanged = false

        func append(_ unit: UInt16) {
            if stackChanged {
                if stack != runScopes {
                    if text.count > runStart {
                        runs.append(HighlightRun(range: runStart..<text.count, scopes: runScopes))
                    }
                    runStart = text.count
                    runScopes = stack
                }
                stackChanged = false
            }
            text.append(unit)
        }

        var at = 0
        while at < units.count {
            let unit = units[at]
            if unit == Unit.less {
                if starts(units, at, with: closeTag) {
                    guard !stack.isEmpty else { return nil }
                    stack.removeLast()
                    stackChanged = true
                    at += closeTag.count
                    continue
                }
                guard starts(units, at, with: openTag) else { return nil }
                let classStart = at + openTag.count
                var end = classStart
                while end < units.count, units[end] != Unit.quote {
                    guard isClassUnit(units[end]) else { return nil }
                    end += 1
                }
                guard end > classStart, end + 1 < units.count, units[end + 1] == Unit.greater,
                    let scope = scope(ofClass: String(decoding: units[classStart..<end], as: UTF16.self))
                else { return nil }
                stack.append(scope)
                stackChanged = true
                at = end + 2
            } else if unit == Unit.ampersand {
                guard let (decoded, length) = entity(units, at) else { return nil }
                for piece in decoded { append(piece) }
                at += length
            } else {
                append(unit)
                at += 1
            }
        }
        guard stack.isEmpty else { return nil }
        if text.count > runStart {
            runs.append(HighlightRun(range: runStart..<text.count, scopes: runScopes))
        }
        return (text, snapped(runs, in: text))
    }

    /// A span's class as a highlight.js scope, the inverse of its `scopeToCSSClass`:
    /// `hljs-title class_ inherited__` is `title.class.inherited`, `language-xml` is `language:xml`.
    static func scope(ofClass value: String) -> String? {
        let pieces = value.split(separator: " ", omittingEmptySubsequences: false)
        guard let first = pieces.first, !first.isEmpty else { return nil }
        if first.hasPrefix("language-") {
            guard pieces.count == 1, first.count > 9 else { return nil }
            return "language:" + first.dropFirst(9)
        }
        guard first.hasPrefix("hljs-"), first.count > 5 else { return nil }
        var parts = [String(first.dropFirst(5))]
        for (index, piece) in pieces.dropFirst().enumerated() {
            let tail = index + 1
            guard piece.count > tail, piece.hasSuffix(String(repeating: "_", count: tail)) else { return nil }
            parts.append(String(piece.dropLast(tail)))
        }
        return parts.joined(separator: ".")
    }

    /// The entity at `at` (which holds `&`) as UTF-16 and its length, or nil when it is not one
    /// highlight.js could have written.
    private static func entity(_ units: [UInt16], _ at: Int) -> ([UInt16], Int)? {
        var end = at + 1
        while end < units.count, end - at <= 10, units[end] != Unit.semicolon { end += 1 }
        guard end < units.count, units[end] == Unit.semicolon, end > at + 1 else { return nil }
        let name = String(decoding: units[(at + 1)..<end], as: UTF16.self)
        let length = end - at + 1
        switch name {
        case "amp": return ([Unit.ampersand], length)
        case "lt": return ([Unit.less], length)
        case "gt": return ([Unit.greater], length)
        case "quot": return ([Unit.quote], length)
        default: break
        }
        guard name.hasPrefix("#") else { return nil }
        let number = name.dropFirst()
        let value: UInt32?
        if number.hasPrefix("x") || number.hasPrefix("X") {
            let digits = number.dropFirst()
            value = digits.allSatisfy(\.isHexDigit) ? UInt32(digits, radix: 16) : nil
        } else {
            value = number.allSatisfy { $0.isASCII && $0.isNumber } ? UInt32(number) : nil
        }
        guard let value, let scalar = Unicode.Scalar(value) else { return nil }
        return (Array(String(Character(scalar)).utf16), length)
    }

    /// Runs whose boundaries never split a surrogate pair: a low surrogate stays with its high
    /// one. Emptied runs go and neighbours left with the same scopes merge.
    private static func snapped(_ runs: [HighlightRun], in text: [UInt16]) -> [HighlightRun] {
        var result: [HighlightRun] = []
        var start = 0
        for (index, run) in runs.enumerated() {
            var end = run.range.upperBound
            if index < runs.count - 1, end > 0, end < text.count,
                UTF16.isLeadSurrogate(text[end - 1]), UTF16.isTrailSurrogate(text[end])
            {
                end += 1
            }
            end = min(max(end, start), text.count)
            guard end > start else { continue }
            if let last = result.last, last.scopes == run.scopes {
                result[result.count - 1] = HighlightRun(range: last.range.lowerBound..<end, scopes: run.scopes)
            } else {
                result.append(HighlightRun(range: start..<end, scopes: run.scopes))
            }
            start = end
        }
        return result
    }

    private static func starts(_ units: [UInt16], _ at: Int, with prefix: [UInt16]) -> Bool {
        guard at + prefix.count <= units.count else { return false }
        for (offset, unit) in prefix.enumerated() where units[at + offset] != unit { return false }
        return true
    }

    /// The characters a class may hold: letters, digits, `_`, `-` and the space between classes.
    private static func isClassUnit(_ unit: UInt16) -> Bool {
        switch unit {
        case 0x30...0x39, 0x41...0x5a, 0x61...0x7a, 0x5f, 0x2d, 0x20: true
        default: false
        }
    }

    private static let openTag = Array(#"<span class=""#.utf16)
    private static let closeTag = Array("</span>".utf16)

    private enum Unit {
        static let less: UInt16 = 0x3c
        static let greater: UInt16 = 0x3e
        static let ampersand: UInt16 = 0x26
        static let quote: UInt16 = 0x22
        static let semicolon: UInt16 = 0x3b
    }
}
