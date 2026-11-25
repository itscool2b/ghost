import O7Marks
import O7Wire
import SwiftUI

/// Starts work: what should happen, where, and with which agent.
struct NewChatSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    var projectId: String?
    let opened: (ChatRoute) -> Void

    @State private var text = ""
    @State private var folderId: String?
    @State private var agentId: String?
    @State private var busy = false
    @State private var failure: String?
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            if let computer = model.selected, let overview = computer.overview {
                form(computer, overview)
            } else {
                ContentUnavailableView(
                    "Waiting for your computer", systemImage: "desktopcomputer",
                    description: Text("New chats start once it is connected."))
            }
        }
    }

    private func form(_ computer: ComputerStore, _ overview: Overview) -> some View {
        let folders = overview.projects.flatMap { project in
            project.folders.map { (project: project, folder: $0) }
        }
        let chosenFolder = folderId ?? folders.first(where: { $0.project.id == projectId })?.folder.id
            ?? folders.first?.folder.id
        let project = folders.first { $0.folder.id == chosenFolder }?.project
        let usual = project?.defaultAgent ?? overview.defaultAgent
        return ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                TextField("What should happen?", text: $text, axis: .vertical)
                    .o7Font(.ui(18, relativeTo: .body))
                    .foregroundStyle(style.fg1)
                    .lineLimit(4...12)
                    .focused($focused)
                    .padding(16)
                    .background(style.card, in: .rect(cornerRadius: 18))
                VStack(alignment: .leading, spacing: 8) {
                    Text("Project").o7Font(.meta).foregroundStyle(style.fg3)
                    Menu {
                        ForEach(folders, id: \.folder.id) { entry in
                            Button(label(entry.project, entry.folder)) { folderId = entry.folder.id }
                        }
                    } label: {
                        HStack {
                            Image(systemName: "folder")
                            Text(folders.first { $0.folder.id == chosenFolder }.map { label($0.project, $0.folder) } ?? "Choose")
                            Spacer()
                            Image(systemName: "chevron.up.chevron.down").font(.caption)
                        }
                        .o7Font(.ui(15))
                        .foregroundStyle(style.fg1)
                        .padding(14)
                        .background(style.card, in: .rect(cornerRadius: 14))
                    }
                }
                VStack(alignment: .leading, spacing: 8) {
                    Text("Agent").o7Font(.meta).foregroundStyle(style.fg3)
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(overview.agents.filter(\.available)) { agent in
                                let chosen = (agentId ?? usual) == agent.id
                                Button {
                                    agentId = agent.id
                                } label: {
                                    HStack(spacing: 8) {
                                        AgentMark(agent: agent.id, size: 22)
                                        Text(agent.name + (agent.id == usual ? " (usual)" : ""))
                                            .o7Font(.ui(14, weight: .medium))
                                    }
                                    .foregroundStyle(chosen ? style.onPrimary : style.fg1)
                                    .padding(.horizontal, 12)
                                    .padding(.vertical, 8)
                                }
                                .buttonStyle(.glassProminent)
                                .tint(chosen ? style.primary : style.control)
                            }
                        }
                    }
                }
                if let failure {
                    Text(failure).o7Font(.meta).foregroundStyle(style.red)
                }
            }
            .padding(20)
        }
        .background(style.pane.ignoresSafeArea())
        .navigationTitle("New chat")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel", systemImage: "xmark") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button {
                    Task { await start(computer, folder: chosenFolder, agent: agentId == usual ? nil : agentId) }
                } label: {
                    if busy { ProgressView() } else { Text("Start") }
                }
                .buttonStyle(.glassProminent)
                .disabled(busy || chosenFolder == nil || text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .onAppear { focused = true }
    }

    private func label(_ project: ProjectSummary, _ folder: ProjectFolder) -> String {
        project.name == folder.displayName ? project.name : "\(project.name) · \(folder.displayName)"
    }

    private func start(_ computer: ComputerStore, folder: String?, agent: String?) async {
        guard let folder else { return }
        busy = true
        defer { busy = false }
        do {
            let chatId = try await computer.create(folderId: folder, agentId: agent, text: text)
            dismiss()
            opened(ChatRoute(hostId: computer.hostId, chatId: chatId))
        } catch {
            failure = (error as? LocalizedError)?.errorDescription ?? "The chat did not start."
        }
    }
}
