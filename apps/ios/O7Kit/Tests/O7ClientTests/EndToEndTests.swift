import Foundation
import O7Client
import O7Crypto
import O7Wire
import Testing

/// The Swift client against the real relay and the desktop's real connection task, through the
/// relay's app path. Run by `apps/ios/scripts/e2e.mjs`, which starts both; skipped otherwise. One
/// at a time: they share the demo computer.
@Suite(.serialized)
struct EndToEnd {
    static let relay = ProcessInfo.processInfo.environment["O7_E2E_RELAY"]
    static let control = ProcessInfo.processInfo.environment["O7_E2E_CONTROL"]
    static let first = ProcessInfo.processInfo.environment["O7_E2E_FIRST"]

    static func freshLink() async throws -> String {
        var request = URLRequest(url: URL(string: control! + "/pair")!)
        request.httpMethod = "POST"
        let (data, _) = try await URLSession.shared.data(for: request)
        return String(decoding: data, as: UTF8.self)
    }

    static func connect(_ credential: Credential, name: String = "iPhone") -> HostConnection {
        HostConnection(
            credential: credential, endpoint: RelayEndpoint(origin: URL(string: relay!)!),
            transport: URLSessionTransport(), client: ClientInfo(name: name, version: "1.0", model: "iPhone18,1"))
    }

    @Test(.enabled(if: EndToEnd.relay != nil))
    func pairsFollowsATurnAndAnswersThroughTheRelay() async throws {
        let link = try #require(PairingLink.parse(Self.first!, sites: PairingLink.developmentSites))
        let connection = Self.connect(.pair(link))
        let watcher = Watcher(connection)
        await connection.start()
        let paired = try await watcher.wait("paired", timeout: .seconds(20)) {
            if case .paired = $0 { return true }
            return false
        }
        guard case .paired(let device, let features) = paired else { return }
        // The app path marks the socket native: the computer offers the app's features.
        #expect(features.contains("pickers"))
        #expect(features.contains("inbox"))
        try await watcher.phase(.ready, timeout: .seconds(20))

        let overview = try await connection.send(.overview, as: Overview.self)
        #expect(overview.hostName == "Studio")
        let chat = try #require(overview.chats.first { $0.chat.title == "Fix the login form" })
        let attach = try await connection.send(.open(chat.chat.id), as: ChatAttach.self)
        #expect(!attach.history.items.isEmpty)

        let ack = try await connection.send(
            .send(chat.chat.id, text: "Add the check and run the tests"), as: PromptAck.self, timeout: .act)
        #expect(!ack.promptId.isEmpty)
        let asked = try await watcher.wait("a permission request", timeout: .seconds(20)) {
            if case .frame(let batch) = $0 {
                return batch.events.contains { if case .permissionRequested = $0 { true } else { false } }
            }
            return false
        }
        guard case .frame(let batch) = asked else { return }
        let request = try #require(
            batch.events.compactMap { event -> PermissionRequest? in
                if case .permissionRequested(let request) = event { return request }
                return nil
            }.first)
        let inbox = try await connection.send(.inbox, as: InboxSnapshot.self)
        #expect(inbox.asking.contains { $0.request?.id == request.requestId })
        let allow = try #require(request.options.first { $0.kind == .allowOnce })
        try await connection.perform(
            .permission(chat.chat.id, request: request.requestId, option: allow.optionId), timeout: .act)
        _ = try await watcher.wait("the turn's end", timeout: .seconds(20)) {
            if case .frame(let batch) = $0 {
                return batch.events.contains { if case .turnEnded = $0 { true } else { false } }
            }
            return false
        }

        // A second iPhone pairs with its own link and sees the same chats.
        let second = try #require(PairingLink.parse(try await Self.freshLink(), sites: PairingLink.developmentSites))
        let other = Self.connect(.pair(second), name: "Second iPhone")
        let otherWatch = Watcher(other)
        await other.start()
        try await otherWatch.phase(.ready, timeout: .seconds(20))
        let seen = try await other.send(.overview, as: Overview.self)
        #expect(seen.chats.contains { $0.chat.id == chat.chat.id })
        try? await other.perform(.forget)
        await other.stop()

        // The device key opens a session of its own next time.
        await connection.stop()
        let again = Self.connect(.device(device))
        let againWatch = Watcher(again)
        await again.start()
        try await againWatch.phase(.ready, timeout: .seconds(20))
        _ = try await again.send(.overview, as: Overview.self)
        await again.stop()
    }

    /// Everything the app does beyond the browser's remote, each through the relay to the desktop's
    /// real handling and the demo computer's answers. It works in a task chat of its own, so the
    /// demo's first chats stay as the other test expects them.
    @Test(.enabled(if: EndToEnd.relay != nil))
    func everyAppOperationReachesTheComputer() async throws {
        let link = try #require(PairingLink.parse(try await Self.freshLink(), sites: PairingLink.developmentSites))
        let connection = Self.connect(.pair(link), name: "iPhone tools")
        let watcher = Watcher(connection)
        await connection.start()
        let paired = try await watcher.wait("paired", timeout: .seconds(20)) {
            if case .paired = $0 { return true }
            return false
        }
        guard case .paired(_, let features) = paired else { return }
        for feature in ["control", "review", "memory", "screens", "tasks", "attachments"] {
            #expect(features.contains(feature), "the app path offers \(feature)")
        }
        try await watcher.phase(.ready, timeout: .seconds(20))

        // A task now: a chat on a branch of its own.
        let started = try await connection.send(
            .task(
                goal: "Check what the phone sends", projectId: "p1", agentId: nil, review: true, tonight: false,
                access: "auto_edit"),
            as: StartedTask.self, timeout: .slow)
        let chatId = try #require(started.chatId)
        #expect(started.branch?.hasPrefix("o7/") == true)

        // What it stands on and what its branch changed.
        let facts = try await connection.send(.facts(chatId), as: ChatFacts.self)
        #expect(facts.worktree)
        #expect(facts.pullRequest?.checks == "passed")
        let changes = try await connection.send(.changes(chatId, scope: .branch), as: ChangedFiles.self)
        let file = try #require(changes.files.first { $0.path == "src/login.ts" })
        #expect(file.kind == "modified")
        #expect(changes.base != nil)
        let diff = try await connection.send(.diff(chatId, path: file.path, scope: .branch), as: FileDiff.self)
        #expect(diff.after != nil)
        await #expect(throws: RemoteFailure.self) {
            try await connection.send(.diff(chatId, path: "../secrets.txt", scope: .branch), as: FileDiff.self)
        }

        // Its browser, and no device.
        let picture = try await connection.send(.picture(chatId, of: .browser), as: ScreenPicture.self)
        #expect(Data(base64Encoded: picture.data) != nil)
        await #expect(throws: RemoteFailure.self) {
            try await connection.send(.picture(chatId, of: .device), as: ScreenPicture.self)
        }

        // Access, title, archiving.
        let choices = try await connection.send(.options(chatId), as: Choices.self)
        #expect(choices.access?.map(\.value).contains("full_auto") == true)
        try await connection.perform(.setAccess(chatId, access: "ask"))
        try await connection.perform(.rename(chatId, title: "Photos from the phone"))
        var overview = try await connection.send(.overview, as: Overview.self)
        #expect(overview.chats.first { $0.chat.id == chatId }?.chat.title == "Photos from the phone")
        try await connection.perform(.archive(chatId, archived: true))
        let archived = try await connection.send(.archived(), as: ArchivedChats.self)
        #expect(archived.chats.contains { $0.chat.id == chatId })
        overview = try await connection.send(.overview, as: Overview.self)
        #expect(!overview.chats.contains { $0.chat.id == chatId })
        try await connection.perform(.archive(chatId, archived: false))

        // The project's brief, saved only over the revision it was read at.
        let brief = try await connection.send(.memory("p1"), as: ProjectBrief.self)
        let saved = try await connection.send(
            .setMemory("p1", markdown: brief.markdown + "\nKeep commits small.", base: brief.revision),
            as: SavedBrief.self)
        #expect(saved.revision != nil)
        #expect(saved.revision != brief.revision)
        if brief.revision != nil {
            await #expect(throws: RemoteFailure.self) {
                try await connection.send(.setMemory("p1", markdown: "Stale.", base: brief.revision), as: SavedBrief.self)
            }
        }

        // Work for tonight, listed and taken off.
        let later = try await connection.send(
            .task(
                goal: "Update the dependencies", projectId: "p1", agentId: nil, review: false, tonight: true,
                access: nil),
            as: StartedTask.self)
        let queued = try #require(later.queued)
        var tasks = try await connection.send(.tasks, as: TasksSnapshot.self)
        #expect(tasks.queued.contains { $0.id == queued })
        #expect(tasks.tasks.contains { $0.chatId == chatId })
        try await connection.perform(.taskCancel(queued))
        tasks = try await connection.send(.tasks, as: TasksSnapshot.self)
        #expect(!tasks.queued.contains { $0.id == queued })

        // The task's turn waits on a permission, as every demo turn does: stopped, the chat takes
        // a message again.
        try await connection.perform(.interrupt(chatId), timeout: .act)
        try await Self.untilIdle(chatId, connection)

        // A photo, in pieces, then a message with it and no words.
        let photo = Self.png(width: 300, height: 200)
        let uploadId = try await connection.upload(photo, name: "photo-1.png", mime: "image/png")
        let ack = try await connection.send(
            .send(chatId, text: "", attachments: [uploadId]), as: PromptAck.self, timeout: .act)
        #expect(!ack.promptId.isEmpty)
        try await connection.perform(.interrupt(chatId), timeout: .act)
        try await Self.untilIdle(chatId, connection)
        // Spent: the same photo does not go twice.
        await #expect(throws: RemoteFailure.self) {
            try await connection.send(.send(chatId, text: "Again", attachments: [uploadId]), as: PromptAck.self)
        }

        // A new chat whose first message is a photo alone.
        let second = try await connection.upload(Self.png(width: 64, height: 64), name: "photo-2.png", mime: "image/png")
        let created = try await connection.send(
            .create(folderId: "f1", agentId: nil, text: "", attachments: [second]), as: Created.self, timeout: .slow)
        #expect(created.unsent == nil)
        try? await connection.perform(.interrupt(created.chat.chat.id), timeout: .act)
        try? await connection.perform(.forget)
        await connection.stop()
    }

    /// Waits until a chat no longer runs.
    static func untilIdle(_ chatId: String, _ connection: HostConnection) async throws {
        let deadline = ContinuousClock.now + .seconds(15)
        while ContinuousClock.now < deadline {
            let overview = try await connection.send(.overview, as: Overview.self)
            if overview.chats.first(where: { $0.chat.id == chatId })?.chat.status.isWorking == false { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        Issue.record("the chat still runs")
    }

    /// A plain PNG of the given size, as a photo stands in for one.
    static func png(width: Int, height: Int) -> Data {
        // A PNG with one IDAT of stored (uncompressed) deflate blocks: no image library needed.
        func crc(_ bytes: [UInt8]) -> UInt32 {
            var table = [UInt32](repeating: 0, count: 256)
            for n in 0..<256 {
                var c = UInt32(n)
                for _ in 0..<8 { c = (c & 1) != 0 ? 0xEDB8_8320 ^ (c >> 1) : c >> 1 }
                table[n] = c
            }
            var c: UInt32 = 0xFFFF_FFFF
            for byte in bytes { c = table[Int((c ^ UInt32(byte)) & 0xFF)] ^ (c >> 8) }
            return c ^ 0xFFFF_FFFF
        }
        func be(_ value: UInt32) -> [UInt8] { [UInt8(value >> 24), UInt8(value >> 16 & 0xFF), UInt8(value >> 8 & 0xFF), UInt8(value & 0xFF)] }
        func chunk(_ type: String, _ data: [UInt8]) -> [UInt8] {
            let body = Array(type.utf8) + data
            return be(UInt32(data.count)) + body + be(crc(body))
        }
        var raw: [UInt8] = []
        for y in 0..<height {
            raw.append(0)
            for x in 0..<width { raw += [UInt8(x % 256), UInt8(y % 256), 0x80] }
        }
        var deflate: [UInt8] = [0x78, 0x01]
        var offset = 0
        while offset < raw.count {
            let end = min(offset + 65_535, raw.count)
            let length = UInt16(end - offset)
            deflate.append(end == raw.count ? 1 : 0)
            deflate += [UInt8(length & 0xFF), UInt8(length >> 8), UInt8(~length & 0xFF), UInt8(~length >> 8)]
            deflate += raw[offset..<end]
            offset = end
        }
        var a: UInt32 = 1
        var b: UInt32 = 0
        for byte in raw {
            a = (a + UInt32(byte)) % 65_521
            b = (b + a) % 65_521
        }
        deflate += be(b << 16 | a)
        let header = be(UInt32(width)) + be(UInt32(height)) + [8, 2, 0, 0, 0]
        let png: [UInt8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
            + chunk("IHDR", header) + chunk("IDAT", deflate) + chunk("IEND", [])
        return Data(png)
    }

    /// A notification, end to end: the phone registers, the demo agent asks, the computer seals
    /// what it asks for this phone alone, the relay hands Apple only the fixed words and the sealed
    /// blob, and the phone opens it with the key derived from its device key and answers from it.
    @Test(.enabled(if: EndToEnd.relay != nil))
    func aNotificationIsSealedForThisPhoneAndAnsweredFromIt() async throws {
        let link = try #require(PairingLink.parse(try await Self.freshLink(), sites: PairingLink.developmentSites))
        let connection = Self.connect(.pair(link), name: "iPhone notifications")
        let watcher = Watcher(connection)
        await connection.start()
        let paired = try await watcher.wait("paired", timeout: .seconds(20)) {
            if case .paired = $0 { return true }
            return false
        }
        guard case .paired(let device, let features) = paired else { return }
        #expect(features.contains("push"))
        try await watcher.phase(.ready, timeout: .seconds(20))

        let token = String(repeating: "ab", count: 32)
        let registered = try await connection.send(
            .pushRegister(token: token, env: "sandbox", prefs: .all), as: PushRegistered.self)
        #expect(registered.relay, "the relay announced that it sends notifications")

        // A new chat: every demo turn asks for a permission.
        let created = try await connection.send(
            .create(folderId: "f1", agentId: nil, text: "Run the tests"), as: Created.self, timeout: .slow)
        let chatId = created.chat.chat.id
        let push = try await Self.push(to: token, of: "alert")
        #expect(push.url == "https://api.sandbox.push.apple.com/3/device/\(token)")
        #expect(push.headers["apns-topic"] == "codes.o7.mobile")
        #expect(push.headers["authorization"]?.hasPrefix("bearer ") == true)
        // Apple sees the fixed words and a blob, never what was asked.
        let body = try #require(try JSONSerialization.jsonObject(with: Data(push.body.utf8)) as? [String: Any])
        let alert = try #require((body["aps"] as? [String: Any])?["alert"] as? [String: String])
        #expect(alert == ["title": "o7", "body": "Something changed on your computer."])
        #expect(!push.body.contains("Run the tests"))
        let envelope = try #require(PushEnvelope(userInfo: body))
        #expect(envelope.device == device.deviceId)

        let key = PushSeal.key(deviceKey: device.key)
        let plain = try #require(
            PushSeal.open(envelope.blob, key: key, hostId: device.hostId, deviceId: device.deviceId))
        let payload = try JSONDecoder().decode(PushPayload.self, from: plain)
        #expect(payload.kind == .asking)
        #expect(payload.chat == chatId)
        let request = try #require(payload.request)
        let allow = try #require(request.allow, "a permission offers Allow once")
        // Answered from the notification, as the phone's button does.
        try await connection.perform(.permission(chatId, request: request.id, option: allow), timeout: .act)

        try await connection.perform(.pushUnregister)
        try? await connection.perform(.forget)
        await connection.stop()
    }

    struct CapturedPush: Decodable {
        var url: String
        var headers: [String: String]
        var body: String
    }

    /// The first notification the relay sent Apple for `token`, waiting for it.
    static func push(to token: String, of type: String) async throws -> CapturedPush {
        let deadline = ContinuousClock.now + .seconds(20)
        while ContinuousClock.now < deadline {
            let (data, _) = try await URLSession.shared.data(from: URL(string: control! + "/pushes")!)
            let pushes = try JSONDecoder().decode([CapturedPush].self, from: data)
            if let found = pushes.first(where: {
                $0.url.hasSuffix("/3/device/\(token)") && $0.headers["apns-push-type"] == type
            }) {
                return found
            }
            try await Task.sleep(for: .milliseconds(200))
        }
        Issue.record("no \(type) notification for the phone within 20 s")
        throw CancellationError()
    }

    @Test(.enabled(if: EndToEnd.relay != nil))
    func theAppPathRefusesAWebPage() async throws {
        let link = try #require(PairingLink.parse(Self.first!, sites: PairingLink.developmentSites))
        var request = URLRequest(url: RelayEndpoint(origin: URL(string: Self.relay!)!).url(hostId: link.hostId))
        request.setValue("https://o7.codes", forHTTPHeaderField: "Origin")
        let task = URLSession.shared.webSocketTask(with: request)
        task.resume()
        await #expect(throws: (any Error).self) { _ = try await task.receive() }
    }
}
