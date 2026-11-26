public import Foundation
public import GhostTheme
import SwiftUI

extension Highlighter {
    /// The code in a palette's code colours, from its runs: each run takes its role's colour
    /// (`HighlightRole.color(in:)`), and emphasis and strong set the inline intents SwiftUI's
    /// `Text` draws in italic and bold. Text no run covers is plain code. Runs out of order or
    /// past the end are skipped where they overlap.
    public static func attributedString(_ code: String, runs: [HighlightRun], palette: Palette) -> AttributedString {
        let units = code.utf16
        let count = units.count
        var result = AttributedString()
        var cursor = 0
        var position = code.startIndex

        func append(upTo end: Int, role: HighlightRole, emphasized: Bool, strong: Bool) {
            guard end > cursor else { return }
            let next = units.index(position, offsetBy: end - cursor)
            var piece = AttributedString(String(code[position..<next]))
            let color = Color(role.color(in: palette))
            piece.foregroundColor = color
            var intent: InlinePresentationIntent = []
            if emphasized { intent.insert(.emphasized) }
            if strong { intent.insert(.stronglyEmphasized) }
            if !intent.isEmpty { piece.inlinePresentationIntent = intent }
            result.append(piece)
            cursor = end
            position = next
        }

        for run in runs {
            let start = min(max(run.range.lowerBound, cursor), count)
            let end = min(run.range.upperBound, count)
            guard end > start else { continue }
            append(upTo: start, role: .plain, emphasized: false, strong: false)
            append(upTo: end, role: run.role, emphasized: run.isEmphasized, strong: run.isStrong)
        }
        append(upTo: count, role: .plain, emphasized: false, strong: false)
        return result
    }
}
