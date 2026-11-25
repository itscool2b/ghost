// JavaScript's string semantics, as the desktop's TypeScript relies on them.
//
// JavaScript indexes, searches and slices strings by UTF-16 code unit and compares them code unit
// by code unit. Swift walks strings by grapheme ("\r\n" is one Character, and so is "e" with a
// combining accent) and compares them by Unicode canonical equivalence ("e\u{301}" == "\u{e9}"),
// so this module never uses `Character`, `==` on text it must tell apart, `hasPrefix`, `contains`
// or Regex. Everything below works on `[UInt16]`, the string's UTF-16 code units.

typealias Units = [UInt16]

extension String {
    /// The string's UTF-16 code units, as JavaScript indexes them.
    var units: Units { Array(utf16) }

    init(units: some Collection<UInt16>) {
        self = String(decoding: units, as: UTF16.self)
    }

    /// Equal code unit for code unit, as JavaScript's `===`.
    func same(as other: String) -> Bool {
        utf8.elementsEqual(other.utf8)
    }
}

enum JS {
    /// ECMAScript's WhiteSpace and LineTerminator code points: what `trim()` removes and `\s`
    /// matches. U+0085 and U+180E are not among them.
    static func isSpace(_ unit: UInt16) -> Bool {
        switch unit {
        case 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x20, 0xA0, 0x1680, 0x2000...0x200A, 0x2028, 0x2029,
            0x202F, 0x205F, 0x3000, 0xFEFF:
            return true
        default:
            return false
        }
    }

    /// The line terminators a regular expression's `.` does not match.
    static func isLineTerminator(_ unit: UInt16) -> Bool {
        unit == 0x0A || unit == 0x0D || unit == 0x2028 || unit == 0x2029
    }

    /// `\d`: an ASCII digit.
    static func isDigit(_ unit: UInt16) -> Bool { unit >= 0x30 && unit <= 0x39 }

    static func isAsciiUpper(_ unit: UInt16) -> Bool { unit >= 0x41 && unit <= 0x5A }
    static func isAsciiLower(_ unit: UInt16) -> Bool { unit >= 0x61 && unit <= 0x7A }
    static func isAsciiLetter(_ unit: UInt16) -> Bool { isAsciiUpper(unit) || isAsciiLower(unit) }

    /// `\w`: an ASCII letter, digit or underscore.
    static func isWord(_ unit: UInt16) -> Bool { isAsciiLetter(unit) || isDigit(unit) || unit == 0x5F }

    static func isSurrogate(_ unit: UInt16) -> Bool { unit >= 0xD800 && unit <= 0xDFFF }

    // MARK: Search

    /// `string.indexOf(search, from)`.
    static func indexOf(_ string: Units, _ search: Units, from: Int = 0) -> Int {
        let start = Swift.min(Swift.max(from, 0), string.count)
        if search.isEmpty { return start }
        guard string.count >= search.count else { return -1 }
        let last = string.count - search.count
        guard start <= last else { return -1 }
        let first = search[0]
        var index = start
        while index <= last {
            if string[index] == first && matches(string, search, at: index) { return index }
            index += 1
        }
        return -1
    }

    /// `string.lastIndexOf(search)`.
    static func lastIndexOf(_ string: Units, _ search: Units) -> Int {
        if search.isEmpty { return string.count }
        guard string.count >= search.count else { return -1 }
        var index = string.count - search.count
        while index >= 0 {
            if matches(string, search, at: index) { return index }
            index -= 1
        }
        return -1
    }

    /// Whether `search` occurs in `string` at `index`.
    static func matches(_ string: Units, _ search: Units, at index: Int) -> Bool {
        guard index >= 0, index + search.count <= string.count else { return false }
        var offset = 0
        while offset < search.count {
            if string[index + offset] != search[offset] { return false }
            offset += 1
        }
        return true
    }

    static func includes(_ string: Units, _ search: Units) -> Bool {
        indexOf(string, search) >= 0
    }

    static func startsWith(_ string: Units, _ search: Units) -> Bool {
        matches(string, search, at: 0)
    }

    static func endsWith(_ string: Units, _ search: Units) -> Bool {
        matches(string, search, at: string.count - search.count)
    }

    // MARK: Slicing

    /// `string.slice(start, end)`: negative positions count from the end, and an end before the
    /// start gives the empty string.
    static func slice(_ string: Units, _ start: Int, _ end: Int? = nil) -> Units {
        let count = string.count
        func clamp(_ position: Int) -> Int {
            position < 0 ? Swift.max(count + position, 0) : Swift.min(position, count)
        }
        let from = clamp(start)
        let to = clamp(end ?? count)
        return from < to ? Array(string[from..<to]) : []
    }

    /// `string.split(separator)` for a separator that is not empty.
    static func split(_ string: Units, _ separator: Units) -> [Units] {
        precondition(!separator.isEmpty)
        var parts: [Units] = []
        var from = 0
        while true {
            let at = indexOf(string, separator, from: from)
            if at < 0 {
                parts.append(Array(string[from...]))
                return parts
            }
            parts.append(Array(string[from..<at]))
            from = at + separator.count
        }
    }

    static func join(_ parts: [Units], _ separator: Units) -> Units {
        var out: Units = []
        for (index, part) in parts.enumerated() {
            if index > 0 { out += separator }
            out += part
        }
        return out
    }

    // MARK: Trimming

    static func trim(_ string: Units) -> Units { trimEnd(trimStart(string)) }

    static func trimStart(_ string: Units) -> Units {
        var from = 0
        while from < string.count && isSpace(string[from]) { from += 1 }
        return from == 0 ? string : Array(string[from...])
    }

    static func trimEnd(_ string: Units) -> Units {
        var to = string.count
        while to > 0 && isSpace(string[to - 1]) { to -= 1 }
        return to == string.count ? string : Array(string[..<to])
    }

    /// `/\s/.test(string)`.
    static func hasSpace(_ string: Units) -> Bool { string.contains(where: isSpace) }

    // MARK: Case

    /// `string.toLowerCase()`: Unicode's full lowercase mappings, with the one rule that depends on
    /// context, Final_Sigma (Swift's `lowercased()` leaves it out).
    static func lowercased(_ text: String) -> String {
        let scalars = Array(text.unicodeScalars)
        var out = String.UnicodeScalarView()
        for (index, scalar) in scalars.enumerated() {
            if scalar.value == 0x03A3 {
                out.append(finalSigma(scalars, index) ? "\u{03C2}" : "\u{03C3}")
            } else {
                out.append(contentsOf: scalar.properties.lowercaseMapping.unicodeScalars)
            }
        }
        return String(out)
    }

    /// A capital sigma ends a word when a cased letter comes before it and none after it, case
    /// ignorable characters skipped on both sides.
    private static func finalSigma(_ scalars: [Unicode.Scalar], _ index: Int) -> Bool {
        var before = index - 1
        while before >= 0 && scalars[before].properties.isCaseIgnorable { before -= 1 }
        guard before >= 0, scalars[before].properties.isCased else { return false }
        var after = index + 1
        while after < scalars.count && scalars[after].properties.isCaseIgnorable { after += 1 }
        return !(after < scalars.count && scalars[after].properties.isCased)
    }

    /// `string.toUpperCase()`.
    static func uppercased(_ text: String) -> String {
        var out = String.UnicodeScalarView()
        for scalar in text.unicodeScalars {
            out.append(contentsOf: scalar.properties.uppercaseMapping.unicodeScalars)
        }
        return String(out)
    }

    /// `text.charAt(0).toUpperCase() + text.slice(1)`. A first character outside the Basic
    /// Multilingual Plane stays as it is: `charAt(0)` is half of it, which has no capital.
    static func capitalized(_ text: String) -> String {
        guard let first = text.unicodeScalars.first else { return text }
        if first.value > 0xFFFF { return text }
        var out = String.UnicodeScalarView()
        out.append(contentsOf: first.properties.uppercaseMapping.unicodeScalars)
        out.append(contentsOf: text.unicodeScalars.dropFirst())
        return String(out)
    }

    /// Whether `string` starts with `prefix`, ASCII letters compared without case: a regular
    /// expression's `i` flag outside Unicode mode, for an ASCII pattern.
    static func startsWithIgnoringAsciiCase(_ string: Units, _ prefix: Units) -> Bool {
        guard string.count >= prefix.count else { return false }
        for (index, unit) in prefix.enumerated() where foldAscii(string[index]) != foldAscii(unit) {
            return false
        }
        return true
    }

    static func foldAscii(_ unit: UInt16) -> UInt16 { isAsciiUpper(unit) ? unit + 0x20 : unit }

    /// UTF-16 code unit order, as `Array.prototype.sort()` compares strings.
    static func codeUnitsPrecede(_ a: String, _ b: String) -> Bool {
        a.utf16.lexicographicallyPrecedes(b.utf16)
    }
}

/// A string key compared code unit by code unit, as a JavaScript `Map` or `Set` compares it.
struct ExactKey: Hashable, Sendable {
    let text: String

    init(_ text: String) { self.text = text }

    static func == (lhs: ExactKey, rhs: ExactKey) -> Bool { lhs.text.same(as: rhs.text) }

    func hash(into hasher: inout Hasher) {
        var text = text
        text.withUTF8 { hasher.combine(bytes: UnsafeRawBufferPointer($0)) }
        hasher.combine(text.utf8.count)
    }
}

/// A set of strings compared as JavaScript's `Set` compares them.
struct ExactSet: Sendable {
    private var members: Set<ExactKey> = []

    init() {}

    init(_ strings: some Sequence<String>) {
        for string in strings { members.insert(ExactKey(string)) }
    }

    mutating func insert(_ string: String) { members.insert(ExactKey(string)) }

    func contains(_ string: String) -> Bool { members.contains(ExactKey(string)) }

    var count: Int { members.count }
}
