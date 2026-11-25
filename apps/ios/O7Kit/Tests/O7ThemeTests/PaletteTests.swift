import Foundation
import O7Theme
import SwiftUI
import Testing

@Suite("RGBA")
struct RGBATests {
    @Test("parses the forms the tokens use")
    func parses() {
        #expect(RGBA(css: "#191817") == RGBA(hex: 0x191817))
        #expect(RGBA(css: "#fff") == RGBA(hex: 0xffffff))
        #expect(RGBA(css: "rgb(245 245 245 / 0.04)") == RGBA(hex: 0xf5f5f5, alpha: 0.04))
        #expect(RGBA(css: "rgb(0 0 0)") == RGBA(hex: 0))
        for bad in ["", "#12345", "#ggg", "rgb(1 2)", "rgb(256 0 0 / 1)", "rgb(1 2 3 / 2)", "red"] {
            #expect(RGBA(css: bad) == nil, "\(bad)")
        }
    }

    @Test("writes hex and keeps 8-bit values exact")
    func hex() {
        #expect(RGBA(hex: 0x0a0b0c).hex == "#0a0b0c")
        #expect(RGBA(hex: 0xf5f1e8, alpha: 0.5).description == "#f5f1e8 / 0.5")
        #expect(RGBA(r: 0.5, g: 0.5, b: 0.5).quantized == RGBA(hex: 0x808080))
    }

    @Test("mixes as CSS color-mix in sRGB and composites source over")
    func mixing() {
        // The desktop's paper for Claude Code dark: fg-1 16% into the pane.
        let paper = RGBA(hex: 0xf5f1e8).mixed(0.16, into: RGBA(hex: 0x21201d)).quantized
        #expect(paper == RGBA(hex: 0x43413d))
        let half = RGBA(hex: 0xffffff, alpha: 0.5).over(RGBA(hex: 0x000000)).quantized
        #expect(half == RGBA(hex: 0x808080))
        #expect(RGBA(hex: 0x123456).over(RGBA(hex: 0xffffff)) == RGBA(hex: 0x123456))
    }

    @Test("measures WCAG contrast")
    func contrast() {
        #expect(abs(RGBA(hex: 0).contrast(with: RGBA(hex: 0xffffff)) - 21) < 1e-9)
        #expect(RGBA(hex: 0x777777).contrast(with: RGBA(hex: 0x777777)) == 1)
        #expect(abs(RGBA(hex: 0xffffff).luminance - 1) < 1e-12)
    }

    @Test("bridges to SwiftUI")
    func color() {
        let resolved = Color(RGBA(hex: 0xff8000, alpha: 0.25)).resolve(in: EnvironmentValues())
        #expect(abs(Double(resolved.opacity) - 0.25) < 1e-6)
    }
}

@Suite("Themes")
struct ThemeTests {
    @Test("lists the desktop's themes, Aura first")
    func order() {
        #expect(ThemeID.allCases.map(\.rawValue) == [
            "aura", "claude", "codex", "grok", "pi", "antigravity", "cursor", "opencode", "oh_my_pi", "hermes",
        ])
        #expect(ThemeID.aura.name == "Aura")
        #expect(ThemeID.aura.agent == nil)
        #expect(ThemeID.ohMyPi.name == "oh-my-pi")
        #expect(ThemeID.ohMyPi.agent == "oh-my-pi")
        #expect(ThemeID.default == .claude)
    }

    @Test("reads any string, falling back to Claude Code")
    func lenient() {
        #expect(ThemeID(lenient: "codex") == .codex)
        #expect(ThemeID(lenient: " Hermes ") == .hermes)
        #expect(ThemeID(lenient: "oh-my-pi") == .ohMyPi)
        #expect(ThemeID(lenient: "OH_MY_PI") == .ohMyPi)
        #expect(ThemeID(lenient: "Grok Build") == .grok)
        #expect(ThemeID(lenient: "solarized") == .claude)
        #expect(ThemeID(lenient: "") == .claude)
        #expect(Appearance(lenient: "Dark") == .dark)
        #expect(Appearance(lenient: "dim") == .system)
    }

    @Test("decodes a look leniently and encodes the desktop's ids")
    func codable() throws {
        struct Look: Codable, Equatable {
            let theme: ThemeID
            let appearance: Appearance
        }
        let decoded = try JSONDecoder().decode(
            [Look].self, from: Data(#"[{"theme":"oh_my_pi","appearance":"light"},{"theme":7,"appearance":"x"}]"#.utf8)
        )
        #expect(decoded == [Look(theme: .ohMyPi, appearance: .light), Look(theme: .claude, appearance: .system)])
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        let encoded = try encoder.encode(Look(theme: .ohMyPi, appearance: .dark))
        #expect(String(decoding: encoded, as: UTF8.self) == #"{"appearance":"dark","theme":"oh_my_pi"}"#)
    }

    @Test("holds every theme in both appearances")
    func lookup() {
        #expect(Palettes.all.count == 20)
        for id in ThemeID.allCases {
            for dark in [true, false] {
                let palette = Palettes.palette(id, dark: dark)
                #expect(palette.theme == id)
                #expect(palette.isDark == dark)
                #expect(palette.ansi.count == 16)
            }
        }
        #expect(Palettes.palette(.codex, appearance: .system, systemIsDark: false).isDark == false)
        #expect(Palettes.palette(.codex, appearance: .dark, systemIsDark: false).isDark)
    }

    @Test("keeps Graphite's own values as Aura and lays each theme over it")
    func values() {
        let auraDark = Palettes.palette(.aura, dark: true)
        #expect(auraDark.page == RGBA(hex: 0x161616))
        #expect(auraDark.pane == RGBA(hex: 0x161616))
        #expect(auraDark.raised == RGBA(hex: 0x1f1f1f))
        #expect(auraDark.window == RGBA(hex: 0x0e0e0e))
        let claudeDark = Palettes.palette(.claude, dark: true)
        #expect(claudeDark.page == RGBA(hex: 0x191817))
        #expect(claudeDark.pane == RGBA(hex: 0x21201d))
        #expect(claudeDark.raised == RGBA(hex: 0x2a2825))
        #expect(claudeDark.primary == RGBA(hex: 0xd97757))
        #expect(claudeDark.accentTint == RGBA(hex: 0xe3896b, alpha: 0.12))
        #expect(claudeDark.codeText == RGBA(hex: 0xdfdacf))
        // Status colours are Graphite's in every theme.
        for palette in Palettes.all {
            let base = Palettes.palette(.aura, dark: palette.isDark)
            #expect(palette.green == base.green)
            #expect(palette.red == base.red)
            #expect(palette.amber == base.amber)
            #expect(palette.redTint == base.redTint)
        }
        // The brand plate is set once, in dark; light keeps Graphite's.
        #expect(Palettes.palette(.codex, dark: true).brandPlate == RGBA(hex: 0x20252c))
        #expect(Palettes.palette(.codex, dark: false).brandPlate == RGBA(hex: 0x252525))
    }

    @Test("sets the paper as the desktop's sent message")
    func paper() {
        for palette in Palettes.all {
            if palette.isDark {
                #expect(palette.paper == palette.fg1.mixed(0.16, into: palette.pane).quantized)
                #expect(palette.onPaper == palette.fg1)
            } else {
                #expect(palette.paper == palette.fg1)
                #expect(palette.onPaper == palette.page)
            }
        }
    }

    @Test("keeps the ink ladder's strengths from tokens.css")
    func ink() {
        let dark = Palettes.palette(.claude, dark: true)
        #expect([dark.inkCard, dark.inkControl, dark.inkSelected, dark.inkHover, dark.inkActive, dark.rim, dark.rimStrong]
            .map(\.a) == [0.04, 0.06, 0.08, 0.1, 0.16, 0.08, 0.14])
        #expect(dark.inkCard.opacity(1) == RGBA(hex: 0xf5f5f5))
        let light = Palettes.palette(.claude, dark: false)
        #expect([light.inkCard, light.inkControl, light.inkSelected, light.inkHover, light.inkActive, light.rim, light.rimStrong]
            .map(\.a) == [0.035, 0.05, 0.06, 0.08, 0.12, 0.08, 0.14])
        #expect(light.inkCard.opacity(1) == RGBA(hex: 0x1c1c1c))
    }
}
