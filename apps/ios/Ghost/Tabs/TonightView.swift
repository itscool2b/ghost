import GhostMarks
import GhostWire
import SwiftUI

/// Work for tonight: what is queued, what ran, and why anything could not start. The computer
/// starts queued work between 1 and 6 in the morning on its own clock, one at a time.
struct TonightView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Binding var settings: Bool
    let open: (ChatRoute) -> Void
    @State private var snapshot: TasksSnapshot?
    @State private var queueing = false
    @State private var failure: String?

    var body: some View {
        List {
            if let computer = model.selected {
                if !computer.offersTasks {
                    Text("The Ghost on \(computer.hostName) does not take tasks from here yet. Update Ghost there.")
                        .ghostFont(.meta)
                        .foregroundStyle(style.fg3)
                        .listRowBackground(Color.clear)
                } else {
                    Section {
                        Text(nightWords(computer))
                            .ghostFont(.meta)
                            .foregroundStyle(style.fg2)
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                    }
                    Section {
                        ForEach(snapshot?.queued ?? []) { item in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(item.goal.split(separator: "\n").first.map(String.init) ?? item.goal)
                                    .ghostFont(.rowTitle)
                                    .foregroundStyle(style.fg1)
                                    .lineLimit(2)
                                Text(details(item, computer))
                                    .ghostFont(.meta)
                                    .foregroundStyle(item.failed == nil ? style.fg3 : style.red)
                            }
                            .padding(.vertical, 6)
                            .swipeActions {
                                Button("Cancel", systemImage: "xmark", role: .destructive) {
                                    Task { await cancel(item, computer) }
                                }
                                .tint(style.red)
                            }
                        }
                        Button("Queue a task", systemImage: "moon.stars") { queueing = true }
                            .ghostFont(.ui(15, weight: .medium))
                    } header: {
                        GroupTitle(title: "Queued", count: snapshot?.queued.isEmpty == false ? snapshot?.queued.count : nil)
                    }
                    .listRowBackground(Color.clear)
                    if let tasks = snapshot?.tasks, !tasks.isEmpty {
                        Section {
                            ForEach(tasks) { task in
                                Button {
                                    open(ChatRoute(hostId: computer.hostId, chatId: task.chatId))
                                } label: {
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text(task.goal.split(separator: "\n").first.map(String.init) ?? task.goal)
                                            .ghostFont(.rowTitle)
                                            .foregroundStyle(style.fg1)
                                            .lineLimit(2)
                                        Text(taskWords(task, computer))
                                            .ghostFont(.meta)
                                            .foregroundStyle(style.fg3)
                                    }
                                    .padding(.vertical, 6)
                                    .contentShape(.rect)
                                }
                                .buttonStyle(.plain)
                            }
                        } header: {
                            GroupTitle(title: "Handed over", count: tasks.count)
                        }
                        .listRowBackground(Color.clear)
                    }
                    if let failure {
                        Text(failure).ghostFont(.meta).foregroundStyle(style.red).listRowBackground(Color.clear)
                    }
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(style.window.ignoresSafeArea())
        .navigationTitle("Tonight")
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button { settings = true } label: { Image(systemName: "gearshape") }
                    .accessibilityLabel("Settings")
            }
        }
        .toolbarTitleMenu { ComputerMenu() }
        .refreshable { await load() }
        .task(id: model.selected?.hostId) { await load() }
        .sheet(isPresented: $queueing, onDismiss: { Task { await load() } }) {
            NewChatSheet(tonight: true) { route in open(route) }
                .ghostLook(style)
        }
    }

    private func load() async {
        guard let computer = model.selected, computer.offersTasks else { return }
        do {
            snapshot = try await computer.connection.send(.tasks, as: TasksSnapshot.self)
            failure = nil
        } catch {
            failure = (error as? LocalizedError)?.errorDescription
        }
    }

    private func cancel(_ item: TasksSnapshot.Queued, _ computer: ComputerStore) async {
        do {
            try await computer.connection.perform(.taskCancel(item.id))
            await load()
        } catch {
            failure = (error as? LocalizedError)?.errorDescription
        }
    }

    private func nightWords(_ computer: ComputerStore) -> String {
        let night = snapshot?.night
        let from = night?.from ?? 1
        let to = night?.to ?? 6
        let budget = night?.budget ?? 3
        return "Ghost starts these between \(from) and \(to) in the morning on \(computer.hostName)'s clock, one at a time, at most \(budget) a night. The computer must be awake."
    }

    private func details(_ item: TasksSnapshot.Queued, _ computer: ComputerStore) -> String {
        if let failed = item.failed { return "Could not start: \(failed)" }
        var parts: [String] = []
        let project = item.projectId.flatMap { computer.project($0) }
        if let project { parts.append(project.name) }
        // No agent named: the project's usual one starts it.
        if let agent = item.agentId ?? project?.defaultAgent ?? computer.overview?.defaultAgent {
            parts.append(computer.agentName(agent))
        }
        if item.review != false { parts.append("reviewed") }
        if item.by == "bot" { parts.append("from a bot") }
        return parts.joined(separator: " · ")
    }

    private func taskWords(_ task: TasksSnapshot.Task, _ computer: ComputerStore) -> String {
        var parts: [String] = []
        if let branch = task.branch { parts.append(branch) }
        if let reviewer = task.reviewAgent { parts.append("reviewed by \(computer.agentName(reviewer))") }
        if task.by == "bot" { parts.append("from a bot") }
        return parts.joined(separator: " · ")
    }
}
