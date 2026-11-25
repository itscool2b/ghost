import Foundation
import O7Client
import O7Crypto
import O7Wire
import Testing

/// The Swift client against the real relay and the desktop's real connection task, through the
/// relay's app path. Run by `apps/ios/scripts/e2e.mjs`, which starts both; skipped otherwise.
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
