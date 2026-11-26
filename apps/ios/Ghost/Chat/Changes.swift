import GhostMarks
import GhostWire
import SwiftUI

/// What a chat changed: its branch and pull request, then the files, each opening to its diff.
struct ChangesView: View {
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    let computer: ComputerStore
    let chatId: String
    @State var scope: ChangeScope = .turn
    @State private var facts: ChatFacts?
    @State private var changes: ChangedFiles?
    @State private var failure: String?

    var body: some View {
        NavigationStack {
            List {
                if let facts, facts.branch != nil || facts.pullRequest != nil {
                    Section { BranchCard(facts: facts) { url in openURL(url) } }
                        .listRowBackground(style.card)
                }
                if facts?.worktree == true {
                    Picker("Changes", selection: $scope) {
                        Text("Last turn").tag(ChangeScope.turn)
                        Text("Whole branch").tag(ChangeScope.branch)
                    }
                    .pickerStyle(.segmented)
                    .listRowBackground(Color.clear)
                }
                if let changes {
                    if changes.files.isEmpty {
                        Text(changes.unavailable ?? "Nothing changed.")
                            .ghostFont(.meta)
                            .foregroundStyle(style.fg3)
                            .listRowBackground(Color.clear)
                    }
                    Section {
                        ForEach(changes.files) { file in
                            NavigationLink(value: file) {
                                FileRow(file: file)
                            }
                            .listRowBackground(style.card)
                        }
                    } header: {
                        if !changes.files.isEmpty {
                            Text("\(changes.files.count) \(changes.files.count == 1 ? "file" : "files")")
                                .ghostFont(.meta)
                                .foregroundStyle(style.fg3)
                        }
                    }
                } else if failure == nil {
                    HStack(spacing: 10) {
                        PixelLoader(size: 14, tempo: .thinking)
                        Text("Asking the computer.").ghostFont(.meta).foregroundStyle(style.fg3)
                    }
                    .listRowBackground(Color.clear)
                }
                if let failure {
                    Text(failure).ghostFont(.meta).foregroundStyle(style.red).listRowBackground(Color.clear)
                }
            }
            .scrollContentBackground(.hidden)
            .background(style.pane.ignoresSafeArea())
            .navigationTitle("Changes")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: ChangedFiles.File.self) { file in
                FileDiffView(computer: computer, chatId: chatId, file: file, scope: scope)
            }
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
            .task { facts = try? await computer.connection.send(.facts(chatId), as: ChatFacts.self) }
            .task(id: scope) {
                changes = nil
                do {
                    changes = try await computer.connection.send(.changes(chatId, scope: scope), as: ChangedFiles.self)
                    failure = nil
                } catch {
                    failure = (error as? LocalizedError)?.errorDescription
                }
            }
        }
    }
}

/// A changed file: added, changed or deleted, its folder dim and its name strong.
private struct FileRow: View {
    @Environment(\.style) private var style
    let file: ChangedFiles.File

    var body: some View {
        HStack(spacing: 10) {
            Text(letter)
                .ghostFont(.mono(12, weight: .semibold))
                .foregroundStyle(color)
                .frame(width: 18)
            let name = (file.path as NSString).lastPathComponent
            let folder = (file.path as NSString).deletingLastPathComponent
            (Text(folder.isEmpty ? "" : folder + "/").foregroundStyle(style.fg3) + Text(name).foregroundStyle(style.fg1))
                .ghostFont(.mono(13))
                .lineLimit(2)
                .truncationMode(.head)
        }
        .accessibilityLabel("\(words), \(file.path)")
    }

    private var letter: String {
        switch file.kind {
        case "added": "A"
        case "deleted": "D"
        default: "M"
        }
    }

    private var words: String {
        switch file.kind {
        case "added": "Added"
        case "deleted": "Deleted"
        default: "Changed"
        }
    }

    private var color: Color {
        switch file.kind {
        case "added": style.green
        case "deleted": style.red
        default: style.amber
        }
    }
}

/// One file's change, line by line.
struct FileDiffView: View {
    @Environment(\.style) private var style
    let computer: ComputerStore
    let chatId: String
    let file: ChangedFiles.File
    let scope: ChangeScope
    @State private var diff: FileDiff?
    @State private var failure: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let diff {
                    if diff.binary {
                        Text("A binary file: there are no lines to show.").ghostFont(.meta).foregroundStyle(style.fg3)
                    } else {
                        DiffView(old: diff.before ?? "", new: diff.after ?? "", path: file.path)
                        if diff.truncated {
                            Text("This file is long: the computer sent its first part.")
                                .ghostFont(.small)
                                .foregroundStyle(style.fg3)
                        }
                    }
                } else if let failure {
                    Text(failure).ghostFont(.meta).foregroundStyle(style.red)
                } else {
                    PixelLoader(size: 14, tempo: .thinking)
                }
            }
            .padding(16)
        }
        .background(style.pane.ignoresSafeArea())
        .navigationTitle((file.path as NSString).lastPathComponent)
        .navigationBarTitleDisplayMode(.inline)
        .task {
            do {
                diff = try await computer.connection.send(.diff(chatId, path: file.path, scope: scope), as: FileDiff.self)
            } catch {
                failure = (error as? LocalizedError)?.errorDescription
            }
        }
    }
}

/// The chat's branch and pull request: checks, reviews from people the repository trusts, and the
/// agent's usage.
struct BranchCard: View {
    @Environment(\.style) private var style
    let facts: ChatFacts
    let open: (URL) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let branch = facts.branch {
                HStack(spacing: 8) {
                    Image(systemName: "arrow.triangle.branch").foregroundStyle(style.fg3)
                    Text(branch).ghostFont(.mono(13)).foregroundStyle(style.fg1).lineLimit(1).truncationMode(.middle)
                }
                if let uncommitted = facts.uncommitted, uncommitted > 0 {
                    Text("\(uncommitted) \(uncommitted == 1 ? "file" : "files") not committed")
                        .ghostFont(.meta)
                        .foregroundStyle(style.fg3)
                }
            }
            if let pull = facts.pullRequest {
                Divider().overlay(style.line)
                HStack(spacing: 8) {
                    Circle().fill(checkColor(pull)).frame(width: 8, height: 8)
                    Text("Pull request #\(pull.number)").ghostFont(.rowTitle).foregroundStyle(style.fg1)
                    Spacer()
                    Text(pull.state.capitalized).ghostFont(.meta).foregroundStyle(style.fg3)
                }
                Text(checkWords(pull)).ghostFont(.meta).foregroundStyle(pull.checks == "failed" ? style.red : style.fg2)
                ForEach(Array(pull.notes.suffix(3).enumerated()), id: \.offset) { _, note in
                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(note.author) \(note.kind)").ghostFont(.small).foregroundStyle(style.fg3)
                        if note.trusted, !note.body.isEmpty {
                            Text(note.body).ghostFont(.chat).foregroundStyle(style.fg1).lineLimit(4)
                        } else if !note.trusted {
                            Text("Someone outside the repository commented. Read it on GitHub.")
                                .ghostFont(.small)
                                .foregroundStyle(style.fg3)
                        }
                    }
                }
                if let url = URL(string: pull.url) {
                    Button("Open the pull request", systemImage: "arrow.up.right") { open(url) }
                        .ghostFont(.ui(14, weight: .medium))
                        .buttonStyle(.glass)
                }
            }
            ForEach(facts.usage, id: \.window) { usage in
                VStack(alignment: .leading, spacing: 4) {
                    Text("\(usage.window): \(Int(usage.used.rounded()))% used").ghostFont(.small).foregroundStyle(style.fg3)
                    ProgressView(value: min(100, max(0, usage.used)), total: 100)
                        .tint(usage.used > 85 ? style.amber : style.fg2)
                }
            }
        }
    }

    private func checkColor(_ pull: ChatFacts.PullRequest) -> Color {
        switch pull.checks {
        case "passed": style.green
        case "failed": style.red
        case "pending": style.amber
        default: style.fg4
        }
    }

    private func checkWords(_ pull: ChatFacts.PullRequest) -> String {
        switch pull.checks {
        case "passed": "Checks passed"
        case "failed": "Checks failed: " + pull.failed.joined(separator: ", ")
        case "pending": "Checks running"
        default: "No checks"
        }
    }
}

/// Under a chat's title: its branch and pull request in one line, opening the changes.
struct BranchStrip: View {
    @Environment(\.style) private var style
    let facts: ChatFacts
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            HStack(spacing: 10) {
                Image(systemName: "arrow.triangle.branch").ghostFont(.small).foregroundStyle(style.fg3)
                VStack(alignment: .leading, spacing: 2) {
                    if let branch = facts.branch {
                        Text(branch)
                            .ghostFont(.mono(13))
                            .foregroundStyle(style.fg1)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    if !details.isEmpty {
                        Text(detailLine)
                            .ghostFont(.small)
                            .foregroundStyle(style.fg3)
                    }
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right").ghostFont(.small).foregroundStyle(style.fg3)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .frame(minHeight: 44)
            .background(style.card, in: .rect(cornerRadius: 12))
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Changes: \([facts.branch, details].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: ", "))")
        .accessibilityAddTraits(.isButton)
    }

    /// The details, after a dot in the checks' colour when there are checks.
    private var detailLine: AttributedString {
        var line = AttributedString()
        if let pull = facts.pullRequest, let tone = tone(pull.checks) {
            var dot = AttributedString("● ")
            dot.foregroundColor = tone
            line.append(dot)
        }
        line.append(AttributedString(details))
        return line
    }

    private func tone(_ checks: String) -> Color? {
        switch checks {
        case "passed": style.green
        case "failed": style.red
        case "pending": style.amber
        default: nil
        }
    }

    private var details: String {
        var parts: [String] = []
        if let uncommitted = facts.uncommitted, uncommitted > 0 {
            parts.append(uncommitted == 1 ? "1 file not committed" : "\(uncommitted) files not committed")
        }
        if let pull = facts.pullRequest {
            var words = "PR #\(pull.number)"
            switch pull.checks {
            case "passed": words += ", checks passed"
            case "failed": words += ", checks failed"
            case "pending": words += ", checks running"
            default: break
            }
            if pull.state == "merged" { words += ", merged" } else if pull.state == "closed" { words += ", closed" }
            parts.append(words)
        }
        return parts.joined(separator: " · ")
    }
}
