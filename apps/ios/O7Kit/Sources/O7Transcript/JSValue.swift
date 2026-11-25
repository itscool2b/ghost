internal import O7Wire

// JSON as JavaScript holds it, for `JSON.parse` and `JSON.stringify(value, null, 2)`:
//   - strings are UTF-16 code units, so an escaped lone surrogate survives a round trip;
//   - an object keeps JavaScript's property order: keys that are array indices first, in
//     ascending order, then the others in the order they were first set;
//   - a repeated key keeps its first place and takes its last value.
//
// O7Wire's `JSONValue` keeps objects in a Swift dictionary, which has no order: Foundation's
// JSONDecoder does not report the order keys arrived in. Its objects are read here with their
// keys sorted by code unit (`JSValue(_:)`), a documented divergence from the desktop, which lists
// a tool's input in the order the agent wrote it.

indirect enum JSValue: Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(Units)
    case array([JSValue])
    case object(JSObject)

    /// A wire value, its objects' keys in code unit order.
    init(_ value: JSONValue) {
        switch value {
        case .null: self = .null
        case .bool(let flag): self = .bool(flag)
        case .number(let number): self = .number(number)
        case .string(let text): self = .string(text.units)
        case .array(let items): self = .array(items.map(JSValue.init))
        case .object(let fields):
            var object = JSObject()
            for key in fields.keys.sorted(by: JS.codeUnitsPrecede) {
                object.set(key.units, JSValue(fields[key] ?? .null))
            }
            self = .object(object)
        }
    }

    subscript(key: String) -> JSValue? {
        if case .object(let object) = self { return object[key.units] }
        return nil
    }

    var stringValue: String? {
        if case .string(let units) = self { return String(units: units) }
        return nil
    }
}

struct JSObject: Equatable, Sendable {
    private var keys: [Units] = []
    private var values: [JSValue] = []
    private var index: [Units: Int] = [:]

    init() {}

    subscript(key: Units) -> JSValue? {
        index[key].map { values[$0] }
    }

    mutating func set(_ key: Units, _ value: JSValue) {
        if let at = index[key] {
            values[at] = value
        } else {
            index[key] = keys.count
            keys.append(key)
            values.append(value)
        }
    }

    var isEmpty: Bool { keys.isEmpty }

    /// The entries in JavaScript's property order.
    var entries: [(key: Units, value: JSValue)] {
        var indices: [(UInt32, Int)] = []
        var others: [Int] = []
        for (at, key) in keys.enumerated() {
            if let number = Self.arrayIndex(key) { indices.append((number, at)) } else { others.append(at) }
        }
        indices.sort { $0.0 < $1.0 }
        return (indices.map(\.1) + others).map { (keys[$0], values[$0]) }
    }

    /// The array index a key names: a canonical decimal integer below 2^32 - 1.
    static func arrayIndex(_ key: Units) -> UInt32? {
        guard !key.isEmpty, key.count <= 10, key.allSatisfy(JS.isDigit) else { return nil }
        if key.count > 1 && key[0] == 0x30 { return nil }
        var value: UInt64 = 0
        for unit in key { value = value * 10 + UInt64(unit - 0x30) }
        return value < 0xFFFF_FFFF ? UInt32(value) : nil
    }
}

// MARK: JSON.parse

extension JSValue {
    /// The most nesting a parse follows. V8 parses deeper, then its stringify runs out of stack at a
    /// depth that depends on the machine; past this the port gives up as if the text were not JSON.
    static let maxDepth = 1000

    /// `JSON.parse(text)`, or nil where it throws.
    static func parse(_ text: Units) -> JSValue? {
        var parser = JSONParser(text: text)
        parser.skipSpace()
        guard let value = parser.value(depth: 0) else { return nil }
        parser.skipSpace()
        return parser.at == text.count ? value : nil
    }
}

private struct JSONParser {
    let text: Units
    var at = 0

    mutating func skipSpace() {
        while at < text.count {
            switch text[at] {
            case 0x20, 0x09, 0x0A, 0x0D: at += 1
            default: return
            }
        }
    }

    mutating func value(depth: Int) -> JSValue? {
        guard at < text.count, depth <= JSValue.maxDepth else { return nil }
        switch text[at] {
        case 0x7B: return object(depth: depth)
        case 0x5B: return array(depth: depth)
        case 0x22: return string().map(JSValue.string)
        case 0x74: return literal("true", .bool(true))
        case 0x66: return literal("false", .bool(false))
        case 0x6E: return literal("null", .null)
        default: return number()
        }
    }

    mutating func literal(_ word: String, _ value: JSValue) -> JSValue? {
        let units = word.units
        guard JS.matches(text, units, at: at) else { return nil }
        at += units.count
        return value
    }

    mutating func object(depth: Int) -> JSValue? {
        at += 1
        var object = JSObject()
        skipSpace()
        if at < text.count && text[at] == 0x7D {
            at += 1
            return .object(object)
        }
        while true {
            skipSpace()
            guard at < text.count, text[at] == 0x22, let key = string() else { return nil }
            skipSpace()
            guard at < text.count, text[at] == 0x3A else { return nil }
            at += 1
            skipSpace()
            guard let value = value(depth: depth + 1) else { return nil }
            object.set(key, value)
            skipSpace()
            guard at < text.count else { return nil }
            if text[at] == 0x2C {
                at += 1
            } else if text[at] == 0x7D {
                at += 1
                return .object(object)
            } else {
                return nil
            }
        }
    }

    mutating func array(depth: Int) -> JSValue? {
        at += 1
        var items: [JSValue] = []
        skipSpace()
        if at < text.count && text[at] == 0x5D {
            at += 1
            return .array(items)
        }
        while true {
            skipSpace()
            guard let item = value(depth: depth + 1) else { return nil }
            items.append(item)
            skipSpace()
            guard at < text.count else { return nil }
            if text[at] == 0x2C {
                at += 1
            } else if text[at] == 0x5D {
                at += 1
                return .array(items)
            } else {
                return nil
            }
        }
    }

    mutating func string() -> Units? {
        at += 1
        var out: Units = []
        while at < text.count {
            let unit = text[at]
            at += 1
            switch unit {
            case 0x22: return out
            case 0x5C:
                guard at < text.count else { return nil }
                let escape = text[at]
                at += 1
                switch escape {
                case 0x22, 0x5C, 0x2F: out.append(escape)
                case 0x62: out.append(0x08)
                case 0x66: out.append(0x0C)
                case 0x6E: out.append(0x0A)
                case 0x72: out.append(0x0D)
                case 0x74: out.append(0x09)
                case 0x75:
                    guard at + 4 <= text.count else { return nil }
                    var code: UInt16 = 0
                    for _ in 0..<4 {
                        guard let digit = hexDigit(text[at]) else { return nil }
                        code = code << 4 | digit
                        at += 1
                    }
                    out.append(code)
                default: return nil
                }
            default:
                guard unit >= 0x20 else { return nil }
                out.append(unit)
            }
        }
        return nil
    }

    func hexDigit(_ unit: UInt16) -> UInt16? {
        switch unit {
        case 0x30...0x39: return unit - 0x30
        case 0x61...0x66: return unit - 0x61 + 10
        case 0x41...0x46: return unit - 0x41 + 10
        default: return nil
        }
    }

    /// `-? (0 | [1-9][0-9]*) (. [0-9]+)? ([eE] [+-]? [0-9]+)?`
    mutating func number() -> JSValue? {
        let start = at
        if at < text.count && text[at] == 0x2D { at += 1 }
        guard at < text.count, JS.isDigit(text[at]) else { return nil }
        if text[at] == 0x30 {
            at += 1
        } else {
            while at < text.count && JS.isDigit(text[at]) { at += 1 }
        }
        if at < text.count && text[at] == 0x2E {
            at += 1
            guard at < text.count, JS.isDigit(text[at]) else { return nil }
            while at < text.count && JS.isDigit(text[at]) { at += 1 }
        }
        if at < text.count && (text[at] == 0x65 || text[at] == 0x45) {
            at += 1
            if at < text.count && (text[at] == 0x2B || text[at] == 0x2D) { at += 1 }
            guard at < text.count, JS.isDigit(text[at]) else { return nil }
            while at < text.count && JS.isDigit(text[at]) { at += 1 }
        }
        let literal = String(units: text[start..<at])
        return Double(literal).map(JSValue.number)
    }
}

// MARK: JSON.stringify

extension JSValue {
    /// `JSON.stringify(value, null, indent)`, or the compact form without an indent.
    func stringified(indent: Int? = nil) -> Units {
        var out: Units = []
        write(into: &out, indent: indent, level: 0)
        return out
    }

    private func write(into out: inout Units, indent: Int?, level: Int) {
        switch self {
        case .null: out += "null".units
        case .bool(let flag): out += (flag ? "true" : "false").units
        case .number(let number):
            out += (number.isFinite ? JS.numberString(number) : "null").units
        case .string(let units): Self.quote(units, into: &out)
        case .array(let items):
            if items.isEmpty {
                out += "[]".units
                return
            }
            out.append(0x5B)
            for (index, item) in items.enumerated() {
                if index > 0 { out.append(0x2C) }
                Self.newline(into: &out, indent: indent, level: level + 1)
                item.write(into: &out, indent: indent, level: level + 1)
            }
            Self.newline(into: &out, indent: indent, level: level)
            out.append(0x5D)
        case .object(let object):
            let entries = object.entries
            if entries.isEmpty {
                out += "{}".units
                return
            }
            out.append(0x7B)
            for (index, entry) in entries.enumerated() {
                if index > 0 { out.append(0x2C) }
                Self.newline(into: &out, indent: indent, level: level + 1)
                Self.quote(entry.key, into: &out)
                out.append(0x3A)
                if indent != nil { out.append(0x20) }
                entry.value.write(into: &out, indent: indent, level: level + 1)
            }
            Self.newline(into: &out, indent: indent, level: level)
            out.append(0x7D)
        }
    }

    private static func newline(into out: inout Units, indent: Int?, level: Int) {
        guard let indent else { return }
        out.append(0x0A)
        out += Units(repeating: 0x20, count: indent * level)
    }

    /// JSON.stringify's QuoteJSONString: the short escapes, other controls and lone surrogates as
    /// `\uXXXX` in lowercase hex, everything else as it is.
    static func quote(_ units: Units, into out: inout Units) {
        out.append(0x22)
        var index = 0
        while index < units.count {
            let unit = units[index]
            switch unit {
            case 0x08: out += [0x5C, 0x62]
            case 0x09: out += [0x5C, 0x74]
            case 0x0A: out += [0x5C, 0x6E]
            case 0x0C: out += [0x5C, 0x66]
            case 0x0D: out += [0x5C, 0x72]
            case 0x22: out += [0x5C, 0x22]
            case 0x5C: out += [0x5C, 0x5C]
            case 0xD800...0xDBFF:
                if index + 1 < units.count, (0xDC00...0xDFFF).contains(units[index + 1]) {
                    out += [unit, units[index + 1]]
                    index += 1
                } else {
                    escape(unit, into: &out)
                }
            case 0xDC00...0xDFFF: escape(unit, into: &out)
            default:
                if unit < 0x20 { escape(unit, into: &out) } else { out.append(unit) }
            }
            index += 1
        }
        out.append(0x22)
    }

    private static func escape(_ unit: UInt16, into out: inout Units) {
        let hex = String(unit, radix: 16)
        out += "\\u".units + Units(repeating: 0x30, count: 4 - hex.utf16.count) + hex.units
    }
}
