public import Foundation

/// A block of an agent's answer, as the chat draws it.
public indirect enum MarkdownBlock: Hashable, Sendable {
    case paragraph(AttributedString)
    case heading(level: Int, AttributedString)
    case code(language: String?, text: String)
    case quote([MarkdownBlock])
    /// `items` each hold their own blocks; `start` is an ordered list's first number.
    case list(ordered: Bool, start: Int, items: [[MarkdownBlock]])
    case table(header: [AttributedString], rows: [[AttributedString]], alignments: [Alignment])
    case rule

    public enum Alignment: Hashable, Sendable { case leading, center, trailing }
}

/// Markdown as blocks, from Foundation's own parser (CommonMark with GitHub's tables and
/// strikethrough). Inline emphasis, strong, code and strikethrough stay as the parser marks them
/// in the text; links stay only when they are `http` or `https`, so a page cannot be made to open
/// anything else from an agent's words.
public enum Markdown {
    public static func blocks(_ text: String) -> [MarkdownBlock] {
        let options = AttributedString.MarkdownParsingOptions(
            allowsExtendedAttributes: false, interpretedSyntax: .full,
            failurePolicy: .returnPartiallyParsedIfPossible)
        guard let parsed = try? AttributedString(markdown: text, options: options) else {
            return [.paragraph(AttributedString(text))]
        }
        return Tree(parsed).blocks()
    }

    /// Drops every link that is not `http` or `https`, keeping its words.
    static func safeLinks(_ text: AttributedString) -> AttributedString {
        var text = text
        for run in text.runs {
            guard let link = run.link else { continue }
            let scheme = link.scheme?.lowercased()
            if scheme != "http" && scheme != "https" { text[run.range].link = nil }
        }
        return text
    }
}

/// The parser marks each run with the path of blocks it sits in, innermost first. Runs that share
/// a path are one leaf block; the paths give the nesting.
private struct Tree {
    final class Node {
        let kind: PresentationIntent.Kind?
        let identity: Int
        var children: [Node] = []
        var text = AttributedString()
        init(kind: PresentationIntent.Kind?, identity: Int) {
            self.kind = kind
            self.identity = identity
        }
    }

    let root = Node(kind: nil, identity: -1)

    init(_ parsed: AttributedString) {
        var index: [Int: Node] = [:]
        for run in parsed.runs {
            let path = (run.presentationIntent?.components ?? []).reversed()
            var parent = root
            for component in path {
                if let found = index[component.identity] {
                    parent = found
                } else {
                    let node = Node(kind: component.kind, identity: component.identity)
                    index[component.identity] = node
                    parent.children.append(node)
                    parent = node
                }
            }
            parent.text.append(parsed[run.range])
        }
    }

    func blocks() -> [MarkdownBlock] { root.children.flatMap(Self.block) }

    static func block(_ node: Node) -> [MarkdownBlock] {
        guard let kind = node.kind else { return node.children.flatMap(block) }
        switch kind {
        case .paragraph:
            return [.paragraph(Markdown.safeLinks(trimmed(node.text)))]
        case .header(let level):
            return [.heading(level: level, Markdown.safeLinks(trimmed(node.text)))]
        case .codeBlock(let language):
            var code = String(node.text.characters)
            if code.hasSuffix("\n") { code.removeLast() }
            return [.code(language: language?.isEmpty == false ? language : nil, text: code)]
        case .thematicBreak:
            return [.rule]
        case .blockQuote:
            return [.quote(node.children.flatMap(block))]
        case .orderedList, .unorderedList:
            let ordered: Bool = if case .orderedList = kind { true } else { false }
            var start = 1
            if ordered, let first = node.children.first, case .listItem(let ordinal) = first.kind {
                start = ordinal
            }
            let items = node.children.map { item in
                item.children.isEmpty ? [MarkdownBlock.paragraph(item.text)] : item.children.flatMap(block)
            }
            return [.list(ordered: ordered, start: start, items: items)]
        case .listItem:
            return node.children.flatMap(block)
        case .table(let columns):
            var header: [AttributedString] = []
            var rows: [[AttributedString]] = []
            for row in node.children {
                let cells = row.children.map { Markdown.safeLinks(trimmed($0.text)) }
                if case .tableHeaderRow = row.kind { header = cells } else { rows.append(cells) }
            }
            let alignments: [MarkdownBlock.Alignment] = columns.map { column in
                switch column.alignment {
                case .center: .center
                case .right: .trailing
                default: .leading
                }
            }
            return [.table(header: header, rows: rows, alignments: alignments)]
        default:
            return node.children.isEmpty ? [.paragraph(trimmed(node.text))] : node.children.flatMap(block)
        }
    }

    static func trimmed(_ text: AttributedString) -> AttributedString {
        var text = text
        while let last = text.characters.last, last == "\n" {
            text.removeSubrange(text.index(beforeCharacter: text.endIndex)..<text.endIndex)
        }
        return text
    }
}
