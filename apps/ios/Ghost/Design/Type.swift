import GhostMarks
import SwiftUI

extension GhostTextStyle {
    /// Geist at `size`, with Ghost's usual line height for it, scaling as `relativeTo` scales.
    static func ui(_ size: CGFloat, weight: GhostWeight = .regular, relativeTo: Font.TextStyle = .body) -> GhostTextStyle {
        GhostTextStyle(size: size, lineHeight: (size * 1.4).rounded(), weight: weight, relativeTo: relativeTo)
    }

    /// Geist Mono at `size`.
    static func mono(_ size: CGFloat, weight: GhostWeight = .regular, relativeTo: Font.TextStyle = .body) -> GhostTextStyle {
        GhostTextStyle(size: size, lineHeight: (size * 1.45).rounded(), weight: weight, mono: true, relativeTo: relativeTo)
    }
}
