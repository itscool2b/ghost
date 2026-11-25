import Foundation
import Observation
import O7Client
import O7Crypto
import O7Store
import O7Wire

enum SessionStepName: Hashable { case relay, proved }

/// One paired computer, live: its session, what it says about itself, its projects and chats,
/// and the chat the person has open on it.
@Observable
final class ComputerStore: Identifiable {
    let hostId: String
    private(set) var record: ComputerRecord?
    private(set) var phase: ConnectionPhase = .connecting
    private(set) var overview: Overview?
    private(set) var inbox: InboxSnapshot?
    private(set) var features: [String] = []
    private(set) var look: Look
    private(set) var hostName: String
    /// The last thing that went wrong, said plainly; cleared by the next success.
    var problem: String?
    /// The chat open on this computer, if any. One per computer, as the protocol has it.
    private(set) var chat: ChatStore?

    let connection: HostConnection
    private let backend: Backend
    private var listening: Task<Void, Never>?
    private var refetch: Task<Void, Never>?
    private var inboxFetch: Task<Void, Never>?
    /// What a pairing link became, while pairing.
    private(set) var pairedDevice: DeviceCredential?
    /// How far the session got: the relay answered, the computer proved the key.
    private(set) var steps: Set<SessionStepName> = []
    private let site: String

    var id: String { hostId }

    /// A computer already paired.
    init(record: ComputerRecord, credential: DeviceCredential, backend: Backend) {
        hostId = record.hostId
        self.record = record
        self.backend = backend
        site = record.site
        hostName = record.hostName
        look = record.look ?? .fallback
        features = record.features
        connection = HostConnection(
            credential: .device(credential),
            endpoint: backend.endpoint(site: record.site)
                ?? RelayEndpoint(origin: URL(string: "wss://o7.codes")!),
            transport: backend.transport, client: ThisDevice.client(name: "iPhone"))
    }

    /// A computer being paired with a link.
    init(link: PairingLink, deviceName: String, backend: Backend) {
        hostId = link.hostId
        self.backend = backend
        site = link.site.absoluteString
        hostName = ""
        look = .fallback
        connection = HostConnection(
            credential: .pair(link),
            endpoint: backend.endpoint(site: site) ?? RelayEndpoint(origin: URL(string: "wss://o7.codes")!),
            transport: backend.transport, client: ThisDevice.client(name: deviceName))
    }

    var isPaired: Bool { record != nil }

    func start() {
        guard listening == nil else { return }
        let events = connection.events
        listening = Task { [weak self] in
            for await event in events { self?.handle(event) }
        }
        Task { await connection.start() }
    }

    func stop() {
        listening?.cancel()
        listening = nil
        Task { await connection.stop() }
    }

    func wake() {
        Task { await connection.wake() }
    }

    private func handle(_ event: HostEvent) {
        switch event {
        case .phase(let next):
            phase = next
            if case .ready = next { problem = nil }
        case .step(let step):
            steps.insert(step == .relay ? .relay : .proved)
        case .paired(let device, let given):
            pairedDevice = device
            features = given
        case .ready(let ready):
            features = ready.features
            if !ready.hostName.isEmpty { hostName = ready.hostName }
            if let given = ready.look { look = given }
            save()
            Task {
                await refresh()
                await refreshInbox()
                await chat?.sessionRestarted()
            }
        case .chat(let update):
            merge(update)
            chat?.statusChanged(update)
            scheduleInbox()
        case .frame(let batch):
            chat?.received(batch)
        case .lost(let chatId):
            if chat?.chatId == chatId { Task { await chat?.reopen() } }
        case .reset:
            chat?.dropped()
            chat = nil
            Task { await refresh() }
        }
    }

    /// Saves what this computer said about itself, or the new pairing.
    private func save() {
        if let device = pairedDevice, record == nil {
            let replaced = try? backend.computers.paired(
                device, hostName: hostName.isEmpty ? "Computer" : hostName, site: site, look: look,
                features: features)
            record = backend.computers.all().first { $0.hostId == hostId }
            // Paired with this computer before: the old device goes there too.
            if let replaced = replaced ?? nil { forgetOld(replaced) }
            return
        }
        backend.computers.update(hostId) { record in
            if !hostName.isEmpty { record.hostName = hostName }
            record.look = look
            record.features = features
        }
        record = backend.computers.all().first { $0.hostId == hostId }
    }

    /// Asks the computer to forget a device this phone used before, over a session of its own.
    private func forgetOld(_ device: DeviceCredential) {
        guard let endpoint = backend.endpoint(site: site) else { return }
        let old = HostConnection(
            credential: .device(device), endpoint: endpoint, transport: backend.transport,
            client: ThisDevice.client(name: "iPhone"))
        Task {
            let events = old.events
            await old.start()
            for await event in events {
                if case .phase(.ready) = event {
                    try? await old.perform(.forget)
                    break
                }
                if case .phase(.ended) = event { break }
            }
            await old.stop()
        }
    }

    // MARK: Reading

    func refresh() async {
        do {
            let fresh = try await connection.send(.overview, as: Overview.self)
            overview = fresh
            if !fresh.hostName.isEmpty, fresh.hostName != hostName {
                hostName = fresh.hostName
                save()
            }
        } catch let failure as RemoteFailure {
            if failure != .unreachable, failure != .sessionRestarted { problem = failure.message }
        } catch {}
    }

    func refreshInbox() async {
        guard features.contains("inbox") else { return }
        if let fresh = try? await connection.send(.inbox, as: InboxSnapshot.self) { inbox = fresh }
    }

    private func scheduleInbox() {
        inboxFetch?.cancel()
        inboxFetch = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(500))
            guard !Task.isCancelled else { return }
            await self?.refreshInbox()
        }
    }

    private func merge(_ update: ChatStatusUpdate) {
        guard var current = overview else { return }
        guard let at = current.chats.firstIndex(where: { $0.chat.id == update.chatId }) else {
            // A chat made elsewhere: one read brings it, however many updates announce it.
            refetch?.cancel()
            refetch = Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(400))
                guard !Task.isCancelled else { return }
                await self?.refresh()
            }
            return
        }
        current.chats[at].chat.status = update.status
        current.chats[at].chat.needsAttention = update.needsAttention
        if let title = update.title { current.chats[at].chat.title = title }
        current.chats[at].chat.updatedAt = update.updatedAt
        current.chats.sort { $0.chat.updatedAt > $1.chat.updatedAt }
        overview = current
    }

    // MARK: What the lists show

    var chats: [WorkspaceChat] { overview?.chats.filter { !$0.archived } ?? [] }

    var needsYou: [WorkspaceChat] { chats.filter(\.chat.needsAttention) }

    var working: [WorkspaceChat] {
        chats.filter { !$0.chat.needsAttention && $0.chat.status.isWorking }
    }

    var recent: [WorkspaceChat] {
        chats.filter { !$0.chat.needsAttention && !$0.chat.status.isWorking }
    }

    func project(_ id: String) -> ProjectSummary? { overview?.projects.first { $0.id == id } }

    func summary(_ chatId: String) -> WorkspaceChat? { overview?.chats.first { $0.chat.id == chatId } }

    func asking(_ chatId: String) -> InboxSnapshot.Asking? {
        inbox?.asking.first { $0.chatId == chatId }
    }

    func workingLine(_ chatId: String) -> InboxSnapshot.Working? {
        inbox?.working.first { $0.chatId == chatId }
    }

    func recentLine(_ chatId: String) -> InboxSnapshot.Recent? {
        inbox?.recent.first { $0.chatId == chatId }
    }

    func agentName(_ id: String) -> String {
        overview?.agents.first { $0.id == id }?.name ?? id
    }

    var offersPickers: Bool { features.contains("pickers") }
    var offersControl: Bool { features.contains("control") }
    var offersReview: Bool { features.contains("review") }
    var offersMemory: Bool { features.contains("memory") }
    var offersScreens: Bool { features.contains("screens") }
    var offersTasks: Bool { features.contains("tasks") }
    var offersPhotos: Bool { features.contains("attachments") }

    // MARK: Acting

    /// Opens a chat, closing the one open before.
    func open(_ chatId: String) -> ChatStore {
        if let chat, chat.chatId == chatId { return chat }
        chat?.dropped()
        let store = ChatStore(chatId: chatId, computer: self)
        chat = store
        Task { await store.open() }
        return store
    }

    func close(_ store: ChatStore) {
        guard chat === store else { return }
        chat = nil
        store.dropped()
        Task { try? await connection.perform(.close) }
    }

    /// Answers a permission request from wherever it is shown.
    func allow(_ chatId: String, request: String, option: String?) async throws {
        try await connection.perform(.permission(chatId, request: request, option: option), timeout: .act)
        await refreshInbox()
    }

    func stopTurn(_ chatId: String) async throws {
        try await connection.perform(.interrupt(chatId), timeout: .act)
    }

    /// Starts a chat with its first message and photos; returns its id.
    func create(folderId: String, agentId: String?, text: String, photos: [PickedPhoto] = []) async throws -> String {
        let ids = photos.isEmpty ? [] : try await upload(photos)
        let created = try await connection.send(
            .create(folderId: folderId, agentId: agentId, text: text, attachments: ids), as: Created.self,
            timeout: .slow)
        await refresh()
        return created.chat.chat.id
    }

    /// Hands work to o7 as a task on its own branch, now or tonight.
    func startTask(
        goal: String, projectId: String?, agentId: String?, review: Bool, tonight: Bool, access: String?,
        photos: [PickedPhoto] = []
    ) async throws -> StartedTask {
        let ids = photos.isEmpty ? [] : try await upload(photos)
        let started = try await connection.send(
            .task(
                goal: goal, projectId: projectId, agentId: agentId, review: review, tonight: tonight, access: access,
                attachments: ids),
            as: StartedTask.self, timeout: .slow)
        if !tonight { await refresh() }
        return started
    }

    /// Unpairs: the computer forgets this phone and this phone its key.
    func forget() async {
        try? await connection.perform(.forget)
        backend.computers.forget(hostId)
        stop()
    }

    /// Deletes this computer's key here without asking it anything (after it refused the key).
    func discard() {
        backend.computers.forget(hostId)
        stop()
    }
}
