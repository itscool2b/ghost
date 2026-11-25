import CoreText
import Foundation
@testable import O7Marks
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
        #expect(O7Fonts.register())
        #expect(O7Fonts.register())
        #expect(O7Fonts.fontURLs.map(\.lastPathComponent) == ["Geist[wght].ttf", "GeistMono[wght].ttf"])
        #expect(family(CTFontCreateWithName(O7Fonts.sansPostScriptName as CFString, 12, nil)) == "Geist")
        #expect(family(CTFontCreateWithName(O7Fonts.monoPostScriptName as CFString, 12, nil)) == "Geist Mono")
        // The licence travels with the fonts.
        let folder = try #require(O7Fonts.fontURLs.first).deletingLastPathComponent()
        let licence = try String(contentsOf: folder.appendingPathComponent("OFL.txt"), encoding: .utf8)
        #expect(licence.contains("SIL OPEN FONT LICENSE Version 1.1"))
    }

    @Test("sets 420, 510 and 600 on the wght axis, and the axis changes the drawing")
    func weights() {
        #expect(O7Weight.allCases.map(\.value) == [420, 510, 600])
        for mono in [false, true] {
            var widths: [CGFloat] = []
            for weight in [400, 420, 510, 600] as [CGFloat] {
                let font = O7Font.ctFont(size: 100, weight: weight, mono: mono, tabular: false)
                #expect(family(font) == (mono ? "Geist Mono" : "Geist"))
                // CoreText leaves the axis default, 400, out of the variation it reports.
                let variation = CTFontCopyVariation(font) as? [NSNumber: NSNumber]
                let reported = variation?[NSNumber(value: O7Font.wghtAxis)]?.doubleValue ?? 400
                #expect(reported == Double(weight), "wght \(weight)")
                widths.append(glyphWidth(font, "H"))
            }
            // Heavier is wider, step by step, so the variation reaches the outlines.
            #expect(widths == widths.sorted() && Set(widths).count == widths.count, "\(widths)")
        }
    }

    @Test("sets tabular figures for times, and proportional ones elsewhere")
    func tabular() {
        let tabular = O7Font.ctFont(size: 12, weight: 420, mono: false, tabular: true)
        let widths = Set("0123456789".map { lineWidth(tabular, String(repeating: $0, count: 4)) })
        #expect(widths.count == 1, "\(widths)")
        let proportional = O7Font.ctFont(size: 12, weight: 420, mono: false, tabular: false)
        #expect(lineWidth(proportional, "1111") < lineWidth(proportional, "0000"))
    }

    @Test("keeps the type scale")
    func scale() {
        let roles: [(O7TextStyle, CGFloat, CGFloat, O7Weight, Bool)] = [
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
        #expect(O7TextStyle.heading.tracking == -0.008)
        #expect(O7TextStyle.title.tracking == -0.012)
        #expect(O7TextStyle.display.tracking == -0.015)
        #expect(O7TextStyle.time.tabular)
        #if os(macOS)
        #expect(O7Font.scaled(15, relativeTo: .body) == 15)
        #endif
        #expect(O7Font.bolder(420) == 600)
        #expect(O7Font.bolder(800) == 900)
    }
}
