import GhostMarks
import GhostWire
import SwiftUI

enum ProjectsRoute: Hashable {
    case project(hostId: String, projectId: String)
    case chat(ChatRoute)
}

/// Every project on the computer in view, busiest first.
struct ProjectsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let zoom: Namespace.ID
    @Binding var settings: Bool
    @Binding var newWork: Bool
    let open: (ProjectsRoute) -> Void

    var body: some View {
        List {
            if let computer = model.selected, let overview = computer.overview {
                ForEach(sorted(overview, computer)) { project in
                    Button {
                        open(.project(hostId: computer.hostId, projectId: project.id))
                    } label: {
                        ProjectRow(project: project, chats: computer.chats.filter { $0.projectId == project.id })
                            .contentShape(.rect)
                    }
                    .buttonStyle(.plain)
                    .listRowBackground(Color.clear)
                    .listRowSeparatorTint(style.line)
                }
                if overview.projects.isEmpty {
                    Text("No projects yet. Add one in Ghost on your computer.")
                        .ghostFont(.meta)
                        .foregroundStyle(style.fg3)
                        .listRowBackground(Color.clear)
                }
            } else {
                ForEach(0..<4, id: \.self) { _ in SkeletonRow() }.listRowBackground(Color.clear)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(style.window.ignoresSafeArea())
        .navigationTitle("Projects")
        .toolbar { TabToolbar(settings: $settings, newWork: $newWork) }
        .toolbarTitleMenu { ComputerMenu() }
        .refreshable { await model.selected?.refresh() }
    }

    private func sorted(_ overview: Overview, _ computer: ComputerStore) -> [ProjectSummary] {
        func latest(_ project: ProjectSummary) -> Int64 {
            computer.chats.filter { $0.projectId == project.id }.map(\.chat.updatedAt).max() ?? project.updatedAt
        }
        return overview.projects.sorted { latest($0) > latest($1) }
    }
}

struct ProjectRow: View {
    @Environment(\.style) private var style
    let project: ProjectSummary
    let chats: [WorkspaceChat]

    var body: some View {
        let waiting = chats.filter(\.chat.needsAttention).count
        let working = chats.filter { $0.chat.status.isWorking && !$0.chat.needsAttention }.count
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline) {
                Text(project.name).ghostFont(.rowTitle).foregroundStyle(style.fg1)
                Spacer()
                if let latest = chats.map(\.chat.updatedAt).max() {
                    Ago(millis: latest).ghostFont(.time).foregroundStyle(style.fg3)
                }
            }
            if let path = project.folders.first?.path {
                Text(path)
                    .ghostFont(.ui(12))
                    .foregroundStyle(style.fg3)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            HStack(spacing: 6) {
                ForEach(chats.filter { $0.chat.status.isWorking || $0.chat.needsAttention }.prefix(4)) { chat in
                    AgentMark(agent: chat.chat.agentId, state: StatusWords(chat.chat).mark, size: 20)
                }
                Text(counts(waiting: waiting, working: working, total: chats.count))
                    .ghostFont(.meta)
                    .foregroundStyle(waiting > 0 ? style.accent : style.fg3)
            }
        }
        .padding(.vertical, 8)
    }

    private func counts(waiting: Int, working: Int, total: Int) -> String {
        var parts: [String] = []
        if waiting > 0 { parts.append("\(waiting) need\(waiting == 1 ? "s" : "") you") }
        if working > 0 { parts.append("\(working) working") }
        if parts.isEmpty { parts.append(total == 1 ? "1 chat" : "\(total) chats") }
        return parts.joined(separator: " · ")
    }
}

/// One project's chats, and its archived ones.
struct ProjectView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let hostId: String
    let projectId: String
    let zoom: Namespace.ID
    let open: (ChatRoute) -> Void
    @State private var newChat = false
    @State private var showArchived = false
    @State private var archived: [WorkspaceChat] = []
    @State private var editingBrief = false

    var body: some View {
        let computer = model.computers.first { $0.hostId == hostId }
        let project = computer?.project(projectId)
        List {
            if computer?.offersControl == true {
                Picker("Chats", selection: $showArchived) {
                    Text("Chats").tag(false)
                    Text("Archived").tag(true)
                }
                .pickerStyle(.segmented)
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            }
            if let computer {
                let chats = showArchived ? archived : computer.chats.filter { $0.projectId == projectId }
                ForEach(chats) { chat in
                    Button {
                        open(ChatRoute(hostId: hostId, chatId: chat.chat.id))
                    } label: {
                        ChatRow(chat: chat)
                    }
                    .buttonStyle(.plain)
                    .matchedTransitionSource(id: chat.chat.id, in: zoom)
                    .listRowBackground(Color.clear)
                    .listRowSeparatorTint(style.line)
                    .swipeActions {
                        if computer.offersControl {
                            Button(showArchived ? "Unarchive" : "Archive", systemImage: "archivebox") {
                                Task {
                                    try? await computer.connection.perform(.archive(chat.chat.id, archived: !showArchived))
                                    await computer.refresh()
                                    await loadArchived(computer)
                                }
                            }
                            .tint(style.fg3)
                        }
                    }
                }
                if chats.isEmpty {
                    Text(showArchived ? "Nothing archived here." : "No chats here yet.")
                        .ghostFont(.meta)
                        .foregroundStyle(style.fg3)
                        .listRowBackground(Color.clear)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(style.window.ignoresSafeArea())
        .navigationTitle(project?.name ?? "Project")
        .navigationSubtitle(project?.folders.first?.path ?? "")
        .toolbar {
            if computer?.offersMemory == true {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Brief", systemImage: "doc.text") { editingBrief = true }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button("New chat", systemImage: "plus") { newChat = true }
                    .buttonStyle(.glassProminent)
            }
        }
        .task(id: showArchived) {
            if showArchived, let computer { await loadArchived(computer) }
        }
        .sheet(isPresented: $newChat) {
            NewChatSheet(projectId: projectId) { route in open(route) }
                .ghostLook(style)
        }
        .sheet(isPresented: $editingBrief) {
            if let computer, let project {
                BriefEditor(computer: computer, project: project)
                    .ghostLook(style)
            }
        }
    }

    private func loadArchived(_ computer: ComputerStore) async {
        guard let page = try? await computer.connection.send(.archived(), as: ArchivedChats.self) else { return }
        archived = page.chats.filter { $0.projectId == projectId }
    }
}
