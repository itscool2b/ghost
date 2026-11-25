import O7Marks
import O7Transcript
import O7Wire
import SwiftUI

/// One chat, live: its turns on the chat's layer, the glass message box on the keyboard, and the
/// chat's controls in the glass bar above.
struct ChatScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let route: ChatRoute
    let zoom: Namespace.ID

    var body: some View {
        Group {
            if let computer = model.computers.first(where: { $0.hostId == route.hostId }) {
                ChatContent(computer: computer, chatId: route.chatId)
            } else {
                ContentUnavailableView("This computer is gone", systemImage: "desktopcomputer.trianglebadge.exclamationmark")
            }
        }
        .navigationTransition(.zoom(sourceID: route.chatId, in: zoom))
        .toolbarVisibility(.hidden, for: .tabBar)
    }
}

private struct ChatContent: View {
    @Environment(\.style) private var style
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let computer: ComputerStore
    let chatId: String
    @State private var chat: ChatStore?
    @State private var draft = ""
    @State private var pickers = false
    @State private var photos: [PickedPhoto] = []
    @State private var picking = false
    @State private var shooting = false
    @State private var atBottom = true
    @State private var position = ScrollPosition(edge: .bottom)
    @State private var workingSince = Date()
    @State private var sentCount = 0
    @State private var facts: ChatFacts?
    @State private var showingChanges: ChangeScope?
    @State private var showingScreen = false
    @State private var renaming = false
    @State private var newTitle = ""
    @State private var archiving = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        let summary = computer.summary(chatId)?.chat ?? chat?.snapshot?.chat
        ZStack(alignment: .bottom) {
            style.pane.ignoresSafeArea()
            if let chat {
                transcript(chat)
            }
        }
        .safeAreaBar(edge: .top) {
            // What the chat stands on stays in view while its turns scroll under it.
            if let facts, facts.worktree || facts.pullRequest != nil {
                BranchStrip(facts: facts) { showingChanges = facts.worktree ? .branch : .turn }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 6)
            }
        }
        .safeAreaBar(edge: .bottom) {
            if let chat {
                VStack(spacing: 8) {
                    let tasks = chat.stripTasks
                    if !tasks.isEmpty {
                        TaskStrip(tasks: tasks, live: chat.live, liveTasks: chat.liveTasks)
                    }
                    Composer(
                        text: $draft, photos: $photos, picking: $picking, shooting: $shooting,
                        running: chat.running, waiting: chat.waiting,
                        enabled: computer.phase == .ready, offersPhotos: computer.offersPhotos,
                        chip: computer.offersPickers ? chip(chat) : nil,
                        onSend: { send(chat) },
                        onStop: { Task { await chat.stop() } },
                        onChip: { pickers = true })
                        .padding(.horizontal, 12)
                        .padding(.bottom, 6)
                }
            }
        }
        .photoSources($photos, picking: $picking, shooting: $shooting)
        .navigationTitle(summary?.title ?? "Untitled chat")
        .navigationSubtitle(subtitle(summary))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    if computer.offersPickers {
                        Button("Model and agent", systemImage: "slider.horizontal.3") { pickers = true }
                    }
                    if computer.offersReview {
                        Button("Changes", systemImage: "plus.forwardslash.minus") { showingChanges = .turn }
                    }
                    if computer.offersScreens {
                        Button("Screen", systemImage: "rectangle.on.rectangle") { showingScreen = true }
                    }
                    if computer.offersControl {
                        Button("Rename", systemImage: "pencil") {
                            newTitle = summary?.title ?? ""
                            renaming = true
                        }
                        if let status = summary?.status, status == .stopped || status == .failed {
                            Button("Start the agent", systemImage: "play") {
                                Task { try? await computer.connection.perform(.startAgent(chatId), timeout: .slow) }
                            }
                        }
                        Button("Archive", systemImage: "archivebox") { archiving = true }
                    }
                    if let chat, chat.running {
                        Button("Stop the turn", systemImage: "stop.fill", role: .destructive) {
                            Task { await chat.stop() }
                        }
                    }
                } label: {
                    Image(systemName: "ellipsis")
                }
                .accessibilityLabel("More")
            }
        }
        .sheet(isPresented: $pickers) {
            if let summary {
                PickersSheet(computer: computer, chat: summary, running: chat?.running ?? false) { next in
                    chat = computer.open(next)
                }
                .presentationDetents([.medium, .large])
                .o7Look(style)
            }
        }
        .sheet(item: $showingChanges) { scope in
            ChangesView(computer: computer, chatId: chatId, scope: scope)
                .presentationDetents([.large])
                .o7Look(style)
        }
        .sheet(isPresented: $showingScreen) {
            ScreenView(computer: computer, chatId: chatId)
                .presentationDetents([.large])
                .o7Look(style)
        }
        .alert("Rename the chat", isPresented: $renaming) {
            TextField("Title", text: $newTitle)
            Button("Rename") {
                let title = newTitle.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !title.isEmpty else { return }
                Task {
                    try? await computer.connection.perform(.rename(chatId, title: title))
                    await computer.refresh()
                }
            }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog("Archive this chat?", isPresented: $archiving, titleVisibility: .visible) {
            Button("Archive") {
                Task {
                    try? await computer.connection.perform(.archive(chatId, archived: true))
                    await computer.refresh()
                    dismiss()
                }
            }
        } message: {
            Text("It leaves the lists here and on your computer. Archived chats are in the project's Archived list.")
        }
        .task(id: chat?.segments.last?.end != nil) {
            // What the chat stands on: read when it opens, and again when a turn ends.
            if computer.offersReview { facts = try? await computer.connection.send(.facts(chatId), as: ChatFacts.self) }
        }
        .onAppear {
            if chat == nil { chat = computer.open(chatId) }
            if let unsent = computer.unsent, unsent.chatId == chatId {
                draft = unsent.text
                chat?.problem = "Not sent: \(unsent.reason)"
                computer.unsent = nil
            }
        }
        .onDisappear {
            if let chat { computer.close(chat) }
        }
        .onChange(of: chat?.running) { _, running in
            if running == true { workingSince = .now }
        }
        .sensoryFeedback(.success, trigger: sentCount)
        .sensoryFeedback(.warning, trigger: chat?.waiting) { old, new in old == false && new == true }
    }

    private func transcript(_ chat: ChatStore) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 22) {
                if chat.hasMore {
                    Button {
                        Task { await chat.loadEarlier() }
                    } label: {
                        HStack(spacing: 8) {
                            if chat.loadingEarlier { PixelLoader(size: 12, tempo: .thinking) }
                            Text(chat.loadingEarlier ? "Loading" : "Earlier messages")
                        }
                        .o7Font(.meta)
                        .foregroundStyle(style.fg3)
                        .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.plain)
                    .onAppear { Task { await chat.loadEarlier() } }
                }
                switch chat.load {
                case .opening:
                    HStack(spacing: 10) {
                        PixelLoader(size: 14, tempo: .thinking)
                        Text(computer.phase == .ready ? "Opening the chat." : "Waiting for \(computer.hostName).")
                            .o7Font(.meta)
                            .foregroundStyle(style.fg3)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.top, 40)
                case .failed(let reason):
                    Text(reason).o7Font(.meta).foregroundStyle(style.red)
                case .open:
                    if chat.segments.isEmpty, chat.sending.isEmpty {
                        Text("Nothing here yet. Send a message.")
                            .o7Font(.meta)
                            .foregroundStyle(style.fg3)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 40)
                    }
                    ForEach(chat.segments) { segment in
                        TurnView(segment: segment, chat: chat)
                            .id(segment.key)
                    }
                }
                ForEach(chat.sending) { sending in
                    UserBubble(
                        text: sending.text, photos: sending.photos,
                        pending: sending.queued ? "Queued until this turn ends" : "Sending")
                }
                if chat.running, !chat.waiting {
                    WorkingLine(since: chat.turnStartedAt ?? workingSince)
                }
                if let problem = chat.problem {
                    Text(problem).o7Font(.meta).foregroundStyle(style.red)
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 12)
            .padding(.bottom, 20)
        }
        .scrollPosition($position)
        .defaultScrollAnchor(.bottom)
        .scrollDismissesKeyboard(.interactively)
        .onScrollGeometryChange(for: Bool.self) { geometry in
            geometry.contentSize.height <= geometry.containerSize.height
                || geometry.visibleRect.maxY >= geometry.contentSize.height - 80
        } action: { _, near in
            atBottom = near
        }
        .onChange(of: chat.segments.last?.revision) {
            // Pinned to the newest words while they arrive, unless the person scrolled up to read.
            if atBottom { withAnimation(reduceMotion ? nil : .smooth) { position.scrollTo(edge: .bottom) } }
        }
        .overlay(alignment: .bottomTrailing) {
            if !atBottom {
                Button {
                    withAnimation(.smooth) { position.scrollTo(edge: .bottom) }
                } label: {
                    Label("Latest", systemImage: "arrow.down")
                        .o7Font(.ui(13, weight: .medium))
                        .padding(.horizontal, 4)
                }
                .buttonStyle(.glass)
                .padding(16)
                .transition(.scale.combined(with: .opacity))
            }
        }
        .animation(.snappy, value: atBottom)
    }

    private func send(_ chat: ChatStore) {
        let text = draft
        let sent = photos
        draft = ""
        photos = []
        sentCount += 1
        Task {
            guard let (words, pictures) = await chat.send(text, photos: sent) else { return }
            if draft.isEmpty { draft = words }
            if photos.isEmpty { photos = pictures }
        }
    }

    private func chip(_ chat: ChatStore) -> String? {
        guard let session = chat.snapshot?.session else { return nil }
        let parts = [session.model, session.effort].compactMap { $0 }.filter { !$0.isEmpty }
        return parts.isEmpty ? (chat.summary?.agentName ?? "Model and agent") : parts.joined(separator: " · ")
    }

    private func subtitle(_ summary: ChatSummary?) -> String {
        guard let summary else { return "" }
        return "\(summary.agentName) · \(StatusWords(summary).words)"
    }
}

extension ChangeScope: @retroactive Identifiable {
    public var id: String { rawValue }
}
