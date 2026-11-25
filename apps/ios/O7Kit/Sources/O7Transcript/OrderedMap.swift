/// A map that keeps its keys in the order they were first set, as a JavaScript `Map` does. Setting
/// a key that is already there replaces its value in place. Keys are compared code unit by code
/// unit, as JavaScript compares strings (Swift's `String` treats canonically equivalent text as
/// equal). A value type: copies share storage until one is changed.
public struct OrderedMap<Value> {
    public private(set) var keys: [String] = []
    private var values: [Value] = []
    private var positions: [ExactKey: Int] = [:]

    public init() {}

    public init(_ pairs: some Sequence<(String, Value)>) {
        for (key, value) in pairs { self[key] = value }
    }

    public var count: Int { keys.count }
    public var isEmpty: Bool { keys.isEmpty }

    public subscript(key: String) -> Value? {
        get { positions[ExactKey(key)].map { values[$0] } }
        set {
            let exact = ExactKey(key)
            if let at = positions[exact] {
                if let newValue {
                    values[at] = newValue
                } else {
                    remove(at: at)
                }
            } else if let newValue {
                positions[exact] = keys.count
                keys.append(key)
                values.append(newValue)
            }
        }
    }

    public func contains(_ key: String) -> Bool { positions[ExactKey(key)] != nil }

    /// The values, in key order.
    public var orderedValues: [Value] { values }

    private mutating func remove(at index: Int) {
        positions[ExactKey(keys[index])] = nil
        keys.remove(at: index)
        values.remove(at: index)
        for later in index..<keys.count { positions[ExactKey(keys[later])] = later }
    }
}

extension OrderedMap: Sequence {
    public typealias Element = (key: String, value: Value)

    public struct Iterator: IteratorProtocol {
        let map: OrderedMap
        var index = 0
        public mutating func next() -> Element? {
            guard index < map.keys.count else { return nil }
            defer { index += 1 }
            return (map.keys[index], map.values[index])
        }
    }

    public func makeIterator() -> Iterator { Iterator(map: self) }
}

extension OrderedMap: Sendable where Value: Sendable {}

extension OrderedMap: Equatable where Value: Equatable {
    public static func == (lhs: OrderedMap, rhs: OrderedMap) -> Bool {
        lhs.keys.count == rhs.keys.count
            && zip(lhs.keys, rhs.keys).allSatisfy { $0.same(as: $1) }
            && lhs.values == rhs.values
    }
}

extension OrderedMap: Hashable where Value: Hashable {
    public func hash(into hasher: inout Hasher) {
        for key in keys { hasher.combine(ExactKey(key)) }
        hasher.combine(values)
    }
}

extension OrderedMap: ExpressibleByDictionaryLiteral {
    public init(dictionaryLiteral elements: (String, Value)...) {
        self.init(elements)
    }
}
