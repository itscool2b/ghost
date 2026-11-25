/// The generated palettes: every theme in both appearances (Palettes.generated.swift).
public enum Palettes {
    /// A theme's palette in one appearance.
    public static func palette(_ id: ThemeID, dark: Bool) -> Palette {
        generated(id, dark: dark)
    }

    /// A theme's palette for an appearance, given whether the system is dark.
    public static func palette(_ id: ThemeID, appearance: Appearance, systemIsDark: Bool) -> Palette {
        generated(id, dark: appearance.isDark(systemIsDark: systemIsDark))
    }

    /// Every palette: each theme in order, dark then light.
    public static let all: [Palette] = ThemeID.allCases.flatMap { id in
        [generated(id, dark: true), generated(id, dark: false)]
    }
}

extension Palette {
    /// highlight.js type: the keyword colour, as on the desktop.
    public var syntaxType: RGBA { syntaxKeyword }
    /// highlight.js built_in: the keyword colour, as on the desktop.
    public var syntaxBuiltIn: RGBA { syntaxKeyword }
    /// highlight.js meta: the comment colour, as on the desktop.
    public var syntaxMeta: RGBA { syntaxComment }
}
