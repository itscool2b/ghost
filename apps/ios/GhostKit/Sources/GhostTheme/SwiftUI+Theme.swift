public import SwiftUI

extension Color {
    /// A palette colour, in sRGB.
    public init(_ rgba: RGBA) {
        self.init(.sRGB, red: rgba.r, green: rgba.g, blue: rgba.b, opacity: rgba.a)
    }
}

extension Palette {
    /// The colour scheme this palette is drawn for.
    public var colorScheme: ColorScheme { isDark ? .dark : .light }
}

extension Appearance {
    /// The colour scheme this appearance forces, or nil to follow the system.
    public var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

extension EnvironmentValues {
    /// The palette Ghost's views draw with. `ghostTheme(_:appearance:)` sets it; without it views get
    /// the default theme in dark.
    @Entry public var ghostPalette: Palette = Palettes.palette(.default, dark: true)
}

extension View {
    /// Draws this view in a theme: sets `ghostPalette` for the appearance in effect (the system's
    /// when `appearance` is `.system`, which follows changes), forces the colour scheme for a
    /// light or dark appearance, and tints controls with the theme's accent. The app's root also
    /// sets `preferredColorScheme(appearance.colorScheme)` so its windows and sheets agree.
    public func ghostTheme(_ theme: ThemeID, appearance: Appearance = .system) -> some View {
        modifier(ThemeModifier(theme: theme, appearance: appearance))
    }
}

private struct ThemeModifier: ViewModifier {
    let theme: ThemeID
    let appearance: Appearance
    @Environment(\.colorScheme) private var colorScheme

    func body(content: Content) -> some View {
        let palette = Palettes.palette(theme, appearance: appearance, systemIsDark: colorScheme == .dark)
        content
            .environment(\.ghostPalette, palette)
            .environment(\.colorScheme, palette.colorScheme)
            .tint(Color(palette.accent))
    }
}
