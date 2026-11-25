public import CoreGraphics
public import O7Theme
public import SwiftUI
import Synchronization

/// An agent's own published mark, exactly as its maker publishes it (third-party/brand-marks.md):
/// never recoloured, stretched or rotated, except that Hermes is turned 45 degrees as Nous
/// Research shows it, and Grok's `currentColor` takes o7's foreground. Data in
/// AgentLogos.generated.swift, written from the desktop's stored SVG files.
public struct AgentLogo: Sendable {
    /// The brand id the mark belongs to.
    public let id: String
    public let name: String
    /// The stored file's viewBox, cropped to the artwork.
    public let viewBox: CGRect
    /// The square the mark is drawn in at its optical scale (`logoViewBox` in AgentLogo.tsx): the
    /// stored box, or the turned frame, widened evenly.
    public let drawBox: CGRect
    /// Optical size within the logo box, above 0.5 and at most 1. Uniform scaling only.
    public let scale: Double
    /// A turn the maker shows the mark with, about the stored viewBox's centre.
    public let turn: LogoTurn?
    public let shapes: [LogoShape]

    /// The agent's mark, by id or alias ("claude", "Claude Code"); nil when it has none.
    public static func logo(for agent: String) -> AgentLogo? {
        let brand = AgentBrand.lookup(agent)
        return brand.isGeneric ? nil : all[brand.id]
    }

    /// Every stored mark, in brand order.
    public static var allLogos: [AgentLogo] {
        AgentBrand.all.compactMap { all[$0.id] }
    }
}

/// A turn of a whole mark: degrees clockwise about a point in the mark's own coordinates.
public struct LogoTurn: Sendable {
    public let degrees: Double
    public let centre: CGPoint
}

/// One flat shape of a stored mark.
public struct LogoShape: Sendable {
    public enum Geometry: Sendable, Hashable {
        /// SVG path data, using only M L H V C Z and their relative forms.
        case path(String)
        case rect(x: Double, y: Double, width: Double, height: Double, rx: Double, ry: Double)
        case ellipse(cx: Double, cy: Double, rx: Double, ry: Double)
    }

    public enum Fill: Sendable, Hashable {
        /// The maker's own colour.
        case color(RGBA)
        /// `currentColor`: o7's foreground, fg-1.
        case foreground
    }

    public let geometry: Geometry
    public let fill: Fill
    /// The even-odd fill rule, where the file asks for it.
    public let evenOdd: Bool
    /// The shape's opacity and fill opacity together.
    public let opacity: Double

    init(_ geometry: Geometry, fill: Fill, evenOdd: Bool, opacity: Double) {
        self.geometry = geometry
        self.fill = fill
        self.evenOdd = evenOdd
        self.opacity = opacity
    }

    /// The shape in the mark's own coordinates.
    public func path() throws -> Path {
        switch geometry {
        case .path(let data):
            return try SVGPathParser.path(data)
        case .rect(let x, let y, let width, let height, let rx, let ry):
            let rect = CGRect(x: x, y: y, width: width, height: height)
            // SVG clamps a corner to half the side.
            let corner = CGSize(width: min(rx, width / 2), height: min(ry, height / 2))
            return corner.width > 0 && corner.height > 0
                ? Path(roundedRect: rect, cornerSize: corner, style: .circular) : Path(rect)
        case .ellipse(let cx, let cy, let rx, let ry):
            return Path(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: 2 * rx, height: 2 * ry))
        }
    }
}

extension AgentLogo {
    private static let parsed = Mutex<[String: [Path]]>([:])

    /// The shapes as paths, parsed on first use and kept. A shape that fails to parse is left
    /// out rather than drawn wrong; the tests parse every stored mark.
    var paths: [Path] {
        if let paths = Self.parsed.withLock({ $0[id] }) { return paths }
        let paths = shapes.map { (try? $0.path()) ?? Path() }
        Self.parsed.withLock { $0[id] = paths }
        return paths
    }

    /// The transform from the mark's coordinates into a square `rect`, turn included.
    public func transform(into rect: CGRect) -> CGAffineTransform {
        let k = rect.width / drawBox.width
        var transform = CGAffineTransform(translationX: rect.minX, y: rect.minY)
            .scaledBy(x: k, y: k)
            .translatedBy(x: -drawBox.minX, y: -drawBox.minY)
        if let turn {
            transform = transform
                .translatedBy(x: turn.centre.x, y: turn.centre.y)
                .rotated(by: turn.degrees * .pi / 180)
                .translatedBy(x: -turn.centre.x, y: -turn.centre.y)
        }
        return transform
    }

    /// Draws the mark into a square `rect`; `currentColor` takes `foreground`.
    public func draw(in context: inout GraphicsContext, rect: CGRect, foreground: RGBA) {
        let transform = transform(into: rect)
        for (shape, path) in zip(shapes, paths) {
            let color: RGBA
            switch shape.fill {
            case .color(let rgba): color = rgba
            case .foreground: color = foreground
            }
            context.fill(
                path.applying(transform),
                with: .color(Color(color.opacity(color.a * shape.opacity))),
                style: FillStyle(eoFill: shape.evenOdd)
            )
        }
    }
}

/// An agent's own mark alone, centred at its optical scale in a square of `size` (AgentLogo.tsx).
/// Nothing for an agent without one; `AgentMark` is the tile that always shows something. It is
/// decorative: the agent's name sits beside it.
public struct AgentLogoView: View {
    private let agent: String
    private let size: CGFloat
    @Environment(\.o7Palette) private var palette

    public init(agent: String, size: CGFloat = 16) {
        self.agent = agent
        self.size = size
    }

    public var body: some View {
        if let logo = AgentLogo.logo(for: agent) {
            let box = CGFloat(size.isFinite ? max(8, min(512, PixelGeometry.jsRound(Double(size)))) : 16)
            let foreground = palette.fg1
            Canvas { context, canvas in
                logo.draw(in: &context, rect: CGRect(origin: .zero, size: canvas), foreground: foreground)
            }
            .frame(width: box, height: box)
            .accessibilityHidden(true)
        }
    }
}
