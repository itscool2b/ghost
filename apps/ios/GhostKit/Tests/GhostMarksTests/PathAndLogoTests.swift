import CoreGraphics
@testable import GhostMarks
import GhostTheme
import SwiftUI
import Testing

private func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: x, y: y) }

@Suite("SVG path parser")
struct SVGPathParserTests {
    @Test("reads absolute and relative commands into absolute steps")
    func commands() throws {
        #expect(try SVGPathParser.commands("M2 2h4v4h-4z") == [
            .move(p(2, 2)), .line(p(6, 2)), .line(p(6, 6)), .line(p(2, 6)), .close,
        ])
        #expect(try SVGPathParser.commands("M10 10L20 10V30H5Z") == [
            .move(p(10, 10)), .line(p(20, 10)), .line(p(20, 30)), .line(p(5, 30)), .close,
        ])
        #expect(try SVGPathParser.commands("m1 1 2 0 0 2z m1 1l1 1") == [
            .move(p(1, 1)), .line(p(3, 1)), .line(p(3, 3)), .close, .move(p(2, 2)), .line(p(3, 3)),
        ])
        #expect(try SVGPathParser.commands("M0 0C1 2 3 4 5 6c1 1 2 2 3 3") == [
            .move(p(0, 0)),
            .curve(to: p(5, 6), control1: p(1, 2), control2: p(3, 4)),
            .curve(to: p(8, 9), control1: p(6, 7), control2: p(7, 8)),
        ])
    }

    @Test("reads SVG's number grammar as the stored marks write it")
    func numbers() throws {
        // Cursor's commas and run-together negatives, relative to the current point.
        expectClose(try SVGPathParser.commands("M457.43,125.94L244.42,2.96c-6.84-3.95-15.28-3.95-22.12,0"), [
            .move(p(457.43, 125.94)), .line(p(244.42, 2.96)),
            .curve(to: p(222.3, 2.96), control1: p(237.58, -0.99), control2: p(229.14, -0.99)),
        ])
        // Hermes's leading dots run together, and exponents.
        expectClose(try SVGPathParser.commands("M.5.5l.2.4v-.1h1e1l1E-1-2.5e+1"), [
            .move(p(0.5, 0.5)), .line(p(0.7, 0.9)), .line(p(0.7, 0.8)), .line(p(10.7, 0.8)),
            .line(p(10.8, -24.2)),
        ])
        #expect(try SVGPathParser.commands(" M 1 , 2 \n L+3\t4 ") == [.move(p(1, 2)), .line(p(3, 4))])
    }

    private func expectClose(
        _ actual: [SVGPathParser.Command], _ expected: [SVGPathParser.Command],
        sourceLocation: SourceLocation = #_sourceLocation
    ) {
        func points(_ command: SVGPathParser.Command) -> [CGPoint] {
            switch command {
            case .move(let point), .line(let point): [point]
            case .curve(let to, let c1, let c2): [to, c1, c2]
            case .close: []
            }
        }
        #expect(actual.count == expected.count, sourceLocation: sourceLocation)
        for (a, e) in zip(actual, expected) {
            let (pa, pe) = (points(a), points(e))
            let same = pa.count == pe.count && zip(pa, pe).allSatisfy { abs($0.x - $1.x) < 1e-9 && abs($0.y - $1.y) < 1e-9 }
            #expect(same, "\(a) is not \(e)", sourceLocation: sourceLocation)
        }
    }

    @Test("refuses what it does not read")
    func refusals() throws {
        for bad in ["L1 1", "1 1", "M1", "M1 1 Z 2 2", "M1 1A1 1 0 0 1 2 2", "M1 1Q1 1 2 2", "M1,,1", "M1 1L.", "M1 1L1e"] {
            #expect(throws: SVGPathParser.Failure.self, "\(bad)") { try SVGPathParser.commands(bad) }
        }
        #expect(try SVGPathParser.commands("").isEmpty)
    }

    @Test("parses every stored mark, inside its viewBox")
    func storedMarks() throws {
        #expect(AgentLogo.allLogos.count == 9)
        for logo in AgentLogo.allLogos {
            var bounds = CGRect.null
            for shape in logo.shapes {
                let path = try shape.path()
                #expect(!path.isEmpty, "\(logo.id)")
                bounds = bounds.union(path.boundingRect)
            }
            let slack = max(logo.viewBox.width, logo.viewBox.height) * 0.01
            #expect(logo.viewBox.insetBy(dx: -slack, dy: -slack).contains(bounds), "\(logo.id): \(bounds) outside \(logo.viewBox)")
            #expect(logo.paths.count == logo.shapes.count)
        }
    }
}

@Suite("Agent logos")
struct AgentLogoTests {
    @Test("draws a logo for each designed agent and for nothing else")
    func coverage() {
        for brand in AgentBrand.all {
            #expect((AgentLogo.logo(for: brand.id) == nil) == brand.isGeneric, "\(brand.id)")
        }
        #expect(AgentLogo.logo(for: "claude")?.id == "claude-code")
        #expect(AgentLogo.logo(for: "Claude Code")?.id == "claude-code")
        #expect(AgentLogo.logo(for: "future-agent") == nil)
        #expect(AgentLogo.logo(for: "generic") == nil)
        #expect(AgentLogo.logo(for: "") == nil)
    }

    @Test("keeps the vendors' colours, Grok in the foreground, Hermes even-odd")
    func fills() throws {
        let claude = try #require(AgentLogo.logo(for: "claude-code"))
        #expect(claude.shapes.map(\.fill) == [.color(RGBA(hex: 0xd97757))])
        let grok = try #require(AgentLogo.logo(for: "grok"))
        #expect(grok.shapes.allSatisfy { $0.fill == .foreground })
        let hermes = try #require(AgentLogo.logo(for: "hermes"))
        #expect(hermes.shapes.allSatisfy { $0.evenOdd })
        #expect(hermes.shapes.map(\.fill) == [.color(RGBA(hex: 0xf2f2f2)), .color(RGBA(hex: 0x0000f2))])
        let ohMyPi = try #require(AgentLogo.logo(for: "oh-my-pi"))
        #expect(ohMyPi.shapes.filter { $0.opacity == 0.8 }.count == 2)
        for logo in AgentLogo.allLogos {
            #expect(logo.scale > 0.5 && logo.scale <= 1, "\(logo.id)")
            #expect(logo.drawBox.width == logo.drawBox.height, "\(logo.id)")
        }
    }

    @Test("turns only Hermes, 45 degrees about the stored viewBox's centre")
    func turn() throws {
        #expect(AgentLogo.allLogos.filter { $0.turn != nil }.map(\.id) == ["hermes"])
        let hermes = try #require(AgentLogo.logo(for: "hermes"))
        #expect(hermes.turn?.degrees == 45)
        #expect(hermes.turn?.centre == CGPoint(x: 19.55, y: 39.25))
        // The turned wing lands inside its square.
        let rect = CGRect(x: 0, y: 0, width: 32, height: 32)
        var bounds = CGRect.null
        for path in hermes.paths { bounds = bounds.union(path.applying(hermes.transform(into: rect)).boundingRect) }
        #expect(rect.insetBy(dx: -0.5, dy: -0.5).contains(bounds), "\(bounds)")
    }
}

@Suite("The Ghost wing")
struct GhostLogoTests {
    @Test("fits the rect it is given at the wing's own proportion, centred")
    func fits() {
        let rect = CGRect(x: 0, y: 0, width: 200, height: 200)
        let bounds = GhostLogo().path(in: rect).boundingRect
        #expect(abs(bounds.width - 200) < 1)
        #expect(abs(bounds.midY - 100) < 6)
        #expect(abs(GhostLogo.aspectRatio - 1044.0 / 440.0) < 1e-12)
        #expect(GhostLogo.smallEyesMaxWidth == 40)
        #expect(GhostLogo(width: 40).smallEyes)
        #expect(!GhostLogo(width: 41).smallEyes)
    }

    @Test("draws the eyes as holes under either fill rule, larger in the small variant")
    func eyes() {
        let rect = CGRect(x: 0, y: 0, width: 1044, height: 440)
        let full = GhostLogo().path(in: rect)
        let small = GhostLogo(smallEyes: true).path(in: rect)
        let body = CGPoint(x: 522, y: 300)
        let eye = CGPoint(x: 470, y: 205)
        let smallEyeOnly = CGPoint(x: 395, y: 215)
        for eoFill in [false, true] {
            #expect(full.contains(body, eoFill: eoFill))
            #expect(!full.contains(eye, eoFill: eoFill))
            #expect(full.contains(smallEyeOnly, eoFill: eoFill))
            #expect(!small.contains(smallEyeOnly, eoFill: eoFill))
            #expect(small.contains(body, eoFill: eoFill))
        }
    }
}
