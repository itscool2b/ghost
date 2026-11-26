public import GhostWire

// The working line's words (`working.ts`, docs/design/visual-language.md): while an agent works
// and no text streams, the tail of its turn says so with one playful verb at a time. Every choice
// is seeded by the turn, so a turn always shows the same sequence, and two turns read differently.

/// What the agent's last call was doing, so the words can lean towards it. `nil` is thinking.
public enum WorkingContext: String, Sendable, Hashable, CaseIterable, Codable {
    case look, make, run
}

/// Thinking and planning: the default, and the half of every pick that does not lean.
public let thinkingVerbs = [
    "Pondering", "Noodling", "Mulling", "Musing", "Cogitating", "Percolating", "Brewing",
    "Simmering", "Marinating", "Scheming", "Plotting", "Conjuring", "Concocting", "Hatching",
    "Puzzling", "Ruminating", "Untangling", "Finagling",
]

/// After a read, a search or a fetch.
public let lookingVerbs = [
    "Sleuthing", "Rummaging", "Spelunking", "Scouring", "Combing", "Foraging", "Perusing", "Digging",
]

/// After an edit, a move or a deletion.
public let makingVerbs = [
    "Whittling", "Crafting", "Tinkering", "Polishing", "Sculpting", "Stitching", "Assembling",
    "Weaving",
]

/// After a command.
public let runningVerbs = [
    "Wrangling", "Herding bits", "Juggling", "Orchestrating", "Churning", "Cranking",
]

/// How often a pick leans towards the context's own words.
private let lean = 0.6

/// The rotation's pace: each verb stays between these, seeded per turn and step.
public let rotationMinMs = 2500
public let rotationMaxMs = 3500

/// FNV-1a over the UTF-16 code units: a turn's key as a 32-bit seed.
public func seedOf(_ text: String) -> UInt32 {
    var hash: UInt32 = 0x811c9dc5
    for unit in text.utf16 {
        hash ^= UInt32(unit)
        hash = hash &* 0x01000193
    }
    return hash
}

/// A number in [0, 1) fixed by `seed` and `step` (one round of mulberry32).
private func unit(_ seed: UInt32, _ step: Int) -> Double {
    var t = seed &+ UInt32(truncatingIfNeeded: step &+ 1) &* 0x6d2b79f5
    t = (t ^ (t >> 15)) &* (t | 1)
    t ^= t &+ (t ^ (t >> 7)) &* (t | 61)
    return Double(t ^ (t >> 14)) / 4_294_967_296
}

/// `pool` in an order fixed by `seed` (Fisher-Yates on the seeded numbers).
private func shuffled(_ pool: [String], _ seed: UInt32) -> [String] {
    var order = pool
    var index = order.count - 1
    while index > 0 {
        let other = Int((unit(seed, 1000 + index) * Double(index + 1)).rounded(.down))
        order.swapAt(index, other)
        index -= 1
    }
    return order
}

private func pool(_ context: WorkingContext) -> [String] {
    switch context {
    case .look: lookingVerbs
    case .make: makingVerbs
    case .run: runningVerbs
    }
}

/// Each pool takes its own order from the turn's seed.
private func salt(_ context: WorkingContext) -> UInt32 {
    switch context {
    case .look: 0x9e3779b9
    case .make: 0x85ebca6b
    case .run: 0xc2b2ae35
    }
}

/// The verb a turn shows at step `tick` (from 0). Each pool is walked in its own seeded order, one
/// step per tick, so a pool never names the same verb twice in a row; with a context, about three
/// picks in five come from its pool, the rest from the thinking words.
public func workingVerb(seed: UInt32, tick: Int, context: WorkingContext?) -> String {
    var words = thinkingVerbs
    var mixed = seed
    if let context, unit(seed, tick * 2 + 1) < lean {
        words = pool(context)
        mixed = seed ^ salt(context)
    }
    let order = shuffled(words, mixed)
    let offset = Int((unit(mixed, 7) * Double(order.count)).rounded(.down))
    return order[(offset + tick) % order.count]
}

/// How long the verb at step `tick` stays, between 2.5 and 3.5 seconds.
public func rotationDelay(seed: UInt32, tick: Int) -> Int {
    rotationMinMs
        + Int((unit(seed, tick * 2) * Double(rotationMaxMs - rotationMinMs)).rounded(.down))
}

/// The context a call of this kind sets. Thinking, mode changes and other tools set none.
public func workingContext(_ kind: ToolKind?) -> WorkingContext? {
    switch kind {
    case .read, .search, .fetch: .look
    case .edit, .delete, .move: .make
    case .execute: .run
    default: nil
    }
}

/// 0 s, 12 s, 1 min 5 s, 1 h 2 min: whole steps, so the count ticks once a second.
public func formatElapsed(_ ms: Double) -> String {
    let seconds = ms.isFinite && ms > 0 ? (ms / 1000).rounded(.down) : 0
    if seconds < 60 { return "\(JS.numberString(seconds)) s" }
    let minutes = (seconds / 60).rounded(.down)
    if minutes < 60 { return "\(JS.numberString(minutes)) min \(JS.numberString(JS.remainder(seconds, 60))) s" }
    return "\(JS.numberString((minutes / 60).rounded(.down))) h \(JS.numberString(JS.remainder(minutes, 60))) min"
}

/// When the UI first saw each turn working, by turn key, so the elapsed time survives a row leaving
/// and re-entering view and the chat being reopened. Holds the newest 64 turns.
public struct WorkingSince: Sendable {
    private var firstSeen = OrderedMap<Double>()
    private static let rememberedTurns = 64

    public init() {}

    public mutating func since(_ key: String, now: Double) -> Double {
        if let known = firstSeen[key] { return known }
        firstSeen[key] = now
        if firstSeen.count > Self.rememberedTurns, let oldest = firstSeen.keys.first {
            firstSeen[oldest] = nil
        }
        return now
    }
}
