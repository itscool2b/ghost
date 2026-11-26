import GhostHighlight
import GhostMarks
import GhostTranscript
import SwiftUI

/// A change, unified: each hunk with three lines around it, the unchanged stretches between folded
/// ("12 unchanged lines", a tap opens them), line numbers beside, added and removed lines tinted in
/// the status colours, the words that changed within a changed line marked, the code in the
/// theme's colours, and each hunk scrolling sideways on its own.
struct DiffView: View {
    @Environment(\.style) private var style
    let old: String
    let new: String
    let path: String
    var context = 3

    @State private var model: DiffModel?
    @State private var colours: DiffColours?
    @State private var opened: Set<Int> = []
    /// One line's height, the same in the numbers and the code, growing with the text size.
    @ScaledMetric(relativeTo: .body) private var rowHeight: CGFloat = 19

    var body: some View {
        Group {
            if let model {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(Array(model.parts.enumerated()), id: \.offset) { index, part in
                        switch part {
                        case .fold(let from, let to):
                            if opened.contains(index) {
                                hunk(model, from: from, to: to)
                            } else {
                                fold(index, count: to - from)
                            }
                        case .change(let from, let to, _, _, _):
                            hunk(model, from: from, to: to)
                        }
                    }
                    if model.truncated {
                        Text("Too long to compare line by line: \(model.added) added, \(model.removed) removed.")
                            .ghostFont(.small)
                            .foregroundStyle(style.fg3)
                    }
                }
            } else {
                PixelLoader(size: 14, tempo: .thinking).frame(maxWidth: .infinity, minHeight: 60)
            }
        }
        .task(id: Key(old: old, new: new, look: style)) {
            let (old, new, context) = (old, new, context)
            let built = await Task.detached(priority: .userInitiated) {
                DiffModel(old: old, new: new, context: context)
            }.value
            model = built
            colours = await DiffColours.make(old: old, new: new, path: path, style: style)
        }
    }

    private struct Key: Equatable {
        let old: String
        let new: String
        /// The theme too: its code colours change with it.
        let look: GhostStyle
    }

    private func fold(_ index: Int, count: Int) -> some View {
        Button {
            withAnimation(.snappy) { _ = opened.insert(index) }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "arrow.up.and.down")
                Text(count == 1 ? "1 unchanged line" : "\(count) unchanged lines")
                Spacer(minLength: 0)
            }
            .ghostFont(.small)
            .foregroundStyle(style.fg3)
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(style.card, in: .rect(cornerRadius: 10))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityHint("Shows them")
    }

    /// Rows `from..<to`: the numbers stay put, the code scrolls sideways.
    private func hunk(_ model: DiffModel, from: Int, to: Int) -> some View {
        let rows = Array(model.rows[from..<to].enumerated()).map { (index: from + $0.offset, row: $0.element) }
        return HStack(alignment: .top, spacing: 0) {
            VStack(alignment: .trailing, spacing: 0) {
                ForEach(rows, id: \.index) { entry in
                    HStack(spacing: 4) {
                        Text((entry.row.newLine ?? entry.row.oldLine).map(String.init) ?? "")
                            .foregroundStyle(style.fg4)
                            .monospacedDigit()
                        Text(sign(entry.row.op))
                            .foregroundStyle(signColour(entry.row.op))
                            .frame(width: 10)
                    }
                    .frame(height: rowHeight)
                    .padding(.leading, 8)
                    .padding(.trailing, 4)
                    .frame(maxWidth: .infinity, alignment: .trailing)
                    .background { Rectangle().fill(tint(entry.row.op)) }
                }
            }
            .fixedSize(horizontal: true, vertical: false)
            .ghostFont(.mono(12))
            .accessibilityHidden(true)
            ScrollView(.horizontal, showsIndicators: false) {
                VStack(alignment: .leading, spacing: 0) {
                    ForEach(rows, id: \.index) { entry in
                        Text(text(entry.index, entry.row, model))
                            .foregroundStyle(style.codeText)
                            .lineLimit(1)
                            .fixedSize()
                            .frame(height: rowHeight)
                            .padding(.horizontal, 8)
                            .accessibilityLabel(spoken(entry.row))
                    }
                }
                .fixedSize(horizontal: true, vertical: false)
                .ghostFont(.mono(12))
            }
            .scrollBounceBehavior(.basedOnSize, axes: .horizontal)
            // The tints reach the hunk's right edge however short its lines are.
            .background(alignment: .topLeading) {
                VStack(spacing: 0) {
                    ForEach(rows, id: \.index) { entry in
                        Rectangle().fill(tint(entry.row.op)).frame(height: rowHeight)
                    }
                }
            }
        }
        .padding(.vertical, 6)
        .background(style.codeBackground, in: .rect(cornerRadius: 10))
        .clipShape(.rect(cornerRadius: 10))
    }

    /// A line's code: coloured when the colours are ready, with what changed in it marked.
    private func text(_ index: Int, _ row: NumberedLine, _ model: DiffModel) -> AttributedString {
        var line: AttributedString
        switch row.op {
        case .remove:
            line = row.oldLine.flatMap { colours?.old.line($0 - 1) } ?? AttributedString(row.text)
        case .add, .same:
            line = row.newLine.flatMap { colours?.new.line($0 - 1) } ?? AttributedString(row.text)
        }
        if line.characters.isEmpty { return AttributedString(" ") }
        if let marked = model.marks[index] {
            let count = line.characters.count
            let lower = min(marked.lowerBound, count)
            let upper = min(marked.upperBound, count)
            if upper > lower {
                let start = line.characters.index(line.startIndex, offsetBy: lower)
                let end = line.characters.index(line.startIndex, offsetBy: upper)
                line[start..<end].backgroundColor = row.op == .add ? style.green.opacity(0.3) : style.red.opacity(0.3)
            }
        }
        return line
    }

    private func spoken(_ row: NumberedLine) -> String {
        switch row.op {
        case .add: "Added: \(row.text)"
        case .remove: "Removed: \(row.text)"
        case .same: row.text
        }
    }

    private func sign(_ op: DiffLine.Op) -> String {
        switch op {
        case .add: "+"
        case .remove: "−"
        case .same: " "
        }
    }

    private func signColour(_ op: DiffLine.Op) -> Color {
        switch op {
        case .add: style.green
        case .remove: style.red
        case .same: style.fg4
        }
    }

    private func tint(_ op: DiffLine.Op) -> Color {
        switch op {
        case .add: style.green.opacity(0.13)
        case .remove: style.red.opacity(0.13)
        case .same: .clear
        }
    }
}

/// A diff worked out once, off the main thread: its rows, its hunks and folds, and the part of
/// each changed line that changed.
nonisolated struct DiffModel: Sendable {
    let rows: [NumberedLine]
    let parts: [DiffPart]
    let truncated: Bool
    let added: Int
    let removed: Int
    /// For a removed line and the added line that replaced it, the characters between what they
    /// share at the start and at the end.
    let marks: [Int: Range<Int>]

    init(old: String, new: String, context: Int) {
        let diff = diffLines(old, new)
        rows = numberLines(diff.lines)
        parts = changeParts(rows, context: context)
        truncated = diff.truncated
        added = diff.added
        removed = diff.removed
        marks = Self.marks(rows)
    }

    /// Pairs each run of removed lines with the added lines straight after it, line by line, and
    /// marks what changed between each pair. A line rewritten from end to end gets no mark.
    private static func marks(_ rows: [NumberedLine]) -> [Int: Range<Int>] {
        var marks: [Int: Range<Int>] = [:]
        var index = 0
        while index < rows.count {
            guard rows[index].op == .remove else {
                index += 1
                continue
            }
            var removedEnd = index
            while removedEnd < rows.count, rows[removedEnd].op == .remove { removedEnd += 1 }
            var addedEnd = removedEnd
            while addedEnd < rows.count, rows[addedEnd].op == .add { addedEnd += 1 }
            let pairs = min(removedEnd - index, addedEnd - removedEnd)
            for offset in 0..<pairs {
                let before = Array(rows[index + offset].text)
                let after = Array(rows[removedEnd + offset].text)
                var prefix = 0
                while prefix < before.count, prefix < after.count, before[prefix] == after[prefix] { prefix += 1 }
                var suffix = 0
                while suffix < before.count - prefix, suffix < after.count - prefix,
                    before[before.count - 1 - suffix] == after[after.count - 1 - suffix]
                {
                    suffix += 1
                }
                guard prefix + suffix > 0 else { continue }
                if before.count - suffix > prefix { marks[index + offset] = prefix..<(before.count - suffix) }
                if after.count - suffix > prefix { marks[removedEnd + offset] = prefix..<(after.count - suffix) }
            }
            index = max(addedEnd, index + 1)
        }
        return marks
    }
}

/// Both sides of a diff in the theme's code colours, line by line.
struct DiffColours {
    struct Side {
        let lines: [AttributedString]
        func line(_ index: Int) -> AttributedString? { lines.indices.contains(index) ? lines[index] : nil }
    }
    let old: Side
    let new: Side

    static func make(old: String, new: String, path: String, style: GhostStyle) async -> DiffColours? {
        let ext = (path as NSString).pathExtension.lowercased()
        guard !ext.isEmpty else { return nil }
        async let before = side(old, ext, style)
        async let after = side(new, ext, style)
        return DiffColours(old: await before, new: await after)
    }

    private static func side(_ text: String, _ language: String, _ style: GhostStyle) async -> Side {
        guard let coloured = await Highlighting.color(text, language: language, style: style) else {
            return Side(lines: [])
        }
        let lines = split(coloured)
        // Lines that do not match the diff's own split are not used: plain text shows instead.
        return Side(lines: lines.count == splitLines(text).count ? lines : [])
    }

    /// Lines as `splitLines` cuts them: at each line feed, without an empty last line.
    private static func split(_ text: AttributedString) -> [AttributedString] {
        var lines: [AttributedString] = []
        let characters = text.characters
        var start = characters.startIndex
        var index = characters.startIndex
        while index < characters.endIndex {
            let next = characters.index(after: index)
            let character = characters[index]
            if character == "\n" || character == "\r\n" {
                lines.append(AttributedString(text[start..<index]))
                start = next
            }
            index = next
        }
        if start < characters.endIndex { lines.append(AttributedString(text[start..<characters.endIndex])) }
        return lines
    }
}
