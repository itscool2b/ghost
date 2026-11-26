import CoreGraphics
@testable import GhostMarks
import GhostTheme
import SwiftUI
import Testing

/// A view drawn at 1x into 8-bit sRGB, top row first.
@MainActor
private struct Bitmap {
    let width: Int
    let height: Int
    let bytes: [UInt8]

    init<Content: View>(_ view: Content, palette: Palette, size: Int) throws {
        let renderer = ImageRenderer(content: view.environment(\.ghostPalette, palette))
        renderer.scale = 1
        renderer.proposedSize = ProposedViewSize(width: CGFloat(size), height: CGFloat(size))
        let image = try #require(renderer.cgImage)
        let (w, h) = (image.width, image.height)
        var data = [UInt8](repeating: 0, count: w * h * 4)
        let space = try #require(CGColorSpace(name: CGColorSpace.sRGB))
        data.withUnsafeMutableBytes { buffer in
            let context = CGContext(
                data: buffer.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            context?.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
        }
        (width, height, bytes) = (w, h, data)
    }

    func pixel(_ x: Int, _ y: Int) -> (r: Int, g: Int, b: Int, a: Int) {
        let at = (y * width + x) * 4
        return (Int(bytes[at]), Int(bytes[at + 1]), Int(bytes[at + 2]), Int(bytes[at + 3]))
    }

    func matches(_ x: Int, _ y: Int, _ color: RGBA, tolerance: Int = 2) -> Bool {
        let p = pixel(x, y)
        let q = color.quantized
        return abs(p.r - Int(q.r * 255)) <= tolerance && abs(p.g - Int(q.g * 255)) <= tolerance
            && abs(p.b - Int(q.b * 255)) <= tolerance && abs(p.a - Int(q.a * 255)) <= tolerance
    }
}

@MainActor
@Suite("Marks as drawn")
struct RenderTests {
    let dark = Palettes.palette(.aura, dark: true)
    let light = Palettes.palette(.aura, dark: false)

    @Test("draws the tile, its ring and the loader's static frame on whole points")
    func loader() throws {
        let image = try Bitmap(AgentMark(agent: "pi", state: .thinking, size: 24, animated: false), palette: dark, size: 24)
        #expect(image.width == 24 && image.height == 24)
        let levels = PixelGeometry.loaderFrame(0)
        for (index, cell) in PixelGeometry.loaderRects(size: 24).enumerated() {
            let color = PixelLoader.cellColor(level: levels[index], tone: .neutral, palette: dark)
            #expect(image.matches(cell.x + 1, cell.y + 1, color), "cell \(index): \(image.pixel(cell.x + 1, cell.y + 1))")
        }
        #expect(image.matches(4, 12, dark.surface3))
        #expect(image.matches(0, 12, dark.lineStrong))
    }

    @Test("puts the waiting and error dots in the logo's place, centred")
    func dots() throws {
        let waiting = try Bitmap(AgentMark(agent: "pi", state: .waiting, size: 24), palette: dark, size: 24)
        #expect(waiting.matches(12, 12, dark.accent), "\(waiting.pixel(12, 12))")
        #expect(waiting.matches(12, 3, dark.surface3))
        let error = try Bitmap(AgentMark(agent: "pi", state: .error, size: 24), palette: dark, size: 24)
        #expect(error.matches(12, 12, dark.red))
    }

    @Test("draws the whole check when still")
    func check() throws {
        let image = try Bitmap(AgentMark(agent: "claude", state: .done, size: 24, animated: false), palette: dark, size: 24)
        for cell in PixelGeometry.checkRects(size: 24) {
            #expect(image.matches(cell.x + 1, cell.y + 1, dark.fg1), "\(cell)")
        }
        #expect(image.matches(5, 7, dark.surface3))
    }

    @Test("sets white marks on the brand plate, idle or a ghost, and a ghost on nothing")
    func plates() throws {
        let codex = try Bitmap(AgentMark(agent: "codex", size: 24), palette: light, size: 24)
        #expect(codex.matches(2, 12, light.brandPlate), "\(codex.pixel(2, 12))")
        let claude = try Bitmap(AgentMark(agent: "claude", size: 24), palette: light, size: 24)
        #expect(claude.matches(2, 12, light.surface3))
        let busyCodex = try Bitmap(AgentMark(agent: "codex", state: .tool, size: 24, animated: false), palette: light, size: 24)
        #expect(busyCodex.matches(2, 12, light.surface3))
        let ghostCursor = try Bitmap(AgentMark(agent: "cursor", state: .thinking, size: 24, ghost: true), palette: light, size: 24)
        #expect(ghostCursor.matches(2, 12, light.brandPlate))
        let ghostClaude = try Bitmap(AgentMark(agent: "claude", state: .thinking, size: 24, ghost: true), palette: light, size: 24)
        #expect(ghostClaude.pixel(2, 12).a == 0)
    }

    @Test("draws Grok's mark in the foreground, and nothing for an agent without a mark")
    func foreground() throws {
        // Rendered first: ImageRenderer reuses its drawables, so an empty view after another
        // shows the other's pixels.
        let none = try Bitmap(AgentLogoView(agent: "future-agent", size: 32).frame(width: 32, height: 32), palette: dark, size: 32)
        #expect(none.bytes.allSatisfy { $0 == 0 })
        let grok = try Bitmap(AgentLogoView(agent: "grok", size: 32), palette: dark, size: 32)
        let lit = (0..<32).flatMap { y in (0..<32).map { (x: $0, y: y) } }.filter { grok.matches($0.x, $0.y, dark.fg1, tolerance: 3) }
        #expect(lit.count > 20)
    }
}
