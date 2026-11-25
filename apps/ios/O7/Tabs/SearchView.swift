import O7Wire
import SwiftUI

/// Chats by title, project and agent on every paired computer, and by what they say where the
/// computer can search its chats.
struct SearchView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let query: String
    let zoom: Namespace.ID
    let open: (ChatRoute) -> Void
    @State private var found: [String: [SearchHits.Hit]] = [:]

    var body: some View {
        List {
            ForEach(model.computers) { computer in
                let local = matches(computer)
                let remote = (found[computer.hostId] ?? []).filter { hit in
                    !local.contains { $0.chat.id == hit.chatId }
                }
                if !local.isEmpty || !remote.isEmpty {
                    Section {
                        ForEach(local) { chat in
                            Button {
                                open(ChatRoute(hostId: computer.hostId, chatId: chat.chat.id))
                            } label: {
                                ChatRow(chat: chat, project: computer.project(chat.projectId)?.name)
                            }
                            .buttonStyle(.plain)
                            .matchedTransitionSource(id: chat.chat.id, in: zoom)
                        }
                        ForEach(remote, id: \.chatId) { hit in
                            if let chat = computer.summary(hit.chatId) {
                                Button {
                                    open(ChatRoute(hostId: computer.hostId, chatId: hit.chatId))
                                } label: {
                                    ChatRow(chat: chat, project: computer.project(chat.projectId)?.name, line: hit.snippet)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    } header: {
                        if model.computers.count > 1 { GroupTitle(title: computer.hostName) }
                    }
                    .listRowBackground(Color.clear)
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(style.window.ignoresSafeArea())
        .navigationTitle("Search")
        .overlay {
            if !query.isEmpty, model.computers.allSatisfy({ matches($0).isEmpty && (found[$0.hostId] ?? []).isEmpty }) {
                ContentUnavailableView.search(text: query)
            }
        }
        .task(id: query) {
            let text = query.trimmingCharacters(in: .whitespacesAndNewlines)
            guard text.count >= 2 else {
                found = [:]
                return
            }
            try? await Task.sleep(for: .milliseconds(300))
            for computer in model.computers where computer.features.contains("search") {
                if let hits = try? await computer.connection.send(.search(text), as: SearchHits.self) {
                    found[computer.hostId] = hits.hits
                }
            }
        }
    }

    private func matches(_ computer: ComputerStore) -> [WorkspaceChat] {
        let words = query.lowercased().split(separator: " ").map(String.init)
        guard !words.isEmpty else { return Array(computer.chats.prefix(20)) }
        return computer.chats.filter { chat in
            let haystack = [
                chat.chat.title ?? "", chat.chat.agentName, computer.project(chat.projectId)?.name ?? "",
            ].joined(separator: " ").lowercased()
            return words.allSatisfy(haystack.contains)
        }
    }
}
