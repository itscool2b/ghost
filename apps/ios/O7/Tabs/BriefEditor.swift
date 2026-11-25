import O7Client
import O7Marks
import O7Wire
import SwiftUI

/// A project's brief: the short part of its memory every chat there receives, at most 4 KB.
struct BriefEditor: View {
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    let computer: ComputerStore
    let project: ProjectSummary
    @State private var text = ""
    /// The brief as the computer last gave it: Save waits for a change.
    @State private var saved = ""
    @State private var revision: String?
    @State private var loaded = false
    @State private var preview = false
    @State private var saving = false
    @State private var failure: String?
    @State private var conflict = false
    @FocusState private var focused: Bool

    private var bytes: Int { text.utf8.count }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                Picker("View", selection: $preview) {
                    Text("Edit").tag(false)
                    Text("Preview").tag(true)
                }
                .pickerStyle(.segmented)
                .padding(16)
                if !loaded {
                    PixelLoader(size: 16, tempo: .thinking).frame(maxHeight: .infinity)
                } else if preview {
                    ScrollView { MarkdownView(text).padding(16) }
                } else {
                    TextEditor(text: $text)
                        .o7Font(.mono(14))
                        .foregroundStyle(style.fg1)
                        .scrollContentBackground(.hidden)
                        .focused($focused)
                        .padding(.horizontal, 12)
                }
                VStack(alignment: .leading, spacing: 6) {
                    if let failure {
                        Text(failure).o7Font(.meta).foregroundStyle(style.red)
                    }
                    if conflict {
                        // Someone changed it on the computer meanwhile: nothing of either is lost
                        // until the person chooses.
                        HStack(spacing: 8) {
                            Button("Save mine over it") { Task { await overwrite() } }
                                .buttonStyle(.glassProminent)
                                .tint(style.primary)
                                .foregroundStyle(style.onPrimary)
                            Button("Take the computer's") { Task { await load() } }
                                .buttonStyle(.glass)
                        }
                        .o7Font(.ui(14, weight: .medium))
                    }
                    Text("\(ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)) of 4 KB. Every chat in \(project.name) receives this.")
                        .o7Font(.small)
                        .foregroundStyle(bytes > 4096 ? style.red : style.fg3)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
            }
            .background(style.pane.ignoresSafeArea())
            .navigationTitle("Brief")
            .navigationSubtitle(project.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", systemImage: "xmark") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button {
                        Task { await save() }
                    } label: {
                        if saving { ProgressView() } else { Text("Save") }
                    }
                    .buttonStyle(.glassProminent)
                    .disabled(!loaded || saving || bytes > 4096 || text == saved)
                }
            }
            .task { await load() }
        }
    }

    private func load() async {
        do {
            let brief = try await computer.connection.send(.memory(project.id), as: ProjectBrief.self)
            text = brief.markdown
            saved = brief.markdown
            revision = brief.revision
            loaded = true
            conflict = false
            failure = nil
        } catch {
            failure = (error as? LocalizedError)?.errorDescription
        }
    }

    /// Saves this version over the one the computer has now.
    private func overwrite() async {
        do {
            let current = try await computer.connection.send(.memory(project.id), as: ProjectBrief.self)
            revision = current.revision
            conflict = false
            await save()
        } catch {
            failure = (error as? LocalizedError)?.errorDescription
        }
    }

    private func save() async {
        saving = true
        defer { saving = false }
        do {
            let result = try await computer.connection.send(
                .setMemory(project.id, markdown: text, base: revision), as: SavedBrief.self)
            revision = result.revision
            saved = text
            dismiss()
        } catch let failure as RemoteFailure {
            self.failure = failure.message
            conflict = failure.code == "conflict"
        } catch {
            failure = "The brief was not saved."
        }
    }
}
