import O7Marks
import SwiftUI

extension O7TextStyle {
    /// Geist at `size`, with o7's usual line height for it, scaling as `relativeTo` scales.
    static func ui(_ size: CGFloat, weight: O7Weight = .regular, relativeTo: Font.TextStyle = .body) -> O7TextStyle {
        O7TextStyle(size: size, lineHeight: (size * 1.4).rounded(), weight: weight, relativeTo: relativeTo)
    }

    /// Geist Mono at `size`.
    static func mono(_ size: CGFloat, weight: O7Weight = .regular, relativeTo: Font.TextStyle = .body) -> O7TextStyle {
        O7TextStyle(size: size, lineHeight: (size * 1.45).rounded(), weight: weight, mono: true, relativeTo: relativeTo)
    }
}
