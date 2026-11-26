import CoreText
import Foundation
@testable import GhostMarks
import SwiftUI
import Testing

private func family(_ font: CTFont) -> String { CTFontCopyFamilyName(font) as String }

private func glyphWidth(_ font: CTFont, _ character: Character) -> CGFloat {
    var units = Array(String(character).utf16)
    var glyphs = [CGGlyph](repeating: 0, count: units.count)
    CTFontGetGlyphsForCharacters(font, &units, &glyphs, units.count)
    return CTFontGetBoundingRectsForGlyphs(font, .horizontal, glyphs, nil, glyphs.count).width
}

/// The width of a shaped line, so OpenType features apply.
private func lineWidth(_ font: CTFont, _ text: String) -> CGFloat {
    let string = NSAttributedString(string: text, attributes: [NSAttributedString.Key(kCTFontAttributeName as String): font])
    return CGFloat(CTLineGetTypographicBounds(CTLineCreateWithAttributedString(string), nil, nil, nil))
}

@Suite("Fonts")
struct FontTests {
    @Test("registers Geist and Geist Mono once, by their family names")
    func registers() throws {
        #expect(GhostFonts.register())
        #expect(GhostFonts.register())
        #expect(GhostFonts.fontURLs.map(\.lastPathComponent) == ["Geist[wght].ttf", "GeistMono[wght].ttf"])
        #expect(family(CTFontCreateWithName(GhostFonts.sansPostScriptName as CFString, 12, nil)) == "Geist")
        #expect(family(CTFontCreateWithName(GhostFonts.monoPostScriptName as CFString, 12, nil)) == "Geist Mono")
        // The licence travels with the fonts.
        let folder = try #require(GhostFonts.fontURLs.first).deletingLastPathComponent()
        let licence = try String(contentsOf: folder.appendingPathComponent("OFL.txt"), encoding: .utf8)
        #expect(licence.contains("SIL OPEN FONT LICENSE Version 1.1"))
    }

    @Test("sets 420, 510 and 600 on the wght axis, and the axis changes the drawing")
    func weights() {
        #expect(GhostWeight.allCases.map(\.value) == [420, 510, 600])
        for mono in [false, true] {
            var widths: [CGFloat] = []
            for weight in [400, 420, 510, 600] as [CGFloat] {
                let font = GhostFont.ctFont(size: 100, weight: weight, mono: mono, tabular: false)
                #expect(family(font) == (mono ? "Geist Mono" : "Geist"))
                // CoreText leaves the axis default, 400, out of the variation it reports.
                let variation = CTFontCopyVariation(font) as? [NSNumber: NSNumber]
                let reported = variation?[NSNumber(value: GhostFont.wghtAxis)]?.doubleValue ?? 400
                #expect(reported == Double(weight), "wght \(weight)")
                widths.append(glyphWidth(font, "H"))
            }
            // Heavier is wider, step by step, so the variation reaches the outlines.
            #expect(widths == widths.sorted() && Set(widths).count == widths.count, "\(widths)")
        }
    }

    @Test("sets tabular figures for times, and proportional ones elsewhere")
    func tabular() {
        let tabular = GhostFont.ctFont(size: 12, weight: 420, mono: false, tabular: true)
        let widths = Set("0123456789".map { lineWidth(tabular, String(repeating: $0, count: 4)) })
        #expect(widths.count == 1, "\(widths)")
        let proportional = GhostFont.ctFont(size: 12, weight: 420, mono: false, tabular: false)
        #expect(lineWidth(proportional, "1111") < lineWidth(proportional, "0000"))
    }

    @Test("keeps the type scale")
    func scale() {
        let roles: [(GhostTextStyle, CGFloat, CGFloat, GhostWeight, Bool)] = [
            (.caption, 11, 16, .regular, false), (.small, 12, 16, .regular, false),
            (.interface, 13, 20, .regular, false), (.chat, 14, 22, .regular, false),
            (.prose, 15, 24, .regular, false), (.heading, 17, 24, .semibold, false),
            (.title, 22, 28, .medium, false), (.display, 26, 32, .medium, false),
            (.code, 13, 20, .regular, true), (.terminal, 12.5, 18, .regular, true),
            (.rowTitle, 15, 21, .medium, false), (.meta, 13, 18, .regular, false),
            (.time, 12, 16, .regular, false), (.composer, 16, 22, .regular, false),
            (.sheetTitle, 18, 24, .medium, false), (.barTitle, 16, 22, .medium, false),
        ]
        for (style, size, lineHeight, weight, mono) in roles {
            #expect(style.size == size && style.lineHeight == lineHeight && style.weight == weight && style.mono == mono)
        }
        #expect(GhostTextStyle.heading.tracking == -0.008)
        #expect(GhostTextStyle.title.tracking == -0.012)
        #expect(GhostTextStyle.display.tracking == -0.015)
        #expect(GhostTextStyle.time.tabular)
        #if os(macOS)
        #expect(GhostFont.scaled(15, relativeTo: .body) == 15)
        #endif
        #expect(GhostFont.bolder(420) == 600)
        #expect(GhostFont.bolder(800) == 900)
    }
}
