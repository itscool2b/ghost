import SwiftUI
import O7Theme
import O7Wire

/// The colours a screen draws with, for the look in force: one of the desktop's ten themes, light
/// or dark. Every view reads them from the environment (`@Environment(\.style)`), so a computer's
/// theme change repaints the whole app at once.
struct O7Style: Equatable {
    var theme: ThemeID
    var dark: Bool
    // Layers, back to front.
    var window: Color
    var pane: Color
    var raised: Color
    var surface: Color
    // Lines.
    var line: Color
    var lineStrong: Color
    // Ink, strongest first.
    var fg1: Color
    var fg2: Color
    var fg3: Color
    var fg4: Color
    // Actions.
    var primary: Color
    var onPrimary: Color
    /// "Volt": what needs the person.
    var accent: Color
    var accentTint: Color
    // Status, the same in every theme.
    var green: Color
    var red: Color
    var redTint: Color
    var amber: Color
    var amberTint: Color
    // The sent message.
    var paper: Color
    var onPaper: Color
    // Code.
    var codeBackground: Color
    var codeText: Color
    // Ink at fixed strengths over any layer.
    var card: Color
    var control: Color
    var selected: Color

    var scheme: ColorScheme { dark ? .dark : .light }

    /// The generated palette behind these colours, for o7's marks and code colours.
    var palette: Palette { Palettes.palette(theme, dark: dark) }
}

extension View {
    /// Puts a look on a view tree: the app's colours and the palette o7's marks read.
    func o7Look(_ style: O7Style) -> some View {
        environment(\.style, style).environment(\.o7Palette, style.palette)
    }
}

extension EnvironmentValues {
    @Entry var style: O7Style = .fallback
}

extension O7Style {
    /// Main, dark: o7's default until a computer says otherwise.
    static let fallback = O7Style.make(theme: .default, dark: true)

    /// The style of a computer's look, resolved against the phone's own appearance.
    static func of(_ look: Look, systemDark: Bool) -> O7Style {
        let dark =
            switch look.appearance {
            case "light": false
            case "dark": true
            default: systemDark
            }
        return make(theme: ThemeID(lenient: look.theme), dark: dark)
    }

    static func make(theme: ThemeID, dark: Bool) -> O7Style {
        let p = Palettes.palette(theme, dark: dark)
        return O7Style(
            theme: theme, dark: dark,
            window: Color(p.window), pane: Color(p.pane), raised: Color(p.raised),
            surface: Color(p.surface2),
            line: Color(p.line), lineStrong: Color(p.lineStrong),
            fg1: Color(p.fg1), fg2: Color(p.fg2), fg3: Color(p.fg3), fg4: Color(p.fg4),
            primary: Color(p.primary), onPrimary: Color(p.onPrimary),
            accent: Color(p.accent), accentTint: Color(p.accentTint),
            green: Color(p.green), red: Color(p.red), redTint: Color(p.redTint),
            amber: Color(p.amber), amberTint: Color(p.amberTint),
            paper: Color(p.paper), onPaper: Color(p.onPaper),
            codeBackground: Color(p.codeBackground), codeText: Color(p.codeText),
            card: Color(p.inkCard), control: Color(p.inkControl), selected: Color(p.inkSelected))
    }
}
