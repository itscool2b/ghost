import CryptoKit
public import Foundation
public import O7Crypto
public import O7Wire

/// A computer in a box: the desktop's half of the sealed protocol (`host/session.rs`) in front of
/// a made-up workspace whose agent follows a script. The app's tests, previews and mock mode talk
/// to it through `MockTransport` exactly as they would through the relay.
public actor MockComputer {
    public nonisolated let hostId: String
    public nonisolated let hostName: String
    public var look = Look(theme: "claude", appearance: "dark")
    public var features = [
        "pickers", "inbox", "search", "control", "review", "memory", "screens", "tasks", "attachments", "push",
        "activity",
    ]
    /// Seconds between the scripted agent's steps.
    public var pace: Duration
    /// While false, the relay says the computer is away.
    public private(set) var present = true

    private var pairing: (id: String, key: Data)?
    private var devices: [String: Data] = [:]
    var chats: [MockChat]
    var state = MockState()
    private var sessions: [Int: Session] = [:]
    private var nextSession = 0
    private var turns = 0
    /// Every operation the computer was asked for, for tests.
    public private(set) var asked: [String] = []

    struct Session {
        let link: MockLink
        var channel: SessionChannel?
        var context: String?
        var device: String?
        var pairing: String?
        var following: String?
        var ready = false
    }

    /// The demo's own ids and key in the app's mock mode, fixed so a notification can be sealed
    /// for it from outside the app (`apps/ios/scripts/push.mjs`). Never a real computer's.
    public static let demoHostId = "TW9jayBjb21wdXRlciAxMg"
    public static let demoDeviceId = "TW9ja0RldjE"
    public static let demoDeviceKey = Data((0..<32).map { UInt8($0) })

    public init(hostName: String = "Studio", hostId: String? = nil, pace: Duration = .milliseconds(350)) {
        self.hostId = hostId ?? Base64URL.encode(randomBytes(16))
        self.hostName = hostName
        self.pace = pace
        chats = Script.chats()
    }

    /// A fresh pairing link for `site`; the last one stops opening.
    public func pairingLink(site: URL = URL(string: "https://o7.codes")!) -> PairingLink {
        let id = Base64URL.encode(randomBytes(8))
        let key = randomBytes(32)
        pairing = (id, key)
        return PairingLink(site: site, hostId: hostId, pairingId: id, key: key)
    }

    /// A device the computer already knows, as if paired earlier.
    public func addDevice() -> (id: String, key: Data) {
        let id = Base64URL.encode(randomBytes(8))
        let key = randomBytes(32)
        devices[id] = key
        return (id, key)
    }

    /// A device the computer knows by an id and key of the caller's.
    public func addDevice(id: String, key: Data) -> (id: String, key: Data) {
        devices[id] = key
        return (id, key)
    }

    public var deviceCount: Int { devices.count }

    public func setPresent(_ value: Bool) {
        present = value
        for session in sessions.values { session.link.deliver(value ? "h1" : "h0") }
    }

    public func setPace(_ value: Duration) { pace = value }

    public func setFeatures(_ value: [String]) { features = value }

    // MARK: Sockets

    func attach(_ link: MockLink) -> Int {
        nextSession += 1
        sessions[nextSession] = Session(link: link)
        link.deliver(present ? "h1" : "h0")
        return nextSession
    }

    func detach(_ id: Int) {
        sessions[id] = nil
    }

    func received(_ text: String, session id: Int) async {
        guard var session = sessions[id] else { return }
        if text == "ping" { return session.link.deliver("pong") }
        guard present else { return session.link.deliver("h0") }
        if text.hasPrefix("mH") {
            hello(String(text.dropFirst(2)), session: &session)
            sessions[id] = session
            return
        }
        guard text.hasPrefix("mE"), var channel = session.channel,
            let opened = try? channel.open(String(text.dropFirst(2)))
        else {
            session.link.closeFromServer()
            sessions[id] = nil
            return
        }
        session.channel = channel
        sessions[id] = session
        await request(opened, session: id)
    }

    private func hello(_ body: String, session: inout Session) {
        struct Hello: Decodable {
            let v: Int
            let kind: String
            let id: String
            let nonce: String
        }
        guard let hello = try? JSONDecoder().decode(Hello.self, from: Data(body.utf8)),
            let clientNonce = Base64URL.decode(hello.nonce, length: 16)
        else { return session.link.closeFromServer() }
        guard hello.v == protocolVersion else {
            return session.link.deliver(#"mR{"reason":"version"}"#)
        }
        let secret: Data?
        if hello.kind == "pair", let pairing, pairing.id == hello.id {
            secret = pairing.key
            session.pairing = hello.id
        } else if hello.kind == "device", let key = devices[hello.id] {
            secret = key
            session.device = hello.id
        } else {
            secret = nil
        }
        guard let secret else { return session.link.deliver(#"mR{"reason":"unknown"}"#) }
        let hostNonce = randomBytes(16)
        var channel = SessionChannel(
            side: .host, secret: SymmetricKey(data: secret), clientNonce: clientNonce,
            hostNonce: hostNonce, context: "\(hello.kind)|\(hello.id)")
        session.link.deliver(#"mW{"v":1,"nonce":"\#(Base64URL.encode(hostNonce))"}"#)
        let ready: JSONValue = .object([
            "ev": .string("ready"), "host_name": .string(hostName),
            "paired": .bool(hello.kind == "device"),
            "features": .array(features.map(JSONValue.string)),
            "look": .object(["theme": .string(look.theme), "appearance": .string(look.appearance)]),
        ])
        if let sealed = try? channel.seal(Self.json(ready)) { session.link.deliver("mE" + sealed) }
        session.channel = channel
        session.ready = true
    }

    // MARK: Operations

    private func send(_ value: JSONValue, to id: Int) {
        guard var session = sessions[id], var channel = session.channel,
            let sealed = try? channel.seal(Self.json(value))
        else { return }
        session.channel = channel
        sessions[id] = session
        session.link.deliver("mE" + sealed)
    }

    func reply(_ re: Double, _ data: JSONValue, to id: Int) {
        send(.object(["re": .number(re), "ok": .bool(true), "data": data]), to: id)
    }

    func refuse(_ re: Double, _ code: String, _ message: String, to id: Int) {
        send(
            .object([
                "re": .number(re), "ok": .bool(false),
                "error": .object(["code": .string(code), "message": .string(message)]),
            ]), to: id)
    }

    private func request(_ text: String, session id: Int) async {
        guard let request = try? JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)),
            let re = request["id"]?.number, let op = request["op"]?.string
        else { return }
        asked.append(op)
        if let pairingId = sessions[id]?.pairing {
            guard op == "pair" else { return refuse(re, "pair_first", "Pair this iPhone first.", to: id) }
            guard pairing?.id == pairingId else {
                return refuse(re, "expired", "This pairing link is no longer open. Create a new one in o7.", to: id)
            }
            let device = Base64URL.encode(randomBytes(8))
            let key = randomBytes(32)
            devices[device] = key
            pairing = nil
            sessions[id]?.pairing = nil
            sessions[id]?.device = device
            return reply(
                re,
                .object([
                    "device_id": .string(device), "device_key": .string(Base64URL.encode(key)),
                    "features": .array(features.map(JSONValue.string)),
                ]), to: id)
        }
        let chatId = request["chat_id"]?.string ?? ""
        switch op {
        case "overview":
            reply(re, (try? JSONValue.of(overview())) ?? .null, to: id)
        case "open":
            guard let chat = chats.first(where: { $0.id == chatId }) else {
                return refuse(re, "not_found", "That chat is gone.", to: id)
            }
            sessions[id]?.following = chatId
            var snapshot = ChatSnapshot(chat: chat.summary.chat, session: SessionInfo(status: chat.summary.chat.status))
            snapshot.tasks = chat.events.compactMap { item in
                if case .taskUpdated(let task) = item.event, task.status == .running { return task }
                return nil
            }
            let attach = ChatAttach(
                subscriptionId: "s\(id)", nextSeq: Int64(chat.events.count), snapshot: snapshot,
                history: HistoryPage(chatId: chatId, items: chat.events, hasMore: false))
            reply(re, (try? JSONValue.of(attach)) ?? .null, to: id)
        case "close":
            sessions[id]?.following = nil
            reply(re, .object([:]), to: id)
        case "history":
            reply(re, (try? JSONValue.of(HistoryPage(chatId: chatId, items: [], hasMore: false))) ?? .null, to: id)
        case "send":
            let text = request["text"]?.string ?? ""
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return refuse(re, "empty", "Write something first.", to: id)
            }
            let queued = chats.first(where: { $0.id == chatId })?.summary.chat.status == .running
            turns += 1
            reply(re, .object(["prompt_id": .string("q\(turns)"), "queued": .bool(queued)]), to: id)
            if !queued { await play(chat: chatId, text: text) }
        case "interrupt":
            reply(re, .object([:]), to: id)
            await stop(chat: chatId)
        case "permission":
            reply(re, .object([:]), to: id)
            await resolve(chat: chatId, option: request["option_id"]?.string)
        case "answer":
            reply(re, .object([:]), to: id)
        case "create":
            let created = newChat(folder: request["folder_id"]?.string ?? "f1", agent: request["agent_id"]?.string)
            turns += 1
            reply(
                re,
                .object([
                    "chat": (try? JSONValue.of(created)) ?? .null,
                    "sent": .object(["prompt_id": .string("q\(turns)"), "queued": .bool(false)]),
                ]), to: id)
            await play(chat: created.chat.id, text: request["text"]?.string ?? "")
        case "options":
            reply(
                re,
                .object([
                    "models": .array([
                        .object(["value": .string("opus"), "name": .string("Opus 5.5")]),
                        .object(["value": .string("sonnet"), "name": .string("Sonnet 5.5")]),
                    ]),
                    "model": .string("opus"),
                    "thinking": .array([
                        .object(["value": .string("low"), "name": .string("Low")]),
                        .object(["value": .string("medium"), "name": .string("Medium")]),
                        .object(["value": .string("high"), "name": .string("High")]),
                    ]),
                    "thinking_level": .string("high"),
                    "access": .array([
                        .object(["value": .string("ask"), "name": .string("Ask first")]),
                        .object(["value": .string("auto_edit"), "name": .string("Edit without asking")]),
                        .object(["value": .string("full_auto"), "name": .string("Full access")]),
                    ]),
                    "access_mode": .string(state.access[chatId] ?? "ask"),
                ]), to: id)
        case "set_model", "set_thinking":
            reply(re, .object([:]), to: id)
        case "switch_agent":
            reply(re, .object(["chat_id": .string(chatId)]), to: id)
        case "inbox":
            reply(re, inbox(), to: id)
        case "search":
            let query = (request["query"]?.string ?? "").lowercased()
            let hits = chats.filter { ($0.summary.chat.title ?? "").lowercased().contains(query) }
            reply(
                re,
                .object([
                    "hits": .array(
                        hits.map {
                            .object([
                                "chat_id": .string($0.id), "title": .string($0.summary.chat.title ?? ""),
                                "snippet": .null, "at": .number(Double($0.summary.chat.updatedAt)),
                            ])
                        })
                ]), to: id)
        case "forget":
            if let device = sessions[id]?.device { devices[device] = nil }
            reply(re, .object([:]), to: id)
            sessions[id]?.link.closeFromServer()
        default:
            if !control(op, request, re: re, session: id) {
                refuse(re, "not_offered", "This computer's o7 does not offer that.", to: id)
            }
        }
    }

    /// Tells every session a chat changed, as a status update does.
    func announce(_ chatId: String) {
        if let chat = chats.first(where: { $0.id == chatId }) {
            status(chatId, chat.summary.chat.status, attention: chat.summary.chat.needsAttention)
        } else {
            refetchAll()
        }
    }

    /// A chat left the list: every session reads the overview again (a status for a chat it does
    /// not know makes it do so).
    private func refetchAll() {
        let event: JSONValue = .object([
            "ev": .string("chat"),
            "update": .object([
                "chat_id": .string("gone-\(Int.random(in: 0...1_000_000))"), "status": .string("stopped"),
                "needs_attention": .bool(false), "title": .null, "updated_at": .number(0),
            ]),
        ])
        for (id, session) in sessions where session.ready && session.device != nil { send(event, to: id) }
    }

    func overview() -> Overview {
        Overview(
            hostName: hostName, projects: Script.projects,
            chats: chats.map(\.summary).sorted { $0.chat.updatedAt > $1.chat.updatedAt },
            agents: Script.agents, defaultAgent: "claude-code")
    }

    private func inbox() -> JSONValue {
        var asking: [JSONValue] = []
        var working: [JSONValue] = []
        var recent: [JSONValue] = []
        for chat in chats {
            if let request = chat.pending {
                asking.append(
                    .object([
                        "chat_id": .string(chat.id),
                        "request": .object([
                            "id": .string(request.requestId), "kind": .string("permission"),
                            "title": .string(request.title),
                            "options": (try? JSONValue.of(request.options)) ?? .array([]),
                        ]),
                    ]))
            } else if chat.summary.chat.status.isWorking {
                working.append(
                    .object([
                        "chat_id": .string(chat.id), "line": .string("Running `swift test`"),
                        "since": .number(Double(Script.now - 240_000)),
                    ]))
            } else {
                let failed = chat.summary.chat.status == .failed
                var entry: [String: JSONValue] = [
                    "chat_id": .string(chat.id), "ended_at": .number(Double(chat.summary.chat.updatedAt)),
                    "duration_ms": .number(192_000),
                    "line": .string(failed ? "The agent stopped unexpectedly." : "The tests pass: 42 passed."),
                ]
                if !failed {
                    entry["files"] = .number(2)
                    entry["added"] = .number(9)
                    entry["removed"] = .number(1)
                }
                if chat.id == "c1" {
                    entry["pull_request"] = .object([
                        "number": .number(12), "state": .string("open"), "checks": .string("passed"),
                    ])
                }
                recent.append(.object(entry))
            }
        }
        return .object(["asking": .array(asking), "working": .array(working), "recent": .array(recent)])
    }

    func newChat(folder: String, agent: String?) -> WorkspaceChat {
        let id = "c\(chats.count + 1)"
        let agentId = agent ?? "claude-code"
        let name = Script.agents.first { $0.id == agentId }?.name ?? agentId
        let chat = WorkspaceChat(
            chat: ChatSummary(
                id: id, title: "New chat", agentId: agentId, agentName: name, folder: "/Users/you/code/o7",
                status: .ready, needsAttention: false, createdAt: Script.now, updatedAt: Script.now),
            projectId: folder == "f2" ? "p2" : "p1", folderId: folder)
        chats.insert(MockChat(summary: chat, events: []), at: 0)
        return chat
    }

    // MARK: The scripted agent

    private func index(_ chatId: String) -> Int? { chats.firstIndex { $0.id == chatId } }

    func status(_ chatId: String, _ status: SessionStatus, attention: Bool) {
        guard let at = index(chatId) else { return }
        chats[at].summary.chat.status = status
        chats[at].summary.chat.needsAttention = attention
        chats[at].summary.chat.updatedAt = Int64(Date().timeIntervalSince1970 * 1000)
        let update = chats[at].summary.chat
        let event: JSONValue = .object([
            "ev": .string("chat"),
            "update": .object([
                "chat_id": .string(chatId), "status": .string(update.status.rawValue),
                "needs_attention": .bool(attention), "title": update.title.map(JSONValue.string) ?? .null,
                "updated_at": .number(Double(update.updatedAt)),
            ]),
        ])
        for (id, session) in sessions where session.ready && session.device != nil { send(event, to: id) }
    }

    private func append(_ chatId: String, _ events: [O7Event]) {
        guard let at = index(chatId), !events.isEmpty else { return }
        let start = Int64(chats[at].events.count)
        let turn = chats[at].events.last?.turnId
        for (offset, event) in events.enumerated() {
            chats[at].events.append(
                HistoryItem(seq: start + Int64(offset), turnId: turn, at: Int64(Date().timeIntervalSince1970 * 1000), event: event))
        }
        let batch = EventBatch(chatId: chatId, seqStart: start, events: events)
        guard let json = try? JSONValue.of(batch) else { return }
        for (id, session) in sessions where session.following == chatId {
            send(.object(["ev": .string("frame"), "batch": json]), to: id)
        }
    }

    private func play(chat chatId: String, text: String) async {
        turns += 1
        let script = Script.turn(chat: chatId, number: turns, text: text)
        status(chatId, .running, attention: false)
        for event in script.before {
            try? await Task.sleep(for: pace)
            if case .permissionRequested(let request) = event, let at = index(chatId) {
                chats[at].pending = request
                append(chatId, [event])
                status(chatId, .running, attention: true)
            } else {
                append(chatId, [event])
            }
        }
    }

    private func resolve(chat chatId: String, option: String?) async {
        guard let at = index(chatId), let request = chats[at].pending else { return }
        chats[at].pending = nil
        let decision: PermissionDecision = option.map { .selected(optionId: $0, plan: nil) } ?? .cancelled
        append(chatId, [.permissionResolved(requestId: request.requestId, decision: decision, by: .user)])
        status(chatId, .running, attention: false)
        let turn = turns
        for event in Script.turn(chat: chatId, number: turn, text: "").after {
            try? await Task.sleep(for: pace)
            append(chatId, [event])
        }
        status(chatId, .ready, attention: false)
    }

    private func stop(chat chatId: String) async {
        guard let at = index(chatId) else { return }
        if let request = chats[at].pending {
            chats[at].pending = nil
            append(chatId, [.permissionResolved(requestId: request.requestId, decision: .cancelled, by: .turn)])
        }
        if let turn = chats[at].events.last?.turnId {
            append(chatId, [.turnEnded(turnId: turn, stopReason: .cancelled, usage: nil, durationMs: nil, recovered: false)])
        }
        status(chatId, .ready, attention: false)
    }

    static func json(_ value: JSONValue) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.withoutEscapingSlashes]
        return String(decoding: (try? encoder.encode(value)) ?? Data(), as: UTF8.self)
    }
}
