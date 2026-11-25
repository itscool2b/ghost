import Foundation
@testable import O7Marks
import Testing

private func rects(_ flat: [Int], at index: Int, count: Int) -> [PixelRect] {
    (0..<count).map { cell in
        let base = (index * count + cell) * 4
        return PixelRect(x: flat[base], y: flat[base + 1], w: flat[base + 2], h: flat[base + 3])
    }
}

@Suite("Pixel geometry against pixels.ts")
struct PixelGeometryTests {
    @Test("lays the loader's cells exactly as pixels.ts at every size")
    func loaderRects() {
        for (index, size) in PixelGoldens.sizes.enumerated() {
            #expect(PixelGeometry.loaderRects(size: size) == rects(PixelGoldens.loaderRects, at: index, count: 6), "size \(size)")
        }
    }

    @Test("lays the check's cells exactly as pixels.ts, in drawing order")
    func checkRects() {
        for (index, size) in PixelGoldens.sizes.enumerated() {
            #expect(PixelGeometry.checkRects(size: size) == rects(PixelGoldens.checkRects, at: index, count: 5), "size \(size)")
        }
        #expect(PixelGeometry.checkGrid == PixelGoldens.checkGrid)
    }

    @Test("centres squares exactly as pixels.ts")
    func centredSquares() {
        let fractions = PixelGoldens.fractions
        for (index, size) in PixelGoldens.sizes.enumerated() {
            for (offset, fraction) in fractions.enumerated() {
                let expected = rects(PixelGoldens.centredSquares, at: index * fractions.count + offset, count: 1)[0]
                #expect(PixelGeometry.centredSquare(size: size, fraction: fraction) == expected, "size \(size) at \(fraction)")
            }
        }
    }

    @Test("walks the loader exactly as pixels.ts")
    func frames() {
        #expect(PixelGeometry.loaderOrder == PixelGoldens.loaderOrder)
        for (index, step) in PixelGoldens.steps.enumerated() {
            let expected = Array(PixelGoldens.loaderFrames[(index * 6)..<(index * 6 + 6)])
            #expect(PixelGeometry.loaderFrame(step) == expected, "step \(step)")
        }
        #expect(PixelGeometry.loaderFrame(0) == [3, 0, 2, 0, 1, 0])
    }

    @Test("keeps the desktop's tempos")
    func tempos() {
        for tempo in LoaderTempo.allCases {
            #expect(PixelGoldens.stepMilliseconds[tempo.rawValue] == tempo.stepMilliseconds)
        }
        #expect(LoaderTempo.streaming.stepMilliseconds < LoaderTempo.tool.stepMilliseconds)
        #expect(LoaderTempo.tool.stepMilliseconds < LoaderTempo.thinking.stepMilliseconds)
    }

    @Test("counts steps from the epoch, so every loader at a tempo steps together")
    func steps() {
        for tempo in LoaderTempo.allCases {
            for n in [0, 1, 5, 6, 1_000, 14_000_000_000] {
                let date = Date(timeIntervalSince1970: TimeInterval(n) * tempo.interval)
                #expect(tempo.step(at: date) == n, "\(tempo) step \(n)")
                #expect(tempo.step(at: date.addingTimeInterval(tempo.interval * 0.99)) == n)
            }
        }
    }

    @Test("leaves a logo an even margin, as logoSide")
    func logoSides() {
        for (index, size) in (12...256).enumerated() {
            #expect(PixelGeometry.logoSide(size: size) == MarkGoldens.logoSides[index], "size \(size)")
        }
        #expect([12, 14, 16, 20, 24, 32, 48, 56].map { PixelGeometry.logoSide(size: $0) } == [8, 10, 12, 14, 16, 22, 32, 36])
    }

    @Test("rounds as JavaScript's Math.round")
    func rounding() {
        #expect(PixelGeometry.jsRound(2.5) == 3)
        #expect(PixelGeometry.jsRound(-2.5) == -2)
        #expect(PixelGeometry.jsRound(0.49999999999999994) == 0)
        #expect(PixelGeometry.jsRound(5.28) == 5)
    }
}

@Suite("Brands and codes against brand.ts and AgentMark.tsx")
struct BrandTests {
    @Test("resolves every sample to the brand, name and code the desktop gives it")
    func samples() {
        for sample in MarkGoldens.agents {
            #expect(AgentBrand.lookup(sample.agent).id == sample.brand, "brand of \(sample.agent.debugDescription)")
            #expect(brandName(sample.agent) == sample.name, "name of \(sample.agent.debugDescription)")
            #expect(agentCode(sample.agent) == sample.code, "code of \(sample.agent.debugDescription)")
        }
    }

    @Test("gives every designed agent a distinct two-letter code")
    func codes() {
        let codes = AgentBrand.all.filter { !$0.isGeneric }.map(\.id).map(agentCode)
        #expect(codes == ["CC", "CX", "PI", "AG", "CU", "GB", "OC", "MP", "HM"])
        #expect(Set(codes).count == codes.count)
        #expect(agentCode("Claude Code") == "CC")
        #expect(agentCode("cursor-agent") == "CU")
        #expect(agentCode("gemini-cli") == "GE")
        #expect(agentCode("generic") == "?")
        #expect(agentCode("") == "?")
        #expect(agentCode("!!") == "?")
    }

    @Test("reads states leniently and labels them as the desktop")
    func states() throws {
        #expect(AgentState(lenient: "sleeping") == .idle)
        #expect(AgentState(lenient: "tool") == .tool)
        #expect(AgentMark.spokenLabel(agent: "claude", state: .waiting) == "Claude Code, waiting for approval")
        #expect(AgentMark.spokenLabel(agent: "codex", state: .streaming) == "Codex, responding")
        #expect(AgentMark.spokenLabel(agent: "future-agent", state: .tool) == "Agent, using a tool")
        let decoded = try JSONDecoder().decode([AgentState].self, from: Data(#"["done","nope"]"#.utf8))
        #expect(decoded == [.done, .idle])
    }

    @Test("puts the white marks on the brand plate, as graphite.css")
    func plates() {
        let plated = AgentBrand.all.filter(\.plate).map(\.id)
        #expect(plated == ["codex", "pi", "antigravity", "cursor", "opencode", "oh-my-pi", "hermes"])
    }
}
