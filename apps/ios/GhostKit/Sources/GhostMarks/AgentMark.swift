public import GhostTheme
public import SwiftUI

/// An agent's identity and live state in one square tile (AgentMark.tsx). Idle, it shows the
/// agent's own published logo, or its two-letter code when it has none. Any other state takes the
/// logo's place, never drawn over it: a walking 2x3 loader while it works, an accent dot while it
/// waits on you, a pixel check that draws in once it is done, a red dot when it failed. A ghost
/// (not installed) is a dashed tile with the logo or code, whatever the state. Reduce Motion, or
/// `animated: false`, draws every state still.
public struct AgentMark: View {
    private let agent: String
    private let state: AgentState
    private let size: CGFloat
    private let ghost: Bool
    private let label: String?
    private let animated: Bool

    @Environment(\.ghostPalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var checkPixels: Int

    /// A mark for `agent` (an id or alias) in a tile of `size` points, 12 to 256. VoiceOver
    /// reads `label`, or "Claude Code, thinking".
    public init(
        agent: String, state: AgentState = .idle, size: CGFloat = 24, ghost: Bool = false, label: String? = nil,
        animated: Bool = true
    ) {
        self.agent = agent
        self.state = state
        self.size = size
        self.ghost = ghost
        self.label = label
        self.animated = animated
        _checkPixels = State(initialValue: Self.indicator(state, ghost: ghost) == .done ? 0 : Self.checkCount)
    }

    public var body: some View {
        let box = Self.box(size)
        let brand = AgentBrand.lookup(agent)
        let logo = AgentLogo.logo(for: agent)
        let indicator = Self.indicator(state, ghost: ghost)
        let tile = RoundedRectangle(cornerRadius: Self.cornerRadius(box), style: .circular)
        let moving = animated && !reduceMotion
        // Vendors' white marks keep their artwork on a contrasting plate, idle or a ghost.
        let plate = logo != nil && brand.plate && (state == .idle || ghost)

        ZStack {
            if plate {
                tile.fill(Color(palette.brandPlate))
            } else if !ghost {
                tile.fill(Color(palette.surface3))
            }
            if ghost {
                tile.inset(by: 0.5).stroke(Color(palette.lineStrong), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
            } else {
                tile.strokeBorder(Color(palette.lineStrong), lineWidth: 1)
            }
            content(indicator: indicator, logo: logo, box: box, moving: moving)
        }
        .frame(width: CGFloat(box), height: CGFloat(box))
        .clipShape(tile)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label ?? Self.spokenLabel(agent: agent, state: state))
        .accessibilityAddTraits(.isImage)
        .task(id: CheckRun(drawing: indicator == .done && moving)) {
            await drawCheck(indicator == .done && moving)
        }
    }

    @ViewBuilder
    private func content(indicator: Indicator, logo: AgentLogo?, box: Int, moving: Bool) -> some View {
        let palette = palette
        switch indicator {
        case .glyph:
            if let logo {
                let side = CGFloat(PixelGeometry.logoSide(size: box))
                let foreground = palette.fg1
                Canvas { context, _ in
                    logo.draw(in: &context, rect: CGRect(x: 0, y: 0, width: side, height: side), foreground: foreground)
                }
                .frame(width: side, height: side)
            } else {
                glyph(box: box)
            }
        case .loader(let tempo):
            if moving {
                TimelineView(.periodic(from: Date(timeIntervalSince1970: 0), by: tempo.interval)) { timeline in
                    LoaderCanvas(box: box, step: tempo.step(at: timeline.date), tone: .neutral, palette: palette)
                }
            } else {
                LoaderCanvas(box: box, step: 0, tone: .neutral, palette: palette)
            }
        case .waiting, .error:
            let dot = PixelGeometry.centredSquare(size: box, fraction: 0.375)
            let color = indicator == .waiting ? palette.accent : palette.red
            Canvas { context, _ in
                context.fill(Path(ellipseIn: dot.cgRect), with: .color(Color(color)))
            }
        case .done:
            let shown = moving ? checkPixels : Self.checkCount
            Canvas { context, _ in
                for cell in PixelGeometry.checkRects(size: box).prefix(shown) {
                    context.fill(Path(cell.cgRect), with: .color(Color(palette.fg1)))
                }
            }
        }
    }

    /// The two-letter code, sized to fit its tile and never under 7 points.
    private func glyph(box: Int) -> some View {
        let code = agentCode(agent)
        let fontSize = code.utf16.count > 1
            ? max(7, PixelGeometry.jsRound(Double(box) * (box < 24 ? 0.5 : 0.38)))
            : PixelGeometry.jsRound(Double(box) * 0.56)
        let size = CGFloat(fontSize)
        // Below 12 a 500 stem drops under one pixel on a 1x screen; 600 keeps it whole.
        return Text(code)
            .font(GhostFont.font(size: size, weight: fontSize < 12 ? 600 : 500, mono: false, tabular: false))
            .tracking(-0.02 * size)
            .foregroundStyle(Color(ghost ? palette.fg3 : palette.fg2))
            .lineLimit(1)
            .fixedSize()
            // The desktop sets the code with line-height 1 and 0.06em on top, which sits it
            // 0.03em lower than a centred SwiftUI line.
            .offset(y: 0.03 * size)
    }

    /// Draws the check a pixel at a time: each pixel lights 40 ms after its turn, turns 50 ms
    /// apart, as the desktop's `g-pixel-on` animation.
    private func drawCheck(_ drawing: Bool) async {
        guard drawing else {
            checkPixels = Self.checkCount
            return
        }
        checkPixels = 0
        for index in 0..<Self.checkCount {
            try? await Task.sleep(for: .milliseconds(index == 0 ? 40 : 50))
            if Task.isCancelled { return }
            checkPixels = index + 1
        }
    }

    private struct CheckRun: Hashable {
        let drawing: Bool
    }

    private static let checkCount = 5

    /// What VoiceOver reads without a label: the brand's name and the state, "Claude Code, thinking".
    nonisolated static func spokenLabel(agent: String, state: AgentState) -> String {
        "\(AgentBrand.lookup(agent).name), \(state.label)"
    }

    enum Indicator: Equatable {
        case glyph
        case loader(LoaderTempo)
        case waiting
        case done
        case error
    }

    nonisolated static func indicator(_ state: AgentState, ghost: Bool) -> Indicator {
        if ghost { return .glyph }
        switch state {
        case .idle: return .glyph
        case .thinking, .streaming, .tool: return .loader(state.tempo ?? .tool)
        case .waiting: return .waiting
        case .done: return .done
        case .error: return .error
        }
    }

    /// The tile a size is drawn in: whole points from 12 to 256, 24 for nonsense.
    nonisolated static func box(_ size: CGFloat) -> Int {
        size.isFinite ? max(12, min(256, PixelGeometry.jsRound(Double(size)))) : 24
    }

    /// The tile's corner radius: 22% of the tile, at least 3.
    nonisolated static func cornerRadius(_ box: Int) -> CGFloat {
        CGFloat(max(3, PixelGeometry.jsRound(Double(box) * 0.22)))
    }
}
