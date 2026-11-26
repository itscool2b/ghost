import GhostWire
import SwiftUI

/// Chats by title, project and agent on every paired computer, and by what they say where the
/// computer can search its chats; narrowed to what works, what needs the person, or what was
/// archived. Searches that led somewhere are kept for next time.
struct SearchView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Binding var query: String
    let zoom: Namespace.ID
    let open: (ChatRoute) -> Void
    @State private var scope: SearchScope = .all
    @State private var found: [String: [SearchHits.Hit]] = [:]
    @State private var archived: [String: [WorkspaceChat]] = [:]
    @AppStorage("search.recent") private var recentJSON = "[]"

    var body: some View {
        List {
            Picker("Show", selection: $scope) {
                ForEach(SearchScope.allCases) { Text($0.title).tag($0) }
            }
            .pickerStyle(.segmented)
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
            if query.isEmpty, scope == .all, !recent.isEmpty {
                Section {
                    ForEach(recent, id: \.self) { words in
                        Button {
                            query = words
                        } label: {
                            Label(words, systemImage: "clock.arrow.circlepath")
                                .ghostFont(.ui(15))
                                .foregroundStyle(style.fg1)
                                .frame(maxWidth: .infinity, minHeight: 36, alignment: .leading)
                                .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                    }
                    .onDelete { offsets in
                        var kept = recent
                        kept.remove(atOffsets: offsets)
                        save(kept)
                    }
                } header: {
                    GroupTitle(title: "Recent")
                }
                .listRowBackground(Color.clear)
            }
            ForEach(model.computers) { computer in
                let local = matches(computer)
                let remote =
                    scope == .all
                    ? (found[computer.hostId] ?? []).filter { hit in !local.contains { $0.chat.id == hit.chatId } }
                    : []
                if !local.isEmpty || !remote.isEmpty {
                    Section {
                        ForEach(local) { chat in
                            Button {
                                go(ChatRoute(hostId: computer.hostId, chatId: chat.chat.id))
                            } label: {
                                ChatRow(chat: chat, project: computer.project(chat.projectId)?.name)
                            }
                            .buttonStyle(.plain)
                            .matchedTransitionSource(id: chat.chat.id, in: zoom)
                        }
                        ForEach(remote, id: \.chatId) { hit in
                            if let chat = computer.summary(hit.chatId) {
                                Button {
                                    go(ChatRoute(hostId: computer.hostId, chatId: hit.chatId))
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
            if nothing {
                if query.isEmpty {
                    ContentUnavailableView(
                        scope.empty, systemImage: scope.symbol,
                        description: Text(scope == .archived ? "Archived chats show here." : ""))
                } else {
                    ContentUnavailableView.search(text: query)
                }
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
        .task(id: scope) {
            guard scope == .archived else { return }
            for computer in model.computers where computer.offersControl {
                if let page = try? await computer.connection.send(.archived(limit: 100), as: ArchivedChats.self) {
                    archived[computer.hostId] = page.chats
                }
            }
        }
    }

    private var nothing: Bool {
        model.computers.allSatisfy { computer in
            matches(computer).isEmpty && (scope != .all || (found[computer.hostId] ?? []).isEmpty)
        }
    }

    private func matches(_ computer: ComputerStore) -> [WorkspaceChat] {
        let pool: [WorkspaceChat] =
            switch scope {
            case .all: computer.chats
            case .working: computer.working
            case .needsYou: computer.needsYou
            case .archived: archived[computer.hostId] ?? []
            }
        let words = query.lowercased().split(separator: " ").map(String.init)
        guard !words.isEmpty else { return scope == .all ? Array(pool.prefix(20)) : pool }
        return pool.filter { chat in
            let haystack = [
                chat.chat.title ?? "", chat.chat.agentName, computer.project(chat.projectId)?.name ?? "",
            ].joined(separator: " ").lowercased()
            return words.allSatisfy(haystack.contains)
        }
    }

    /// Opens a result, keeping the search that found it.
    private func go(_ route: ChatRoute) {
        let words = query.trimmingCharacters(in: .whitespacesAndNewlines)
        if words.count >= 2 { save([words] + recent.filter { $0.caseInsensitiveCompare(words) != .orderedSame }) }
        open(route)
    }

    private var recent: [String] {
        (try? JSONDecoder().decode([String].self, from: Data(recentJSON.utf8))) ?? []
    }

    private func save(_ searches: [String]) {
        let kept = Array(searches.prefix(8))
        recentJSON = (try? String(decoding: JSONEncoder().encode(kept), as: UTF8.self)) ?? "[]"
    }
}

enum SearchScope: String, CaseIterable, Identifiable {
    case all, working, needsYou, archived
    var id: String { rawValue }

    var title: String {
        switch self {
        case .all: "All"
        case .working: "Working"
        case .needsYou: "Needs you"
        case .archived: "Archived"
        }
    }

    var empty: String {
        switch self {
        case .all: "No chats yet"
        case .working: "Nothing is working"
        case .needsYou: "Nothing needs you"
        case .archived: "No archived chats"
        }
    }

    var symbol: String {
        switch self {
        case .all: "bubble.left.and.bubble.right"
        case .working: "hammer"
        case .needsYou: "hand.raised"
        case .archived: "archivebox"
        }
    }
}
