import Foundation
import GhostClient
import GhostCrypto
import GhostMockHost
import GhostWire
import Testing

/// A session with the made-up computer as a device it already knows.
private func session(_ computer: MockComputer) async throws -> HostConnection {
    let (id, key) = await computer.addDevice()
    let connection = HostConnection(
        credential: .device(DeviceCredential(hostId: computer.hostId, deviceId: id, key: key)), endpoint: endpoint,
        transport: MockTransport(computer: computer), client: phone, timing: quick())
    let watcher = Watcher(connection)
    await connection.start()
    try await watcher.phase(.ready)
    return connection
}

@Test func aPhotoArrivesWholeAPieceAtATime() async throws {
    let computer = MockComputer(pace: .milliseconds(1))
    let connection = try await session(computer)
    // Three chunks: two full, one short.
    let photo = randomBytes(49_152 * 2 + 1_000)
    let heard = Heard()
    let uploadId = try await connection.upload(photo, name: "photo-1.jpg", mime: "image/jpeg") { fraction in
        Task { await heard.add(fraction) }
    }
    #expect(await computer.uploaded(uploadId) == photo)
    try await Task.sleep(for: .milliseconds(50))
    #expect(await heard.values.last == 1)
    await connection.stop()
}

@Test func aBriefSavesOnlyOverTheVersionItWasReadFrom() async throws {
    let computer = MockComputer(pace: .milliseconds(1))
    let connection = try await session(computer)
    let brief = try await connection.send(.memory("p1"), as: ProjectBrief.self)
    #expect(brief.markdown.contains("No em dashes"))
    let saved = try await connection.send(
        .setMemory("p1", markdown: "# Ghost\n\nShorter.", base: brief.revision), as: SavedBrief.self)
    #expect(saved.revision != brief.revision)
    // A second save from the old read is refused: the brief changed since.
    await #expect(throws: RemoteFailure.refused(code: "conflict", message: "The brief changed on the computer since you opened it.")) {
        try await connection.send(.setMemory("p1", markdown: "# Ghost\n\nLonger.", base: brief.revision), as: SavedBrief.self)
    }
    let again = try await connection.send(.memory("p1"), as: ProjectBrief.self)
    #expect(again.markdown == "# Ghost\n\nShorter.")
    await connection.stop()
}

@Test func tonightsWorkIsQueuedListedAndCancelled() async throws {
    let computer = MockComputer(pace: .milliseconds(1))
    let connection = try await session(computer)
    let started = try await connection.send(
        .task(
            goal: "Make the settings screen load faster", projectId: "p1", agentId: nil, review: true,
            tonight: true, access: "auto_edit"),
        as: StartedTask.self)
    let queued = try #require(started.queued)
    #expect(started.chatId == nil)
    var tasks = try await connection.send(.tasks, as: TasksSnapshot.self)
    #expect(tasks.queued.contains { $0.id == queued && $0.goal == "Make the settings screen load faster" })
    #expect(tasks.night?.from == 1)
    try await connection.perform(.taskCancel(queued))
    tasks = try await connection.send(.tasks, as: TasksSnapshot.self)
    #expect(!tasks.queued.contains { $0.id == queued })
    // Ids are never reused, so cancelling again finds nothing.
    await #expect(throws: RemoteFailure.self) { try await connection.perform(.taskCancel(queued)) }

    // Work started now opens a chat on its own branch.
    let now = try await connection.send(
        .task(
            goal: "Fix the flaky test", projectId: "p1", agentId: "codex", review: true, tonight: false,
            access: nil),
        as: StartedTask.self)
    #expect(now.chatId != nil)
    #expect(now.branch == "ghost/fix-the-flaky-test")
    await connection.stop()
}

@Test func aChatIsRenamedArchivedAndBroughtBack() async throws {
    let computer = MockComputer(pace: .milliseconds(1))
    let connection = try await session(computer)
    try await connection.perform(.rename("c1", title: "Check emails first"))
    var overview = try await connection.send(.overview, as: Overview.self)
    #expect(overview.chats.first { $0.chat.id == "c1" }?.chat.title == "Check emails first")

    try await connection.perform(.archive("c1", archived: true))
    overview = try await connection.send(.overview, as: Overview.self)
    #expect(!overview.chats.contains { $0.chat.id == "c1" })
    let archived = try await connection.send(.archived(), as: ArchivedChats.self)
    #expect(archived.chats.map(\.chat.id) == ["c1"])
    #expect(!archived.more)

    try await connection.perform(.archive("c1", archived: false))
    overview = try await connection.send(.overview, as: Overview.self)
    #expect(overview.chats.contains { $0.chat.id == "c1" })
    await connection.stop()
}

@Test func whatAChatStandsOnAndWhatItChanged() async throws {
    let computer = MockComputer(pace: .milliseconds(1))
    let connection = try await session(computer)
    let facts = try await connection.send(.facts("c1"), as: ChatFacts.self)
    #expect(facts.worktree)
    #expect(facts.pullRequest?.number == 12)
    #expect(facts.pullRequest?.notes.first?.trusted == true)
    let turn = try await connection.send(.changes("c1"), as: ChangedFiles.self)
    let branch = try await connection.send(.changes("c1", scope: .branch), as: ChangedFiles.self)
    #expect(turn.files.count == 2)
    #expect(branch.files.count == 3)
    #expect(branch.base != nil)
    let diff = try await connection.send(.diff("c1", path: "src/login.ts"), as: FileDiff.self)
    #expect(diff.before?.contains("post(email)") == true)
    #expect(diff.after?.contains("setError") == true)
    let picture = try await connection.send(.picture("c1", of: .browser), as: ScreenPicture.self)
    #expect(picture.mime == "image/png")
    #expect(Data(base64Encoded: picture.data)?.starts(with: [0x89, 0x50, 0x4E, 0x47]) == true)
    await connection.stop()
}

@Test func aComputerWithoutControlIsNeverAskedToRename() async throws {
    let computer = MockComputer(pace: .milliseconds(1))
    await computer.setFeatures(["pickers", "inbox", "search"])
    let connection = try await session(computer)
    await #expect(throws: RemoteFailure.notOffered) { try await connection.perform(.rename("c1", title: "No")) }
    await connection.stop()
}

private actor Heard {
    private(set) var values: [Double] = []
    func add(_ value: Double) { values.append(value) }
}
