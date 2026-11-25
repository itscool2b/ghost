public import CoreGraphics
public import Foundation

/// A rectangle on whole points, as the desktop's pixel marks lay them out.
public struct PixelRect: Sendable, Hashable {
    public let x: Int
    public let y: Int
    public let w: Int
    public let h: Int

    public init(x: Int, y: Int, w: Int, h: Int) {
        self.x = x
        self.y = y
        self.w = w
        self.h = h
    }

    public var cgRect: CGRect { CGRect(x: x, y: y, width: w, height: h) }
}

/// How fast a loader walks: thinking slowest, streaming fastest.
public enum LoaderTempo: String, CaseIterable, Sendable {
    case thinking
    case streaming
    case tool

    /// Milliseconds per step, as `LOADER_STEP_MS` in pixels.ts.
    public var stepMilliseconds: Int {
        switch self {
        case .thinking: 130
        case .tool: 110
        case .streaming: 90
        }
    }

    /// Seconds per step.
    public var interval: TimeInterval { TimeInterval(stepMilliseconds) / 1000 }

    /// The step a loader at this tempo shows at `date`. Steps count from the Unix epoch, so every
    /// loader at the same tempo lights the same cell at once.
    public func step(at date: Date) -> Int {
        let steps = date.timeIntervalSince1970 * 1000 / Double(stepMilliseconds)
        // A schedule's dates land on whole steps; the nudge keeps rounding error from
        // reading one as the step before.
        return Int((steps + 1e-6).rounded(.down))
    }
}

/// The desktop's pixel geometry (packages/ui/src/graphite/pixels.ts), ported exactly: every
/// shape lands on whole points inside a square box.
public enum PixelGeometry {
    /// The loader's cells in walking order. Cells are numbered row-major, two per row:
    /// `0 1 / 2 3 / 4 5`; a light walks the perimeter clockwise with a two-cell trail.
    public static let loaderOrder: [Int] = [0, 1, 3, 5, 4, 2]

    /// Brightness per loader cell at a step: 3 for the head, 2 and 1 for the trail, 0 at rest.
    public static func loaderFrame(_ step: Int) -> [Int] {
        var levels = [0, 0, 0, 0, 0, 0]
        let n = loaderOrder.count
        let head = ((step % n) + n) % n
        levels[loaderOrder[head]] = 3
        levels[loaderOrder[(head + n - 1) % n]] = 2
        levels[loaderOrder[(head + n - 2) % n]] = 1
        return levels
    }

    /// The loader's six cells inside a square box of `size` points.
    public static func loaderRects(size: Int) -> [PixelRect] {
        let cell = max(2, jsRound(Double(size) * 0.125))
        let gap = max(1, jsRound(Double(size) * 0.0625))
        let w = 2 * cell + gap
        let h = 3 * cell + 2 * gap
        let x0 = floorHalf(size - w)
        let y0 = floorHalf(size - h)
        return (0..<6).map { i in
            PixelRect(x: x0 + (i % 2) * (cell + gap), y: y0 + (i / 2) * (cell + gap), w: cell, h: cell)
        }
    }

    /// A solid square of about `fraction` of the box, centred on whole points.
    public static func centredSquare(size: Int, fraction: Double) -> PixelRect {
        var side = max(2, jsRound(Double(size) * fraction))
        if (size - side) % 2 != 0 { side += 1 }
        let at = (size - side) / 2
        return PixelRect(x: at, y: at, w: side, h: side)
    }

    /// The done check as a grid, short arm first.
    public static let checkGrid: [String] = ["....#", "...#.", "#.#..", ".#..."]

    private static let checkOrder: [(x: Int, y: Int)] = [(0, 2), (1, 3), (2, 2), (3, 1), (4, 0)]

    /// The check's cells inside a square box of `size` points, in the order they draw.
    public static func checkRects(size: Int) -> [PixelRect] {
        let cell = max(2, jsRound(Double(size) / 8))
        let x0 = floorHalf(size - 5 * cell)
        let y0 = floorHalf(size - 4 * cell)
        return checkOrder.map { PixelRect(x: x0 + $0.x * cell, y: y0 + $0.y * cell, w: cell, h: cell) }
    }

    /// The square a logo is drawn in inside a tile of `size` (AgentMark.tsx `logoSide`): an even
    /// margin of 2 up to 16, then about a sixth of the tile.
    public static func logoSide(size: Int) -> Int {
        let margin = size <= 16 ? 2 : jsRound(Double(size) * 0.17)
        return size - 2 * margin
    }

    /// JavaScript's `Math.round`: the nearest integer, halves toward positive infinity.
    static func jsRound(_ value: Double) -> Int {
        let floor = value.rounded(.down)
        return Int(value - floor >= 0.5 ? floor + 1 : floor)
    }

    /// `Math.floor(value / 2)` for an integer, negative values included.
    private static func floorHalf(_ value: Int) -> Int {
        Int((Double(value) / 2).rounded(.down))
    }
}
