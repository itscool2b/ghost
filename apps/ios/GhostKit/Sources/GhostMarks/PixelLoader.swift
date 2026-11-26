public import GhostTheme
public import SwiftUI

/// The colour of a loader's lit head cell; the trail stays grey.
public enum LoaderTone: String, CaseIterable, Sendable {
    /// fg-1, white in dark.
    case neutral
    /// The theme's accent (volt).
    case accent
    case green
    case red
}

/// Six pixels in a 2x3 grid with a light walking round them: "working", without a spinner
/// (PixelLoader.tsx). Every loader steps from the Unix epoch, so all loaders at a tempo light the
/// same cell at once. Reduce Motion, or `animated: false` for widgets and the Live Activity, draws
/// the static frame.
public struct PixelLoader: View {
    private let size: CGFloat
    private let tempo: LoaderTempo
    private let tone: LoaderTone
    private let animated: Bool
    private let label: String?

    @Environment(\.ghostPalette) private var palette
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// A loader in a square box of `size` points (8 to 128; cells grow with it). Without a
    /// `label` it is decorative.
    public init(
        size: CGFloat = 12, tempo: LoaderTempo = .tool, tone: LoaderTone = .neutral, animated: Bool = true,
        label: String? = nil
    ) {
        self.size = size
        self.tempo = tempo
        self.tone = tone
        self.animated = animated
        self.label = label
    }

    public var body: some View {
        let box = Self.box(size)
        Group {
            if animated && !reduceMotion {
                TimelineView(.periodic(from: Date(timeIntervalSince1970: 0), by: tempo.interval)) { timeline in
                    LoaderCanvas(box: box, step: tempo.step(at: timeline.date), tone: tone, palette: palette)
                }
            } else {
                LoaderCanvas(box: box, step: 0, tone: tone, palette: palette)
            }
        }
        .frame(width: CGFloat(box), height: CGFloat(box))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label ?? "")
        .accessibilityAddTraits(label == nil ? [] : .isImage)
        .accessibilityHidden(label == nil)
    }

    /// The box a size is drawn in: whole points from 8 to 128, 12 for nonsense.
    nonisolated static func box(_ size: CGFloat) -> Int {
        size.isFinite ? max(8, min(128, PixelGeometry.jsRound(Double(size)))) : 12
    }

    /// A cell's colour at a brightness level (graphite.css `.g-px-*`): line-strong at rest, fg-4,
    /// fg-3, then fg-1 or the tone for the head.
    nonisolated static func cellColor(level: Int, tone: LoaderTone, palette: Palette) -> RGBA {
        switch level {
        case ...0: return palette.lineStrong
        case 1: return palette.fg4
        case 2: return palette.fg3
        default:
            switch tone {
            case .neutral: return palette.fg1
            case .accent: return palette.accent
            case .green: return palette.green
            case .red: return palette.red
            }
        }
    }

    /// Draws a loader frame into a square box of `box` points at `origin`.
    nonisolated static func draw(
        in context: inout GraphicsContext, box: Int, step: Int, tone: LoaderTone, palette: Palette,
        origin: CGPoint = .zero
    ) {
        let levels = PixelGeometry.loaderFrame(step)
        for (index, cell) in PixelGeometry.loaderRects(size: box).enumerated() {
            let color = cellColor(level: levels[index], tone: tone, palette: palette)
            context.fill(Path(cell.cgRect.offsetBy(dx: origin.x, dy: origin.y)), with: .color(Color(color)))
        }
    }
}

/// One loader frame; steps flip with no tween.
struct LoaderCanvas: View {
    let box: Int
    let step: Int
    let tone: LoaderTone
    let palette: Palette

    var body: some View {
        Canvas { context, _ in
            PixelLoader.draw(in: &context, box: box, step: step, tone: tone, palette: palette)
        }
    }
}
