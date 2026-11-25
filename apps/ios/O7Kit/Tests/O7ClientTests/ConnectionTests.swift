import Foundation
import O7Client
import O7Crypto
import O7MockHost
import O7Wire
import Testing

/// Collects a connection's events and waits for the ones a test expects.
actor Watcher {
    private(set) var seen: [HostEvent] = []
    private var task: Task<Void, Never>?
    /// Each wait looks only at what came after the last event a wait matched.
    private var cursor = 0

    init(_ connection: HostConnection) {
        let stream = connection.events
        task = nil
        Task { await self.follow(stream) }
    }

    private func follow(_ stream: AsyncStream<HostEvent>) async {
        for await event in stream { seen.append(event) }
    }

    func wait(
        _ what: String, timeout: Duration = .seconds(5), until match: @Sendable (HostEvent) -> Bool
    ) async throws -> HostEvent {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            while cursor < seen.count {
                let event = seen[cursor]
                cursor += 1
                if match(event) { return event }
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        Issue.record("no \(what) within \(timeout)")
        throw CancellationError()
    }

    func phase(_ wanted: ConnectionPhase, timeout: Duration = .seconds(5)) async throws {
        _ = try await wait("phase \(wanted)", timeout: timeout) {
            if case .phase(let phase) = $0 { return phase == wanted }
            return false
        }
    }
}

func quick() -> Timing {
    var timing = Timing()
    timing.ping = .milliseconds(800)
    timing.pong = .milliseconds(400)
    timing.probe = .milliseconds(300)
    timing.backoffCap = .milliseconds(300)
    return timing
}

let phone = ClientInfo(name: "iPhone", version: "1.0", model: "iPhone18,1")
let endpoint = RelayEndpoint(site: URL(string: "https://o7.codes")!)!

@Test func pairsThenWorksAsTheDevice() async throws {
    let computer = MockComputer(pace: .milliseconds(5))
    let link = await computer.pairingLink()
    let connection = HostConnection(
        credential: .pair(link), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    let paired = try await watcher.wait("paired") {
        if case .paired = $0 { return true }
        return false
    }
    guard case .paired(let device, let features) = paired else { return }
    #expect(device.hostId == computer.hostId)
    #expect(device.key.count == 32)
    #expect(features.contains("inbox"))
    try await watcher.phase(.ready)
    let overview = try await connection.send(.overview, as: Overview.self)
    #expect(overview.hostName == "Studio")
    #expect(overview.chats.count == 4)
    #expect(await computer.deviceCount == 1)

    // The same link is spent: another phone is told so, and nothing else.
    let stranger = HostConnection(
        credential: .pair(link), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let strangerWatch = Watcher(stranger)
    await stranger.start()
    let ended = try await strangerWatch.wait("ended") {
        if case .phase(.ended) = $0 { return true }
        return false
    }
    guard case .phase(.ended(let reason, let refused)) = ended else { return }
    #expect(reason.contains("already used or has expired"))
    #expect(!refused)
    await stranger.stop()

    // The device key opens a session of its own next time.
    await connection.stop()
    let again = HostConnection(
        credential: .device(device), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let againWatch = Watcher(again)
    await again.start()
    try await againWatch.phase(.ready)
    let inbox = try await again.send(.inbox, as: InboxSnapshot.self)
    #expect(inbox.asking.first?.request?.title == "Run `pnpm lint --fix`")
    #expect(inbox.asking.first?.request?.options?.first?.kind == .allowOnce)
    await again.stop()
}

@Test func anUnknownDeviceIsTurnedAwayAndNothingIsDeleted() async throws {
    let computer = MockComputer()
    let device = DeviceCredential(hostId: computer.hostId, deviceId: "AAAAAAAAAAA", key: randomBytes(32))
    let connection = HostConnection(
        credential: .device(device), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    let ended = try await watcher.wait("ended") {
        if case .phase(.ended) = $0 { return true }
        return false
    }
    guard case .phase(.ended(let reason, let refused)) = ended else { return }
    #expect(refused)
    #expect(reason.contains("no longer accepts this iPhone"))
    await connection.stop()
}

@Test func aComputerThatGoesAwayComesBackWithoutHelp() async throws {
    let computer = MockComputer()
    let known = await computer.addDevice()
    let device = DeviceCredential(hostId: computer.hostId, deviceId: known.id, key: known.key)
    let connection = HostConnection(
        credential: .device(device), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    try await watcher.phase(.ready)
    await computer.setPresent(false)
    try await watcher.phase(.away)
    await #expect(throws: RemoteFailure.unreachable) {
        try await connection.send(.overview, as: Overview.self)
    }
    await computer.setPresent(true)
    try await watcher.phase(.ready)
    _ = try await connection.send(.overview, as: Overview.self)
    await connection.stop()
}

@Test func aLiveTurnArrivesAndAnAnswerFinishesIt() async throws {
    let computer = MockComputer(pace: .milliseconds(5))
    let known = await computer.addDevice()
    let device = DeviceCredential(hostId: computer.hostId, deviceId: known.id, key: known.key)
    let connection = HostConnection(
        credential: .device(device), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    try await watcher.phase(.ready)
    let attach = try await connection.send(.open("c1"), as: ChatAttach.self)
    #expect(attach.history.items.count == attach.nextSeq.asInt)
    let ack = try await connection.send(.send("c1", text: "Add the check"), as: PromptAck.self, timeout: .act)
    #expect(!ack.queued)
    let asked = try await watcher.wait("permission") {
        if case .frame(let batch) = $0 {
            return batch.events.contains { if case .permissionRequested = $0 { return true } else { return false } }
        }
        return false
    }
    guard case .frame(let batch) = asked else { return }
    #expect(batch.seqStart >= attach.nextSeq)
    _ = try await watcher.wait("needs you") {
        if case .chat(let update) = $0 { return update.chatId == "c1" && update.needsAttention }
        return false
    }
    let request = batch.events.compactMap { event -> PermissionRequest? in
        if case .permissionRequested(let request) = event { return request }
        return nil
    }.first!
    try await connection.perform(.permission("c1", request: request.requestId, option: "allow"), timeout: .act)
    _ = try await watcher.wait("turn end") {
        if case .frame(let batch) = $0 {
            return batch.events.contains { if case .turnEnded = $0 { return true } else { return false } }
        }
        return false
    }
    _ = try await watcher.wait("idle") {
        if case .chat(let update) = $0 { return update.chatId == "c1" && update.status == .ready }
        return false
    }
    await connection.stop()
}

@Test func aQuietSocketIsReplacedWhenTheAppWakes() async throws {
    let computer = MockComputer()
    let known = await computer.addDevice()
    let device = DeviceCredential(hostId: computer.hostId, deviceId: known.id, key: known.key)
    let transport = MockTransport(computer: computer)
    let connection = HostConnection(
        credential: .device(device), endpoint: endpoint, transport: transport, client: phone,
        timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    try await watcher.phase(.ready)
    // The phone slept: its socket says it is open and carries nothing.
    transport.silenceAll()
    await connection.wake()
    try await watcher.phase(.connecting, timeout: .seconds(3))
    try await watcher.phase(.ready, timeout: .seconds(5))
    _ = try await connection.send(.overview, as: Overview.self)
    await connection.stop()
}

@Test func anOperationTheComputerDoesNotOfferIsNeverSent() async throws {
    let computer = MockComputer()
    await computer.setFeatures(["pickers"])
    let known = await computer.addDevice()
    let device = DeviceCredential(hostId: computer.hostId, deviceId: known.id, key: known.key)
    let connection = HostConnection(
        credential: .device(device), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    try await watcher.phase(.ready)
    await #expect(throws: RemoteFailure.notOffered) {
        try await connection.send(.inbox, as: InboxSnapshot.self)
    }
    #expect(await !computer.asked.contains("inbox"))
    await connection.stop()
}

@Test func manyRequestsAtOnceAllAnswerInOrder() async throws {
    let computer = MockComputer()
    let known = await computer.addDevice()
    let device = DeviceCredential(hostId: computer.hostId, deviceId: known.id, key: known.key)
    let connection = HostConnection(
        credential: .device(device), endpoint: endpoint, transport: MockTransport(computer: computer),
        client: phone, timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    try await watcher.phase(.ready)
    try await withThrowingTaskGroup(of: String.self) { group in
        for _ in 0..<24 {
            group.addTask { try await connection.send(.overview, as: Overview.self).hostName }
        }
        for try await name in group { #expect(name == "Studio") }
    }
    await connection.stop()
}

extension Int64 {
    var asInt: Int { Int(self) }
}
