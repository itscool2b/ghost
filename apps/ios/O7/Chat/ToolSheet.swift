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
