import Foundation
import Observation
import O7Client
import O7Transcript
import O7Wire

/// One open chat: its history and live events folded by the desktop's own reducer, what the person
/// sent that the computer has not shown yet, and the chat's actions (`client.ts` open, frame,
/// loadEarlier).
@Observable
final class ChatStore {
    let chatId: String
    unowned let computer: ComputerStore

    enum Load: Equatable {
        case opening
        case open
        case failed(String)
    }

    private(set) var load: Load = .opening
    private(set) var snapshot: ChatSnapshot?
    private(set) var transcript = Transcript.empty(cut: nil) {
        didSet { segments = cache.render(transcript, live: live) }
    }
    /// The transcript as the chat draws it; unchanged turns keep their rendering.
    private(set) var segments: [RenderedSegment] = []
    private var cache = RenderCache()
    private(set) var hasMore = false
    private(set) var loadingEarlier = false
    /// Messages sent from here and not yet in the transcript: "Sending", then maybe "Queued".
    private(set) var sending: [Sending] = []
    /// A failure of something the person did here, said plainly.
    var problem: String?
    /// When the turn that runs now started, for the working line's clock.
    private(set) var turnStartedAt: Date?

    struct Sending: Identifiable, Equatable {
        let id = UUID()
        let text: String
        /// How many user messages with this text were already in the transcript when it was sent.
        let before: Int
        var queued = false
    }

    /// Every entry, oldest first: an earlier page rebuilds the transcript from them.
    private var entries: [Entry] = []
    private var cut: String?
    private var expected: Int64 = 0
    /// Live frames that came while the history was on its way.
    private var early: [EventBatch] = []
    private var opening = false
    private var gone = false
    private var pendingFold: [Entry] = []
    private var folding: Task<Void, Never>?

    init(chatId: String, computer: ComputerStore) {
        self.chatId = chatId
        self.computer = computer
    }

    var summary: ChatSummary? { computer.summary(chatId)?.chat ?? snapshot?.chat }

    /// The chat's agent has a process: steps still running are live, not stopped.
    var live: Bool { hasProcess(snapshot?.chat.status ?? summary?.status ?? .stopped) }

    /// A turn runs now.
    var running: Bool {
        let status = summary?.status ?? snapshot?.chat.status ?? .stopped
        return status == .running || status == .starting || status == .cancelling
    }

    /// Background tasks the strip over the message box lists: running ones, and those that ended in
    /// the newest turn (`stripTasks`).
    var stripTasks: [TaskItem] {
        O7Transcript.stripTasks(
            transcript.tasks, turnKey: newestTurnKey(transcript.segments), live: live, liveTasks: liveTasks)
    }

    /// The background tasks the computer says run now.
    var liveTasks: Set<String> {
        Set((snapshot?.tasks ?? []).filter { $0.status == .running }.map(\.taskId))
    }

    /// Something in the chat waits on the person.
    var waiting: Bool { summary?.needsAttention ?? snapshot?.chat.needsAttention ?? false }

    // MARK: Opening

    func open() async {
        guard !gone else { return }
        opening = true
        defer { opening = false }
        do {
            let attach = try await computer.connection.send(.open(chatId), as: ChatAttach.self)
            guard !gone else { return }
            attached(attach)
        } catch let failure as RemoteFailure where failure.code == "superseded" {
            return
        } catch {
            // The session may be coming up: it opens again once it is.
            if case .ready = computer.phase {
                load = .failed((error as? LocalizedError)?.errorDescription ?? "The chat did not open.")
            }
        }
    }

    func reopen() async {
        early.removeAll()
        await open()
    }

    /// A new session started: the chat that was open opens again.
    func sessionRestarted() async {
        await reopen()
    }

    func dropped() {
        gone = true
        folding?.cancel()
    }

    private func attached(_ attach: ChatAttach) {
        snapshot = attach.snapshot
        // The open turn's start, from when its first event was stored.
        if let open = attach.history.items.last(where: {
            if case .turnStarted = $0.event { true } else { false }
        }), !attach.history.items.contains(where: { item in
            if case .turnEnded(let turn, _, _, _, _) = item.event { turn == open.turnId } else { false }
        }) {
            turnStartedAt = Date(timeIntervalSince1970: Double(open.at) / 1000)
        } else {
            turnStartedAt = nil
        }
        cut = attach.history.cutTurnId
        hasMore = attach.history.hasMore
        entries = compactToolUpdates(attach.history.items.map(Entry.init))
        expected = attach.nextSeq
        transcript = Transcript.empty(cut: cut).applying(entries).withTasks(attach.snapshot.tasks ?? [])
        load = .open
        let waiting = early
        early.removeAll()
        for batch in waiting { received(batch) }
    }

    // MARK: Live events

    func received(_ batch: EventBatch) {
        guard batch.chatId == chatId, !gone else { return }
        if opening || load == .opening {
            early.append(batch)
            return
        }
        // Events were missed: open the chat again rather than show a hole.
        if batch.seqStart > expected {
            Task { await reopen() }
            return
        }
        var fresh: [Entry] = []
        for (offset, event) in batch.events.enumerated() {
            let seq = batch.seqStart + Int64(offset)
            guard seq >= expected else { continue }
            if case .sessionUpdated(let info) = event {
                snapshot?.session = info
                snapshot?.chat.status = info.status
            }
            if case .turnStarted = event { turnStartedAt = .now }
            fresh.append(.event(seq: seq, turnId: nil, event: event))
        }
        guard !fresh.isEmpty else { return }
        expected = batch.seqStart + Int64(batch.events.count)
        entries.append(contentsOf: fresh)
        pendingFold.append(contentsOf: fresh)
        scheduleFold()
    }

    /// Folds what arrived at most once a frame, so a fast agent does not redraw on every word.
    private func scheduleFold() {
        guard folding == nil else { return }
        folding = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(16))
            guard let self, !Task.isCancelled else { return }
            let batch = pendingFold
            pendingFold.removeAll()
            folding = nil
            transcript = transcript.applying(batch)
            settleSending()
        }
    }

    func statusChanged(_ update: ChatStatusUpdate) {
        guard update.chatId == chatId else { return }
        snapshot?.chat.status = update.status
        snapshot?.chat.needsAttention = update.needsAttention
        if let title = update.title { snapshot?.chat.title = title }
    }

    // MARK: Earlier history

    func loadEarlier() async {
        guard hasMore, !loadingEarlier, let first = entries.first else { return }
        loadingEarlier = true
        defer { loadingEarlier = false }
        do {
            let page = try await computer.connection.send(.history(chatId, before: first.seq), as: HistoryPage.self)
            let older = page.items.map(Entry.init)
            entries = compactToolUpdates(older + entries)
            cut = page.cutTurnId
            hasMore = page.hasMore
            transcript = Transcript.empty(cut: cut).applying(entries)
        } catch {
            problem = (error as? LocalizedError)?.errorDescription
        }
    }

    // MARK: Acting

    /// Sends a message. It shows at once; a refused one hands its text back.
    func send(_ text: String) async -> String? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        let entry = Sending(text: trimmed, before: userCount(trimmed))
        sending.append(entry)
        do {
            let ack = try await computer.connection.send(.send(chatId, text: trimmed), as: PromptAck.self, timeout: .act)
            if ack.queued, let at = sending.firstIndex(where: { $0.id == entry.id }) { sending[at].queued = true }
            problem = nil
            return nil
        } catch {
            sending.removeAll { $0.id == entry.id }
            problem = (error as? LocalizedError)?.errorDescription ?? "Not sent."
            return text
        }
    }

    func stop() async {
        do {
            try await computer.stopTurn(chatId)
        } catch {
            problem = (error as? LocalizedError)?.errorDescription
        }
    }

    func permission(_ request: String, option: String?) async {
        do {
            try await computer.allow(chatId, request: request, option: option)
        } catch {
            problem = (error as? LocalizedError)?.errorDescription
        }
    }

    func answer(_ request: String, _ answer: UiAnswer?) async {
        do {
            try await computer.connection.perform(.answer(chatId, request: request, answer: answer), timeout: .act)
        } catch {
            problem = (error as? LocalizedError)?.errorDescription
        }
    }

    /// A sent message leaves "Sending" once the transcript shows it.
    private func settleSending() {
        sending.removeAll { userCount($0.text) > $0.before }
    }

    private func userCount(_ text: String) -> Int {
        transcript.userMessages.filter { $0 == text }.count
    }
}

extension Entry {
    init(_ item: HistoryItem) {
        self = .event(seq: item.seq, turnId: item.turnId, event: item.event)
    }
}

extension Transcript {
    /// The person's messages, in order, as the transcript shows them.
    var userMessages: [String] {
        items.orderedValues.compactMap { item in
            if case .user(let user) = item { return user.text }
            return nil
        }
    }
}
