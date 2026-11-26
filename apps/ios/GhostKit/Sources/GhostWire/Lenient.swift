/// A string enum that reads any value: one this copy does not know becomes its fallback, as the
/// desktop's `lenient_enum!` does. The app can be older or newer than the computer it talks to.
public protocol LenientEnum: RawRepresentable, Codable, Hashable, Sendable, CaseIterable
where RawValue == String {
    static var fallback: Self { get }
}

extension LenientEnum {
    public init(from decoder: any Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: raw) ?? Self.fallback
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

/// Coding keys made from any string, for tagged unions and flattened objects.
public struct AnyKey: CodingKey, Hashable, Sendable {
    public let stringValue: String
    public var intValue: Int? { nil }
    public init(_ string: String) { stringValue = string }
    public init?(stringValue: String) { self.stringValue = stringValue }
    public init?(intValue: Int) { nil }
}

extension KeyedDecodingContainer where Key == AnyKey {
    /// A field that may be missing or null.
    func maybe<T: Decodable>(_ key: String, _ type: T.Type = T.self) throws -> T? {
        try decodeIfPresent(T.self, forKey: AnyKey(key))
    }

    func need<T: Decodable>(_ key: String, _ type: T.Type = T.self) throws -> T {
        try decode(T.self, forKey: AnyKey(key))
    }
}

extension KeyedEncodingContainer where Key == AnyKey {
    /// Writes `null` for nil, as serde and the TypeScript side do.
    mutating func put<T: Encodable>(_ key: String, _ value: T?) throws {
        if let value { try encode(value, forKey: AnyKey(key)) } else { try encodeNil(forKey: AnyKey(key)) }
    }

    mutating func put<T: Encodable>(_ key: String, _ value: T) throws {
        try encode(value, forKey: AnyKey(key))
    }
}
