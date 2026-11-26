public import GhostTheme

/// A stretch of code and the highlight.js scopes around it. A highlighted block's runs follow one
/// another and cover the whole code.
public struct HighlightRun: Sendable, Hashable {
    /// Where the run is in the code, in UTF-16 code units, as JavaScript counts.
    public let range: Range<Int>
    /// The highlight.js scopes around the run, outermost first, as highlight.js names them
    /// (`["meta", "keyword"]`, `title.class`, `language:xml`). Empty for plain text.
    public let scopes: [String]

    public init(range: Range<Int>, scopes: [String]) {
        self.range = range
        self.scopes = scopes
    }

    /// The innermost scope, or nil for plain text.
    public var scope: String? { scopes.last }

    /// The colour the desktop's stylesheet gives this run.
    public var role: HighlightRole { HighlightRole(scopes: scopes) }

    /// Whether the desktop sets this run in italic: it sits in an `emphasis` scope.
    public var isEmphasized: Bool { scopes.contains { HighlightRole.base(of: $0) == "emphasis" } }

    /// Whether the desktop sets this run in semibold: it sits in a `strong` scope.
    public var isStrong: Bool { scopes.contains { HighlightRole.base(of: $0) == "strong" } }
}

/// The colours apps/desktop/src/components/rich-blocks.css gives highlight.js's scopes. The
/// innermost scope with a rule decides; a scope without one (`subst`, `operator`, a sub-language)
/// takes its parent's colour, and text in no rule's scope is plain code.
public enum HighlightRole: Sendable, Hashable, CaseIterable {
    case plain
    case keyword
    case string
    case number
    case title
    case attr
    case comment
    case addition
    case deletion

    /// The role for a run's scopes, outermost first.
    public init(scopes: [String]) {
        self = scopes.reversed().lazy.compactMap(HighlightRole.init(scope:)).first ?? .plain
    }

    /// The role one scope's rule sets, or nil when the stylesheet has no rule for it. Only the
    /// scope's first part counts: `title.class` is drawn as `title`.
    public init?(scope: String) {
        guard let role = Self.rules[Self.base(of: scope)] else { return nil }
        self = role
    }

    /// The palette colour for this role.
    public func color(in palette: Palette) -> RGBA {
        switch self {
        case .plain: palette.codeText
        case .keyword: palette.syntaxKeyword
        case .string: palette.syntaxString
        case .number: palette.syntaxNumber
        case .title: palette.syntaxTitle
        case .attr: palette.syntaxAttr
        case .comment: palette.syntaxComment
        case .addition: palette.syntaxAddition
        case .deletion: palette.syntaxDeletion
        }
    }

    /// A scope's first part, the one highlight.js writes as its `hljs-` class.
    static func base(of scope: String) -> Substring {
        scope.prefix { $0 != "." }
    }

    /// Each rule of rich-blocks.css, by the scope it names.
    private static let rules: [Substring: HighlightRole] = {
        let table: [(HighlightRole, [Substring])] = [
            (.keyword, ["keyword", "selector-tag", "built_in", "doctag", "template-tag", "type"]),
            (.string, ["string", "regexp", "template-variable", "symbol", "link", "selector-attr", "selector-pseudo"]),
            (.number, ["number", "literal", "bullet"]),
            (.title, ["title", "section", "selector-id", "selector-class", "name"]),
            (.attr, ["attr", "attribute", "property", "params", "variable"]),
            (.comment, ["comment", "quote", "meta"]),
            (.addition, ["addition"]),
            (.deletion, ["deletion"]),
        ]
        var rules: [Substring: HighlightRole] = [:]
        for (role, scopes) in table {
            for scope in scopes { rules[scope] = role }
        }
        return rules
    }()
}
