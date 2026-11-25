import O7Highlight
import SwiftUI

/// Code coloured the way the desktop colours it: the desktop's own highlight.js, run off the main
/// thread, mapped onto the theme's syntax colours.
enum Highlighting {
    static func color(_ code: String, language: String?, style: O7Style) async -> AttributedString? {
        let runs = await Highlighter.shared.tokens(code: code, language: language)
        guard !runs.isEmpty else { return nil }
        return Highlighter.attributedString(code, runs: runs, palette: style.palette)
    }
}
