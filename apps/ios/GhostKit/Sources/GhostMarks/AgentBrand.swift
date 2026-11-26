import Foundation

/// An agent's brand as the desktop's brand.ts gives it: its id, its display name and the
/// two-letter code its mark shows without a logo. Data in AgentBrands.generated.swift.
public struct AgentBrand: Sendable, Hashable {
    /// The brand id: `claude-code`, `codex`, `pi` and so on, or `generic`.
    public let id: String
    /// The name labels use: "Claude Code", or "Agent" for an agent without a brand.
    public let name: String
    /// The designed two-letter code ("CC", "CX"); nil for the generic brand, whose code comes
    /// from the agent's own id.
    public let code: String?
    /// Whether the mark's white artwork sits on the brand plate when idle or a ghost.
    let plate: Bool

    /// True for an agent without a designed brand.
    public var isGeneric: Bool { id == "generic" }

    /// The generic brand: an agent without a designed one.
    public static let generic: AgentBrand =
        all.first { $0.id == "generic" } ?? AgentBrand(id: "generic", name: "Agent", code: nil, plate: false)

    private static let byID: [String: AgentBrand] = Dictionary(uniqueKeysWithValues: all.map { ($0.id, $0) })

    /// The brand for an agent id, tolerating case, separators and the known aliases ("Claude
    /// Code", "claude_code", "agy"), as `getBrand` does. Any other id is the generic brand.
    public static func lookup(_ agent: String) -> AgentBrand {
        let key = normalizedID(agent)
        if let brand = byID[key] { return brand }
        if let alias = aliases[key], let brand = byID[alias] { return brand }
        return generic
    }

    /// `id.trim().toLowerCase().replace(/[\s_.]+/g, "-")`, with JavaScript's whitespace.
    static func normalizedID(_ id: String) -> String {
        let trimmed = id.unicodeScalars.drop(while: isJSWhitespace).reversed().drop(while: isJSWhitespace).reversed()
        let lowered = String(String.UnicodeScalarView(trimmed)).lowercased()
        var out = String.UnicodeScalarView()
        var inRun = false
        for scalar in lowered.unicodeScalars {
            if isJSWhitespace(scalar) || scalar == "_" || scalar == "." {
                if !inRun { out.append("-") }
                inRun = true
            } else {
                out.append(scalar)
                inRun = false
            }
        }
        return String(out)
    }

    /// ECMAScript's WhiteSpace and LineTerminator, which `trim()` and `\s` match.
    static func isJSWhitespace(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x09...0x0D, 0x20, 0xA0, 0x1680, 0x2000...0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF:
            true
        default:
            false
        }
    }
}

/// An agent's two-letter code, as AgentMark.tsx's `agentCode`: the designed code for a branded
/// agent (CC Claude Code, CX Codex, PI Pi, MP oh-my-pi, AG Antigravity, HM Hermes, CU Cursor,
/// GB Grok Build, OC OpenCode); for any other agent the first two letters or digits of its id,
/// uppercased; "?" for an empty or "generic" id.
public func agentCode(_ agent: String) -> String {
    let brand = AgentBrand.lookup(agent)
    if let code = brand.code { return code }
    let kept = agent.precomposedStringWithCanonicalMapping.unicodeScalars.filter(isLetterOrNumber)
    let letters = String(String.UnicodeScalarView(kept)).uppercased()
    if letters.isEmpty || letters == "GENERIC" { return "?" }
    return String(String.UnicodeScalarView(letters.unicodeScalars.prefix(2)))
}

/// The name labels use for an agent: its brand's ("Claude Code"), or "Agent".
public func brandName(_ agent: String) -> String {
    AgentBrand.lookup(agent).name
}

/// `\p{L}` or `\p{N}`.
private func isLetterOrNumber(_ scalar: Unicode.Scalar) -> Bool {
    switch scalar.properties.generalCategory {
    case .uppercaseLetter, .lowercaseLetter, .titlecaseLetter, .modifierLetter, .otherLetter,
        .decimalNumber, .letterNumber, .otherNumber:
        true
    default:
        false
    }
}

/// What an agent is doing, as its mark shows it: the desktop's orb states.
public enum AgentState: String, CaseIterable, Sendable {
    case idle
    case thinking
    case streaming
    case tool
    case waiting
    case done
    case error

    /// The state for any string; anything unknown is idle, as on the desktop.
    public init(lenient value: String) {
        self = AgentState(rawValue: value) ?? .idle
    }

    /// The loader's pace while working; nil when the agent is not working.
    public var tempo: LoaderTempo? {
        switch self {
        case .thinking: .thinking
        case .streaming: .streaming
        case .tool: .tool
        case .idle, .waiting, .done, .error: nil
        }
    }
}

extension AgentState: Codable {
    /// Decodes leniently: an unknown state is idle.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        self.init(lenient: (try? container.decode(String.self)) ?? "")
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}
