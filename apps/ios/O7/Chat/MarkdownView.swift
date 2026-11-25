import O7Markdown
import O7Marks
import SwiftUI

/// An agent's answer, drawn block by block in o7's type: prose in Geist, code in Geist Mono on its
/// own raised block with a Copy button, tables that scroll sideways.
struct MarkdownView: View {
    @Environment(\.style) private var style
    let blocks: [MarkdownBlock]

    init(_ text: String) {
        blocks = Markdown.blocks(text)
    }

    init(blocks: [MarkdownBlock]) {
        self.blocks = blocks
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                BlockView(block: block)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct BlockView: View {
    @Environment(\.style) private var style
    /// Read so a change of text size redraws the inline runs, whose fonts are set per run.
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let block: MarkdownBlock

    var body: some View {
        let _ = dynamicTypeSize
        switch block {
        case .paragraph(let text):
            Text(styled(text))
                .o7Font(.prose)
                .foregroundStyle(style.fg1)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        case .heading(let level, let text):
            Text(styled(text))
                .o7Font(level <= 1 ? .title : level == 2 ? .heading : .ui(15, weight: .semibold))
                .foregroundStyle(style.fg1)
                .padding(.top, 4)
        case .code(let language, let text):
            CodeBlock(code: text, language: language)
        case .quote(let inner):
            HStack(alignment: .top, spacing: 10) {
                RoundedRectangle(cornerRadius: 1.5).fill(style.lineStrong).frame(width: 3)
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(inner.enumerated()), id: \.offset) { _, block in BlockView(block: block) }
                }
                .foregroundStyle(style.fg2)
            }
        case .list(let ordered, let start, let items):
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(ordered ? "\(start + index)." : "•")
                            .o7Font(.prose)
                            .monospacedDigit()
                            .foregroundStyle(style.fg3)
                            .frame(minWidth: 16, alignment: .trailing)
                        VStack(alignment: .leading, spacing: 6) {
                            ForEach(Array(item.enumerated()), id: \.offset) { _, block in BlockView(block: block) }
                        }
                    }
                }
            }
        case .table(let header, let rows, let alignments):
            TableBlock(header: header, rows: rows, alignments: alignments)
        case .rule:
            Rectangle().fill(style.line).frame(height: 1).padding(.vertical, 4)
        }
    }

    /// Inline code in Geist Mono on a faint ground, strong words in Geist's semibold, emphasis in
    /// italic, links in the accent. The fonts are named per run: o7's fonts carry their weight, so
    /// the system cannot embolden them on its own.
    private func styled(_ text: AttributedString, size: CGFloat = 15) -> AttributedString {
        var text = text
        for run in text.runs {
            if let intent = run.inlinePresentationIntent, intent.contains(.code) {
                text[run.range].font = O7Font.code
                text[run.range].foregroundColor = style.codeText
                text[run.range].backgroundColor = style.codeBackground
            } else if let intent = run.inlinePresentationIntent {
                let strong = intent.contains(.stronglyEmphasized)
                let emphasized = intent.contains(.emphasized)
                if strong || emphasized {
                    let font = O7Font.ui(size, weight: strong ? .semibold : .regular, relativeTo: .subheadline)
                    text[run.range].font = emphasized ? font.italic() : font
                }
                if intent.contains(.strikethrough) {
                    text[run.range].strikethroughStyle = .single
                }
            }
            if run.link != nil {
                text[run.range].foregroundColor = style.accent
                text[run.range].underlineStyle = .single
            }
        }
        return text
    }
}

/// Code on its own raised block, coloured as the desktop colours it.
struct CodeBlock: View {
    @Environment(\.style) private var style
    let code: String
    let language: String?
    @State private var colored: AttributedString?
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language ?? "code")
                    .o7Font(.ui(12))
                    .foregroundStyle(style.fg3)
                Spacer()
                Button {
                    UIPasteboard.general.string = code
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                        .o7Font(.ui(12, weight: .medium))
                        .foregroundStyle(style.fg2)
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(.rect)
                }
                .buttonStyle(.plain)
                .contentTransition(.symbolEffect(.replace))
            }
            .padding(.horizontal, 12)
            ScrollView(.horizontal, showsIndicators: false) {
                Text(colored ?? AttributedString(code))
                    .o7Font(.code)
                    .foregroundStyle(style.codeText)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: true)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 12)
            }
        }
        .background(style.codeBackground, in: .rect(cornerRadius: 10))
        .sensoryFeedback(.success, trigger: copied) { _, new in new }
        .task(id: code) { colored = await Highlighting.color(code, language: language, style: style) }
    }
}

/// A table whose columns can be wider than the phone: it scrolls sideways.
private struct TableBlock: View {
    @Environment(\.style) private var style
    let header: [AttributedString]
    let rows: [[AttributedString]]
    let alignments: [MarkdownBlock.Alignment]

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
                GridRow {
                    ForEach(Array(header.enumerated()), id: \.offset) { index, cell in
                        Text(cell).o7Font(.ui(14, weight: .semibold)).foregroundStyle(style.fg1)
                            .gridColumnAlignment(alignment(index))
                    }
                }
                Divider().overlay(style.line)
                ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                    GridRow {
                        ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                            Text(cell).o7Font(.ui(14)).foregroundStyle(style.fg1)
                        }
                    }
                }
            }
            .padding(12)
        }
        .background(style.card, in: .rect(cornerRadius: 10))
    }

    private func alignment(_ index: Int) -> HorizontalAlignment {
        switch index < alignments.count ? alignments[index] : .leading {
        case .leading: .leading
        case .center: .center
        case .trailing: .trailing
        }
    }
}
