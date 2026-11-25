import O7Marks
import O7Transcript
import O7Wire
import SwiftUI

/// One tool call in full: what it was given, what it showed, where.
struct ToolSheet: View {
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    let tool: ToolItem

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text(tool.title.replacingOccurrences(of: "`", with: ""))
                        .o7Font(.mono(14, weight: .medium))
                        .foregroundStyle(style.fg1)
                        .textSelection(.enabled)
                    if !tool.locations.isEmpty {
                        VStack(alignment: .leading, spacing: 4) {
                            ForEach(Array(tool.locations.enumerated()), id: \.offset) { _, location in
                                Text(location.line.map { "\(location.path):\($0)" } ?? location.path)
                                    .o7Font(.mono(12))
                                    .foregroundStyle(style.fg2)
                            }
                        }
                    }
                    let fields = toolInput(tool)
                    if !fields.isEmpty {
                        section("Input") {
                            ForEach(Array(fields.enumerated()), id: \.offset) { _, field in
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(field.key).o7Font(.small).foregroundStyle(style.fg3)
                                    Text(field.value)
                                        .o7Font(field.block ? .mono(12) : .chat)
                                        .foregroundStyle(style.fg1)
                                        .textSelection(.enabled)
                                }
                            }
                        }
                    }
                    ForEach(Array(tool.content.enumerated()), id: \.offset) { _, content in
                        switch content {
                        case .text(let text, _):
                            section("Output") { MarkdownView(text) }
                        case .terminal(let output, let exitCode, _):
                            section(exitCode.map { $0 == 0 ? "Output" : "Output, exit code \($0)" } ?? "Output") {
                                ScrollView(.horizontal, showsIndicators: false) {
                                    Text(output)
                                        .o7Font(.terminal)
                                        .foregroundStyle(style.codeText)
                                        .textSelection(.enabled)
                                        .fixedSize(horizontal: true, vertical: true)
                                        .padding(12)
                                }
                                .background(style.codeBackground, in: .rect(cornerRadius: 10))
                            }
                        case .diff(let path, let old, let new, _):
                            section((path as NSString).lastPathComponent) {
                                DiffView(old: old ?? "", new: new, path: path)
                            }
                        case .unknown:
                            EmptyView()
                        }
                    }
                    if let truncated = tool.truncated {
                        Text("Cut short here: the whole output is \(ByteCountFormatter.string(fromByteCount: truncated.totalBytes, countStyle: .file)) on the computer.")
                            .o7Font(.small)
                            .foregroundStyle(style.fg3)
                    }
                }
                .padding(20)
            }
            .background(style.raised.ignoresSafeArea())
            .navigationTitle(toolPhrase(tool.title, kind: tool.toolKind, display: ToolDisplayStatus(tool.status)).verb ?? "Step")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
        }
        .presentationBackground(style.raised)
    }

    private func section(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).o7Font(.ui(13, weight: .medium)).foregroundStyle(style.fg2)
            content()
        }
    }
}

/// A file's change, line by line: removed in red, added in green, with line numbers.
struct DiffView: View {
    @Environment(\.style) private var style
    let old: String
    let new: String
    let path: String

    var body: some View {
        let diff = diffLines(old, new)
        let lines = numberLines(diff.lines)
        ScrollView(.horizontal, showsIndicators: false) {
            LazyVStack(alignment: .leading, spacing: 0) {
                ForEach(lines, id: \.key) { line in
                    HStack(spacing: 8) {
                        Text((line.newLine ?? line.oldLine).map(String.init) ?? "")
                            .frame(width: 34, alignment: .trailing)
                            .foregroundStyle(style.fg4)
                        Text(sign(line.op))
                            .foregroundStyle(color(line.op))
                        Text(line.text.isEmpty ? " " : line.text)
                            .foregroundStyle(style.codeText)
                            .fixedSize()
                    }
                    .o7Font(.mono(12))
                    .padding(.vertical, 1)
                    .padding(.horizontal, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(background(line.op))
                }
                if diff.truncated {
                    Text("Too long to compare line by line: \(diff.added) added, \(diff.removed) removed.")
                        .o7Font(.small)
                        .foregroundStyle(style.fg3)
                        .padding(8)
                }
            }
            .padding(.vertical, 8)
        }
        .background(style.codeBackground, in: .rect(cornerRadius: 10))
    }

    private func sign(_ op: DiffLine.Op) -> String {
        switch op {
        case .add: "+"
        case .remove: "−"
        case .same: " "
        }
    }

    private func color(_ op: DiffLine.Op) -> Color {
        switch op {
        case .add: style.green
        case .remove: style.red
        case .same: style.fg4
        }
    }

    private func background(_ op: DiffLine.Op) -> Color {
        switch op {
        case .add: style.green.opacity(0.12)
        case .remove: style.red.opacity(0.12)
        case .same: .clear
        }
    }
}
