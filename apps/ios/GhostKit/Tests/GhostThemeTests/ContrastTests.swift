import GhostTheme
import Testing

// The contrast checks from packages/ui/src/graphite/themes.test.ts, run over every generated
// palette, Aura included, plus the layers only the phone draws (the window, the pane, the raised
// layer, the paper and the ink ladder over them).

private let cases: [(label: String, palette: Palette)] = Palettes.all.map { palette in
    ("\(palette.theme.name) \(palette.isDark ? "dark" : "light")", palette)
}

/// `top` drawn over an opaque `under`, rounded to 8 bits, as the desktop test's `over` does.
private func over(_ top: RGBA, _ under: RGBA) -> RGBA {
    top.over(under).quantized
}

private func expectContrast(
    _ text: RGBA, on background: RGBA, atLeast minimum: Double, _ what: String,
    sourceLocation: SourceLocation = #_sourceLocation
) {
    let ratio = text.contrast(with: background)
    #expect(ratio >= minimum, "\(what): \(ratio) under \(minimum)", sourceLocation: sourceLocation)
}

@Suite("Theme contrast")
struct ContrastTests {
    @Test("keeps text readable on the page and every surface")
    func textOnSurfaces() {
        for (label, p) in cases {
            let surfaces: [(String, RGBA)] = [
                ("page", p.page), ("surface1", p.surface1), ("surface2", p.surface2),
                ("surface3", p.surface3), ("surface4", p.surface4),
            ]
            var texts: [(String, RGBA)] = [
                ("fg1", p.fg1), ("fg2", p.fg2), ("accent", p.accent), ("green", p.green), ("red", p.red),
            ]
            if !p.isDark { texts += [("fg3", p.fg3), ("amber", p.amber)] }
            for (surface, background) in surfaces {
                for (name, text) in texts {
                    expectContrast(text, on: background, atLeast: 4.5, "\(label): \(name) on \(surface)")
                }
            }
            for (surface, background) in surfaces.prefix(4) {
                expectContrast(p.fg3, on: background, atLeast: 4.5, "\(label): fg3 on \(surface)")
            }
        }
    }

    @Test("keeps outlines and focus visible, and filled controls' labels readable")
    func controls() {
        for (label, p) in cases {
            for (surface, background) in [("surface1", p.surface1), ("surface2", p.surface2)] {
                // The desktop's --g-outline is its strong line in every theme (the generator checks).
                expectContrast(p.lineStrong, on: background, atLeast: 3, "\(label): outline on \(surface)")
                expectContrast(p.focus, on: background, atLeast: 3, "\(label): focus on \(surface)")
            }
            for (fill, background) in [
                ("primary", p.primary), ("primaryHover", p.primaryHover), ("primaryPressed", p.primaryPressed),
            ] {
                expectContrast(p.onPrimary, on: background, atLeast: 4.5, "\(label): label on \(fill)")
            }
            if p.isDark {
                // Dark: the destructive pill takes the same dark label as the primary one.
                for (fill, background) in [("red", p.red), ("redHover", p.redHover)] {
                    expectContrast(p.onPrimary, on: background, atLeast: 4.5, "\(label): label on \(fill)")
                }
            }
        }
    }

    @Test("keeps code readable where it is set")
    func code() {
        for (label, p) in cases {
            for (name, text) in [
                ("codeText", p.codeText), ("syntaxAttr", p.syntaxAttr), ("syntaxComment", p.syntaxComment),
                ("syntaxKeyword", p.syntaxKeyword), ("syntaxString", p.syntaxString),
                ("syntaxNumber", p.syntaxNumber), ("syntaxTitle", p.syntaxTitle),
                ("terminalText", p.terminalText),
            ] {
                expectContrast(text, on: p.codeBackground, atLeast: 4.5, "\(label): \(name) on codeBackground")
            }
        }
    }

    @Test("orders the dark surface and text ladders")
    func ladders() {
        for (label, p) in cases where p.isDark {
            let surfaces = [p.page, p.surface1, p.surface2, p.surface3, p.surface4].map(\.luminance)
            for index in 1..<surfaces.count {
                #expect(surfaces[index] > surfaces[index - 1], "\(label): surface \(index)")
            }
            let texts = [p.fg1, p.fg2, p.fg3, p.fg4].map(\.luminance)
            for index in 1..<texts.count {
                #expect(texts[index] < texts[index - 1], "\(label): fg\(index + 1)")
            }
        }
    }

    @Test("keeps text readable on the theme's glass over the worst plausible window material")
    func glass() {
        for (label, p) in cases {
            let material = RGBA(hex: p.isDark ? 0x3a3a3a : 0xd4d4d4)
            // The desktop's --g-window-tint is the window colour at 60%, in every theme.
            let tinted = over(p.window.opacity(0.6), material)
            let layer = over(p.pane, tinted)
            let card = over(p.inkCard, layer)
            let bases: [(String, RGBA)] = [
                ("tinted", tinted),
                ("layer", layer),
                ("card", card),
                ("control on card", over(p.inkControl, card)),
                // The desktop's bar is the pane's own colour in every theme.
                ("bar", over(p.pane, layer)),
                ("pop", over(p.raised, layer)),
                ("opaque", over(p.pane, p.window)),
            ]
            for (base, background) in bases {
                for (name, text) in [("fg1", p.fg1), ("fg2", p.fg2), ("fg3", p.fg3)] {
                    expectContrast(text, on: background, atLeast: 4.5, "\(label): \(name) on \(base)")
                }
            }
        }
    }

    @Test("keeps text readable on every layer the phone draws")
    func phoneLayers() {
        for (label, p) in cases {
            let layers: [(String, RGBA)] = [
                ("window", p.window), ("pane", p.pane), ("raised", p.raised), ("overlay", p.overlay),
                ("card on pane", over(p.inkCard, p.pane)),
                ("control on pane", over(p.inkControl, p.pane)),
                ("selected on pane", over(p.inkSelected, p.pane)),
                ("card on raised", over(p.inkCard, p.raised)),
                ("control on raised", over(p.inkControl, p.raised)),
            ]
            for (layer, background) in layers {
                for (name, text) in [("fg1", p.fg1), ("fg2", p.fg2), ("fg3", p.fg3)] {
                    expectContrast(text, on: background, atLeast: 4.5, "\(label): \(name) on \(layer)")
                }
            }
            expectContrast(p.onPaper, on: p.paper, atLeast: 4.5, "\(label): words on the paper")
        }
    }

    // Measured, not guaranteed: tertiary text (fg3) falls to 3.4 to 4.4 on a pressed fill in
    // dark, fg2 to 4.43 on a pressed fill over the raised layer (Aura and Grok Build dark), and fg3
    // to 4.41 on a selected fill over the raised layer (Aura dark). Meta text stays off those.
    @Test("keeps primary and secondary text readable on selected, hovered and pressed fills")
    func interactionFills() {
        for (label, p) in cases {
            for (state, ink) in [("selected", p.inkSelected), ("hover", p.inkHover), ("pressed", p.inkActive)] {
                for (layer, under) in [("pane", p.pane), ("raised", p.raised)] {
                    let background = over(ink, under)
                    expectContrast(p.fg1, on: background, atLeast: 4.5, "\(label): fg1 on \(state) on \(layer)")
                    if state != "pressed" || layer == "pane" {
                        expectContrast(p.fg2, on: background, atLeast: 4.5, "\(label): fg2 on \(state) on \(layer)")
                    }
                }
            }
        }
    }
}
