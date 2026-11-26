import Foundation
import GhostWire
import Testing

@testable import GhostTranscript

// What every golden test shares: the files, JSON comparison, and the encodings the writer uses
// for what JSON cannot hold (apps/ios/scripts/write-goldens.mts, `enc`):
//   { "$map": [[key, value]…] }  a JavaScript Map, in its order
//   { "$set": […] }              a Set
//   { "$": "NaN" }               NaN, Infinity, -Infinity, and -0 in arguments

/// One golden file, named by its file for the test report.
struct Golden: CustomTestStringConvertible, Sendable {
    let url: URL
    var testDescription: String { url.deletingPathExtension().lastPathComponent }

    static let all: [Golden] = {
        guard let folder = Bundle.module.url(forResource: "Goldens", withExtension: nil),
            let names = try? FileManager.default.contentsOfDirectory(atPath: folder.path)
        else { return [] }
        return names.filter { $0.hasSuffix(".json") }.sorted().map { Golden(url: folder.appendingPathComponent($0)) }
    }()

    func json() throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
    }
}

struct GoldenError: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

// MARK: Comparison

/// The first place `actual` and `expected` differ, as a path and both values, or nil when they
/// are equal. A missing field equals null; numbers compare as doubles; strings code unit by code
/// unit (Swift's `==` would call canonically equivalent text equal).
func firstDifference(_ actual: JSONValue?, _ expected: JSONValue?, at path: String = "$") -> String? {
    let left = actual ?? .null
    let right = expected ?? .null
    switch (left, right) {
    case (.object(let a), .object(let b)):
        for key in Set(a.keys).union(b.keys).sorted() {
            if let difference = firstDifference(a[key], b[key], at: "\(path).\(key)") { return difference }
        }
        return nil
    case (.array(let a), .array(let b)):
        for index in 0..<min(a.count, b.count) {
            if let difference = firstDifference(a[index], b[index], at: "\(path)[\(index)]") { return difference }
        }
        return a.count == b.count ? nil : "\(path): \(a.count) items, expected \(b.count)"
    case (.number(let a), .number(let b)):
        return a == b || (a.isNaN && b.isNaN) ? nil : "\(path): \(a), expected \(b)"
    case (.string(let a), .string(let b)):
        return a.utf8.elementsEqual(b.utf8) ? nil : "\(path): \(quoted(a)), expected \(quoted(b))"
    default:
        return left == right ? nil : "\(path): \(brief(left)), expected \(brief(right))"
    }
}

private func quoted(_ text: String) -> String {
    let shown = text.unicodeScalars.map { scalar -> String in
        scalar.value < 0x20 || (scalar.value >= 0x7F && scalar.value < 0xA0) || scalar.properties.isDefaultIgnorableCodePoint
            ? "\\u{\(String(scalar.value, radix: 16))}" : String(scalar)
    }.joined()
    return "\"\(shown.count > 200 ? String(shown.prefix(200)) + "…" : shown)\""
}

private func brief(_ value: JSONValue) -> String {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
    let text = (try? encoder.encode(value)).map { String(decoding: $0, as: UTF8.self) } ?? "?"
    return text.count > 300 ? String(text.prefix(300)) + "…" : text
}

// MARK: Values in and out

extension JSONValue {
    /// A value of an encodable type, as JSON, with what JSON cannot hold written as the writer does.
    static func golden(_ value: some Encodable) throws -> JSONValue {
        try JSONValue.of(value)
    }

    /// A number as the writer gives it: plain, or `{ "$": "NaN" }` and its kind.
    static func golden(_ number: Double?) -> JSONValue {
        guard let number else { return .null }
        if number.isNaN { return .object(["$": .string("NaN")]) }
        if number.isInfinite { return .object(["$": .string(number > 0 ? "Infinity" : "-Infinity")]) }
        return .number(number)
    }

    static func golden(_ text: String?) -> JSONValue { text.map(JSONValue.string) ?? .null }

    /// An insertion-ordered map as the writer gives a JavaScript Map.
    static func goldenMap<Value>(_ map: OrderedMap<Value>, _ value: (Value) throws -> JSONValue) rethrows -> JSONValue {
        .object(["$map": .array(try map.map { .array([.string($0.key), try value($0.value)]) })])
    }

    /// The argument's typed value.
    func arg<T: Decodable>(_ type: T.Type = T.self) throws -> T {
        try decode(T.self)
    }

    /// An optional argument: null is nil.
    func optional<T: Decodable>(_ type: T.Type = T.self) throws -> T? {
        isNull ? nil : try decode(T.self)
    }

    /// A numeric argument, special values included.
    var double: Double {
        get throws {
            switch self {
            case .number(let number): return number
            case .object(let object):
                switch object["$"]?.string {
                case "NaN": return .nan
                case "Infinity": return .infinity
                case "-Infinity": return -.infinity
                case "-0": return -0.0
                default: break
                }
            default: break
            }
            throw GoldenError("not a number: \(self)")
        }
    }

    var optionalDouble: Double? {
        get throws { isNull ? nil : try double }
    }

    var text: String {
        get throws {
            guard let string else { throw GoldenError("not a string: \(self)") }
            return string
        }
    }

    var flag: Bool {
        get throws {
            guard let bool else { throw GoldenError("not a boolean: \(self)") }
            return bool
        }
    }

    var int: Int {
        get throws { Int(try double) }
    }

    var items: [JSONValue] {
        get throws {
            guard case .array(let items) = self else { throw GoldenError("not an array: \(self)") }
            return items
        }
    }

    /// A `$map` argument as an insertion-ordered map.
    func map<Value: Decodable>(_ type: Value.Type = Value.self) throws -> OrderedMap<Value> {
        guard case .array(let pairs) = self["$map"] ?? .null else { throw GoldenError("not a map: \(self)") }
        var map = OrderedMap<Value>()
        for pair in pairs {
            let parts = try pair.items
            map[try parts[0].text] = try parts[1].decode(Value.self)
        }
        return map
    }

    /// A `$set` argument.
    var stringSet: Set<String> {
        get throws {
            guard case .array(let members) = self["$set"] ?? .null else { throw GoldenError("not a set: \(self)") }
            return Set(try members.map { try $0.text })
        }
    }
}
