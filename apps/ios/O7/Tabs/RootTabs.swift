import O7Marks
import O7Wire
import SwiftUI

/// Where a chat is: which computer, which chat.
struct ChatRoute: Hashable {
    let hostId: String
    let chatId: String
}

enum AppTab: Hashable { case inbox, projects, tonight, search }

/// The floating glass tab bar, and above it the strip that says what is working or waiting.
struct RootTabs: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @State private var tab: AppTab = .inbox
    @State private var inboxPath: [ChatRoute] = []
    @State private var projectsPath: [ProjectsRoute] = []
    @State private var searchPath: [ChatRoute] = []
    @State private var tonightPath: [ChatRoute] = []
    @State private var settings = false
    @State private var newWork = false
    @State private var query = ""
    @Namespace private var zoom

    var body: some View {
        TabView(selection: $tab) {
            Tab("Inbox", systemImage: "tray.full", value: AppTab.inbox) {
                NavigationStack(path: $inboxPath) {
                    InboxView(zoom: zoom, settings: $settings, newWork: $newWork) { inboxPath.append($0) }
                        .navigationDestination(for: ChatRoute.self) { route in
                            ChatScreen(route: route, zoom: zoom)
                        }
                }
            }
            .badge(model.needsYouCount)
            Tab("Projects", systemImage: "folder", value: AppTab.projects) {
                NavigationStack(path: $projectsPath) {
                    ProjectsView(zoom: zoom, settings: $settings, newWork: $newWork) { projectsPath.append($0) }
                        .navigationDestination(for: ProjectsRoute.self) { route in
                            switch route {
                            case .project(let hostId, let projectId):
                                ProjectView(hostId: hostId, projectId: projectId, zoom: zoom) {
                                    projectsPath.append(.chat($0))
                                }
                            case .chat(let chat):
                                ChatScreen(route: chat, zoom: zoom)
                            }
                        }
                }
            }
            if model.selected?.offersTasks == true {
                Tab("Tonight", systemImage: "moon.stars", value: AppTab.tonight) {
                    NavigationStack(path: $tonightPath) {
                        TonightView(settings: $settings) { tonightPath.append($0) }
                            .navigationDestination(for: ChatRoute.self) { route in
                                ChatScreen(route: route, zoom: zoom)
                            }
                    }
                }
            }
            Tab(value: AppTab.search, role: .search) {
                NavigationStack(path: $searchPath) {
                    SearchView(query: $query, zoom: zoom) { searchPath.append($0) }
                        .navigationDestination(for: ChatRoute.self) { route in
                            ChatScreen(route: route, zoom: zoom)
                        }
                }
            }
        }
        .searchable(text: $query, prompt: "Chats, projects, agents")
        .tabViewSearchActivation(.searchTabSelection)
        .tabBarMinimizeBehavior(.onScrollDown)
        .tabViewBottomAccessory(isEnabled: showsAccessory) {
            WorkingAccessory { route in
                tab = .inbox
                inboxPath = [route]
            }
        }
        .sheet(isPresented: $settings) {
            SettingsView().o7Look(style)
        }
        .sheet(isPresented: $newWork) {
            NewChatSheet { route in
                tab = .inbox
                inboxPath = [route]
            }
            .o7Look(style)
            .presentationDetents([.large])
        }
    }

    /// The strip shows while anything works or waits, and never over an open chat.
    private var showsAccessory: Bool {
        let inChat =
            switch tab {
            case .inbox: !inboxPath.isEmpty
            case .projects: projectsPath.contains { if case .chat = $0 { true } else { false } }
            case .tonight: !tonightPath.isEmpty
            case .search: !searchPath.isEmpty
            }
        return !inChat && (model.needsYouCount > 0 || model.workingCount > 0)
    }
}

/// Settings and New, on the top bar of each tab's first screen.
struct TabToolbar: ToolbarContent {
    @Binding var settings: Bool
    @Binding var newWork: Bool

    var body: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Button {
                settings = true
            } label: {
                Image(systemName: "gearshape")
            }
            .accessibilityLabel("Settings")
        }
        ToolbarItem(placement: .topBarTrailing) {
            Button {
                newWork = true
            } label: {
                Image(systemName: "plus")
            }
            .buttonStyle(.glassProminent)
            .accessibilityLabel("New work")
        }
    }
}

/// The glass strip above the tabs: what needs the person first, else what works.
struct WorkingAccessory: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.tabViewBottomAccessoryPlacement) private var placement
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let open: (ChatRoute) -> Void

    var body: some View {
        let (computer, chat, waiting) = lead
        Button {
            if let computer, let chat { open(ChatRoute(hostId: computer.hostId, chatId: chat.chat.id)) }
        } label: {
            HStack(spacing: 10) {
                if waiting {
                    Circle().fill(style.accent).frame(width: 9, height: 9)
                } else {
                    PixelLoader(size: 16, tempo: .tool)
                }
                VStack(alignment: .leading, spacing: 0) {
                    Text(summary)
                        .o7Font(.ui(13, weight: .medium))
                        .foregroundStyle(.primary)
                        .lineLimit(1)
                        .minimumScaleFactor(0.75)
                    // The strip keeps the system's height: its second line goes when text grows.
                    if placement != .inline, dynamicTypeSize <= .large, let chat {
                        Text(inline(detail(computer, chat, waiting), code: .primary))
                            .o7Font(.ui(12))
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.right")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.tertiary)
            }
            .padding(.horizontal, 14)
            .contentShape(.rect)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(summary)
    }

    private var lead: (ComputerStore?, WorkspaceChat?, Bool) {
        for computer in model.computers {
            if let chat = computer.needsYou.first { return (computer, chat, true) }
        }
        for computer in model.computers {
            if let chat = computer.working.first { return (computer, chat, false) }
        }
        return (nil, nil, false)
    }

    private var summary: String {
        let waiting = model.needsYouCount
        let working = model.workingCount
        var parts: [String] = []
        if waiting > 0 { parts.append("\(waiting) need\(waiting == 1 ? "s" : "") you") }
        if working > 0 { parts.append("\(working) working") }
        return parts.joined(separator: " · ")
    }

    private func detail(_ computer: ComputerStore?, _ chat: WorkspaceChat, _ waiting: Bool) -> String {
        let title = chat.chat.title ?? "Untitled chat"
        if waiting, let ask = computer?.asking(chat.chat.id)?.request { return "\(title): \(ask.title)" }
        if let line = computer?.workingLine(chat.chat.id)?.line { return "\(chat.chat.agentName): \(line)" }
        return title
    }
}
