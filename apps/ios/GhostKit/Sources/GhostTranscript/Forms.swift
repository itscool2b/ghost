public import Foundation
public import GhostWire

// An MCP server's form (MCP elicitation, form mode) while it is filled in, and the answer it sends
// (`forms.ts`). The checks mirror `ghost_protocol::forms`, which the session runs again before anything
// reaches the agent, so a mistake shows at its field instead of as a refused answer. A form's
// fields arrive as `UiPrompt.form`'s raw JSON; `UiField.fields(_:)` reads them.

public enum UiTextFormat: String, Sendable, Hashable, Codable, CaseIterable {
    case email, uri, date
    case dateTime = "date_time"
}

public enum UiFieldKind: Sendable, Hashable {
    case text(format: UiTextFormat?, minLength: Int?, maxLength: Int?, defaultText: String?)
    case number(integer: Bool, minimum: Double?, maximum: Double?, defaultNumber: Double?)
    case boolean(defaultValue: Bool?)
    /// One choice or, with `multiple`, any number within the bounds. Answers name choices by
    /// position, so shortening a label for display never changes what is sent.
    case choice(options: [String], multiple: Bool, minItems: Int?, maxItems: Int?, defaultSelected: [Int])
}

/// One field of a form. `id` is the schema property's name, which answers use.
public struct UiField: Sendable, Hashable, Identifiable {
    public var id: String
    public var label: String
    public var description: String?
    public var required: Bool
    public var kind: UiFieldKind

    public init(id: String, label: String, description: String? = nil, required: Bool, kind: UiFieldKind) {
        self.id = id
        self.label = label
        self.description = description
        self.required = required
        self.kind = kind
    }

    /// A form prompt's fields, as `UiPrompt.form` keeps them.
    public static func fields(_ json: JSONValue) -> [UiField] {
        (try? json.decode([UiField].self)) ?? []
    }
}

extension UiField: Codable {
    enum CodingKeys: String, CodingKey {
        case id, label, description, required, kind
    }

    enum KindKeys: String, CodingKey {
        case type, format, options, multiple, integer, minimum, maximum
        case minLength = "min_length", maxLength = "max_length", minItems = "min_items"
        case maxItems = "max_items", defaultValue = "default"
    }

    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(String.self, forKey: .id)
        label = try container.decodeIfPresent(String.self, forKey: .label) ?? id
        description = try container.decodeIfPresent(String.self, forKey: .description)
        required = try container.decodeIfPresent(Bool.self, forKey: .required) ?? false
        let kind = try container.nestedContainer(keyedBy: KindKeys.self, forKey: .kind)
        switch try kind.decode(String.self, forKey: .type) {
        case "number":
            self.kind = .number(
                integer: try kind.decodeIfPresent(Bool.self, forKey: .integer) ?? false,
                minimum: try kind.decodeIfPresent(Double.self, forKey: .minimum),
                maximum: try kind.decodeIfPresent(Double.self, forKey: .maximum),
                defaultNumber: try kind.decodeIfPresent(Double.self, forKey: .defaultValue))
        case "boolean":
            self.kind = .boolean(defaultValue: try kind.decodeIfPresent(Bool.self, forKey: .defaultValue))
        case "choice":
            self.kind = .choice(
                options: try kind.decodeIfPresent([String].self, forKey: .options) ?? [],
                multiple: try kind.decodeIfPresent(Bool.self, forKey: .multiple) ?? false,
                minItems: try kind.decodeIfPresent(Int.self, forKey: .minItems),
                maxItems: try kind.decodeIfPresent(Int.self, forKey: .maxItems),
                defaultSelected: try kind.decodeIfPresent([Int].self, forKey: .defaultValue) ?? [])
        default:
            // A format this copy does not know is read as none.
            self.kind = .text(
                format: try? kind.decodeIfPresent(UiTextFormat.self, forKey: .format),
                minLength: try kind.decodeIfPresent(Int.self, forKey: .minLength),
                maxLength: try kind.decodeIfPresent(Int.self, forKey: .maxLength),
                defaultText: try kind.decodeIfPresent(String.self, forKey: .defaultValue))
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(id, forKey: .id)
        try container.encode(label, forKey: .label)
        try container.encode(description, forKey: .description)
        try container.encode(required, forKey: .required)
        var kind = container.nestedContainer(keyedBy: KindKeys.self, forKey: .kind)
        switch self.kind {
        case .text(let format, let minLength, let maxLength, let defaultText):
            try kind.encode("text", forKey: .type)
            try kind.encode(format, forKey: .format)
            try kind.encode(minLength, forKey: .minLength)
            try kind.encode(maxLength, forKey: .maxLength)
            try kind.encode(defaultText, forKey: .defaultValue)
        case .number(let integer, let minimum, let maximum, let defaultNumber):
            try kind.encode("number", forKey: .type)
            try kind.encode(integer, forKey: .integer)
            try kind.encode(minimum, forKey: .minimum)
            try kind.encode(maximum, forKey: .maximum)
            try kind.encode(defaultNumber, forKey: .defaultValue)
        case .boolean(let defaultValue):
            try kind.encode("boolean", forKey: .type)
            try kind.encode(defaultValue, forKey: .defaultValue)
        case .choice(let options, let multiple, let minItems, let maxItems, let defaultSelected):
            try kind.encode("choice", forKey: .type)
            try kind.encode(options, forKey: .options)
            try kind.encode(multiple, forKey: .multiple)
            try kind.encode(minItems, forKey: .minItems)
            try kind.encode(maxItems, forKey: .maxItems)
            try kind.encode(defaultSelected, forKey: .defaultValue)
        }
    }
}

/// An answered field's value. A field left empty has no value at all.
public enum UiValue: Sendable, Hashable {
    case text(String)
    case number(Double?)
    case boolean(Bool)
    /// Positions in the field's options, each at most once.
    case choices([Int])

    var json: JSONValue {
        switch self {
        case .text(let text): .object(["type": .string("text"), "text": .string(text)])
        case .number(let number): .object(["type": .string("number"), "number": number.map(JSONValue.number) ?? .null])
        case .boolean(let value): .object(["type": .string("boolean"), "value": .bool(value)])
        case .choices(let selected):
            .object(["type": .string("choices"), "selected": .array(selected.map { .number(Double($0)) })])
        }
    }

    /// A value as the wire gives it, or nil for one this copy cannot read.
    init?(json: JSONValue) {
        switch json["type"]?.string {
        case "text": self = .text(json["text"]?.string ?? "")
        case "number": self = .number(json["number"]?.number)
        case "boolean": self = .boolean(json["value"]?.bool ?? false)
        case "choices":
            guard case .array(let items) = json["selected"] ?? .array([]) else { return nil }
            self = .choices(items.compactMap { $0.number.map { Int($0) } })
        default: return nil
        }
    }
}

public struct UiFieldValue: Sendable, Hashable {
    public var id: String
    public var value: UiValue

    public init(id: String, value: UiValue) {
        self.id = id
        self.value = value
    }

    var json: JSONValue { .object(["id": .string(id), "value": value.json]) }
}

/// Text and number fields hold what was typed; a flag holds nil until touched.
public enum FieldDraft: Sendable, Hashable {
    case text(String)
    case flag(Bool?)
    case choices([Int])
}

/// `2026-11-05T08:30:00.000Z` as a `datetime-local` input shows it, in local time.
public func localDateTime(_ iso: String, timeZone: TimeZone = .current) -> String {
    let time = JSDate.parse(iso, timeZone: timeZone)
    if time.isNaN { return "" }
    let (year, month, day, hour, minute) = JSDate.localParts(time, timeZone: timeZone)
    func pad(_ value: Int64) -> String { value < 10 && value >= 0 ? "0\(value)" : String(value) }
    return "\(year)-\(pad(month))-\(pad(day))T\(pad(hour)):\(pad(minute))"
}

/// Each field starts at the server's default, if it gave one.
public func initialDrafts(_ fields: [UiField], timeZone: TimeZone = .current) -> OrderedMap<FieldDraft> {
    var drafts = OrderedMap<FieldDraft>()
    for field in fields {
        switch field.kind {
        case .text(let format, _, _, let defaultText):
            if format == .dateTime, let defaultText, !defaultText.isEmpty {
                drafts[field.id] = .text(localDateTime(defaultText, timeZone: timeZone))
            } else {
                drafts[field.id] = .text(defaultText ?? "")
            }
        case .number(_, _, _, let defaultNumber):
            drafts[field.id] = .text(defaultNumber.map(JS.numberString) ?? "")
        case .boolean(let defaultValue):
            drafts[field.id] = .flag(defaultValue)
        case .choice(_, _, _, _, let defaultSelected):
            drafts[field.id] = .choices(defaultSelected)
        }
    }
    return drafts
}

/// A `datetime-local` value as the UTC moment it names; text that is no date stays as typed.
private func isoMoment(_ local: String, timeZone: TimeZone) -> String {
    let time = JSDate.parse(local, timeZone: timeZone)
    return time.isNaN ? local : JSDate.isoString(time)
}

public func acceptsFormat(_ format: UiTextFormat, _ text: String, timeZone: TimeZone = .current) -> Bool {
    let units = text.units
    switch format {
    case .email: return isEmail(units)
    case .uri: return isURI(units)
    case .date: return isDate(units)
    case .dateTime:
        return !JSDate.parse(text, timeZone: timeZone).isNaN && hasTimeOfDay(units)
    }
}

/// `/^[^\s@]+@[^\s@.]+\.[^\s@]+$/`: one "@", no whitespace, and a dot after a label past the "@".
private func isEmail(_ text: Units) -> Bool {
    guard !text.contains(where: JS.isSpace), let at = text.firstIndex(of: 0x40), at > 0 else { return false }
    let domain = text[(at + 1)...]
    guard !domain.contains(0x40), let dot = domain.firstIndex(of: 0x2E) else { return false }
    return dot > at + 1 && dot < text.count - 1
}

/// `/^[A-Za-z][A-Za-z0-9+.-]*:\S+$/`
private func isURI(_ text: Units) -> Bool {
    guard let first = text.first, JS.isAsciiLetter(first) else { return false }
    var at = 1
    while at < text.count, JS.isAsciiLetter(text[at]) || JS.isDigit(text[at]) || [0x2B, 0x2E, 0x2D].contains(text[at]) {
        at += 1
    }
    guard at < text.count, text[at] == 0x3A else { return false }
    let rest = text[(at + 1)...]
    return !rest.isEmpty && !rest.contains(where: JS.isSpace)
}

/// `/^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$/`
private func isDate(_ text: Units) -> Bool {
    guard text.count == 10, text[4] == 0x2D, text[7] == 0x2D else { return false }
    let digits = [0, 1, 2, 3, 5, 6, 8, 9]
    guard digits.allSatisfy({ JS.isDigit(text[$0]) }) else { return false }
    let month = Int(text[5] - 0x30) * 10 + Int(text[6] - 0x30)
    let day = Int(text[8] - 0x30) * 10 + Int(text[9] - 0x30)
    return (1...12).contains(month) && (1...31).contains(day)
}

/// `/T\d{2}:\d{2}:\d{2}/` anywhere in the text.
private func hasTimeOfDay(_ text: Units) -> Bool {
    guard text.count >= 9 else { return false }
    for start in 0...(text.count - 9) where text[start] == 0x54 {
        let shape: [Bool] = [true, true, false, true, true, false, true, true]
        let matched = shape.enumerated().allSatisfy { offset, digit in
            let unit = text[start + 1 + offset]
            return digit ? JS.isDigit(unit) : unit == 0x3A
        }
        if matched { return true }
    }
    return false
}

private func formatWords(_ format: UiTextFormat) -> String {
    switch format {
    case .email: "an email address"
    case .uri: "a web address or other URI"
    case .date: "a date"
    case .dateTime: "a date and time"
    }
}

/// A field's answer, or why it cannot be sent. An empty optional field sends nothing.
public struct FieldAnswer: Sendable, Hashable {
    public var value: UiFieldValue?
    public var error: String?
}

public func fieldAnswer(_ field: UiField, _ draft: FieldDraft?, timeZone: TimeZone = .current) -> FieldAnswer {
    let name = field.label
    func none(_ error: String? = nil) -> FieldAnswer { FieldAnswer(value: nil, error: error) }
    func some(_ value: UiValue) -> FieldAnswer { FieldAnswer(value: UiFieldValue(id: field.id, value: value), error: nil) }
    switch field.kind {
    case .text(let format, let minLength, let maxLength, _):
        guard case .text(let raw) = draft, !JS.trim(raw.units).isEmpty else {
            return none(field.required ? "\(name) is required." : nil)
        }
        // A datetime-local input has no zone or seconds: it is sent as the moment it names.
        let text = format == .dateTime ? isoMoment(raw, timeZone: timeZone) : raw
        let length = text.unicodeScalars.count
        if let minLength, length < minLength { return none("\(name) needs at least \(minLength) characters.") }
        if let maxLength, length > maxLength { return none("\(name) takes at most \(maxLength) characters.") }
        if let format, !acceptsFormat(format, text, timeZone: timeZone) {
            return none("\(name) must be \(formatWords(format)).")
        }
        return some(.text(text))
    case .number(let integer, let minimum, let maximum, _):
        let raw: Units = if case .text(let text) = draft { JS.trim(text.units) } else { [] }
        if raw.isEmpty { return none(field.required ? "\(name) is required." : nil) }
        let number = JS.toNumber(String(units: raw))
        if !number.isFinite { return none("\(name) must be a number.") }
        if integer && !JS.isInteger(number) { return none("\(name) must be a whole number.") }
        if let minimum, number < minimum { return none("\(name) must be at least \(JS.numberString(minimum)).") }
        if let maximum, number > maximum { return none("\(name) must be at most \(JS.numberString(maximum)).") }
        return some(.number(number))
    case .boolean:
        guard case .flag(let value?) = draft else {
            // A required checkbox left alone is a no; an optional one left alone says nothing.
            return field.required ? some(.boolean(false)) : none()
        }
        return some(.boolean(value))
    case .choice(_, let multiple, let minItems, let maxItems, _):
        let selected: [Int] = if case .choices(let picked) = draft { picked } else { [] }
        if !multiple {
            if selected.isEmpty { return none(field.required ? "Choose one for \(name)." : nil) }
            return some(.choices([selected[0]]))
        }
        if selected.isEmpty && !field.required && minItems == nil { return none() }
        if let minItems, selected.count < minItems { return none("Choose at least \(minItems) for \(name).") }
        if let maxItems, selected.count > maxItems { return none("Choose at most \(maxItems) for \(name).") }
        return some(.choices(selected))
    }
}

/// The submitted form, or each field's problem by id when it cannot be sent yet.
public struct FormAnswer: Sendable, Hashable {
    public var answer: UiAnswer?
    public var errors: OrderedMap<String>
}

public func formAnswer(_ fields: [UiField], _ drafts: OrderedMap<FieldDraft>, timeZone: TimeZone = .current)
    -> FormAnswer
{
    var values: [UiFieldValue] = []
    var errors = OrderedMap<String>()
    for field in fields {
        let answer = fieldAnswer(field, drafts[field.id], timeZone: timeZone)
        if let error = answer.error {
            errors[field.id] = error
        } else if let value = answer.value {
            values.append(value)
        }
    }
    return FormAnswer(
        answer: errors.isEmpty ? .form(accept: true, values: .array(values.map(\.json))) : nil,
        errors: errors)
}

/// A choice field after the person picks `index`: one choice replaces another, many toggle.
public func pickChoice(_ field: UiField, _ draft: FieldDraft?, _ index: Int) -> FieldDraft {
    let selected: [Int] = if case .choices(let picked) = draft { picked } else { [] }
    guard case .choice(_, true, _, _, _) = field.kind else { return .choices([index]) }
    return .choices(selected.contains(index) ? selected.filter { $0 != index } : selected + [index])
}

// MARK: JSON, as the desktop's TypeScript holds these

extension UiValue: Codable {
    public init(from decoder: any Decoder) throws {
        guard let value = UiValue(json: try JSONValue(from: decoder)) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "Not a field value"))
        }
        self = value
    }

    public func encode(to encoder: any Encoder) throws { try json.encode(to: encoder) }
}

extension UiFieldValue: Codable {
    enum CodingKeys: String, CodingKey { case id, value }
}

extension FieldDraft: Codable {
    enum CodingKeys: String, CodingKey { case kind, text, value, selected }

    /// `{ kind: "text", text }`, `{ kind: "flag", value }` or `{ kind: "choices", selected }`.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .kind) {
        case "flag": self = .flag(try container.decodeIfPresent(Bool.self, forKey: .value))
        case "choices": self = .choices(try container.decodeIfPresent([Int].self, forKey: .selected) ?? [])
        default: self = .text(try container.decodeIfPresent(String.self, forKey: .text) ?? "")
        }
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .text(let text):
            try container.encode("text", forKey: .kind)
            try container.encode(text, forKey: .text)
        case .flag(let value):
            try container.encode("flag", forKey: .kind)
            try container.encode(value, forKey: .value)
        case .choices(let selected):
            try container.encode("choices", forKey: .kind)
            try container.encode(selected, forKey: .selected)
        }
    }
}
