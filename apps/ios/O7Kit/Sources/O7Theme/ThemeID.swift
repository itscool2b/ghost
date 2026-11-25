import Foundation

/// How the app looks: one of the computer's themes in an appearance. The default look is Claude
/// Code following the system.
extension ThemeID {
    /// The theme the app takes when it knows nothing better.
    public static let `default`: ThemeID = .claude

    /// The theme for any string: an id as the computer sends it (`oh_my_pi`, also `oh-my-pi` or
    /// `OH_MY_PI`) or a theme's name (`Claude Code`). Anything else is Claude Code.
    public init(lenient value: String) {
        let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
        let key = String(text.lowercased().map { $0 == "-" || $0 == " " ? "_" : $0 })
        if let match = ThemeID(rawValue: key) {
            self = match
        } else if let named = ThemeID.allCases.first(where: { $0.name.lowercased() == text.lowercased() }) {
            self = named
        } else {
            self = .default
        }
    }
}

extension ThemeID: Codable {
    /// Decodes leniently: an unknown or missing theme is the default, never an error.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        self.init(lenient: (try? container.decode(String.self)) ?? "")
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

/// The appearance a look asks for: follow the system, or always light or dark.
public enum Appearance: String, CaseIterable, Sendable {
    case system
    case light
    case dark

    /// The appearance for any string; anything unknown follows the system.
    public init(lenient value: String) {
        self = Appearance(rawValue: value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()) ?? .system
    }

    /// Whether this appearance draws dark when the system's own is `systemIsDark`.
    public func isDark(systemIsDark: Bool) -> Bool {
        switch self {
        case .system: systemIsDark
        case .light: false
        case .dark: true
        }
    }
}

extension Appearance: Codable {
    /// Decodes leniently: an unknown or missing appearance follows the system.
    public init(from decoder: any Decoder) throws {
        let container = try decoder.singleValueContainer()
        self.init(lenient: (try? container.decode(String.self)) ?? "")
    }

    public func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}
