import CryptoKit
public import Foundation
public import O7Crypto
public import O7Wire

/// A paired device's credential for one computer.
public struct DeviceCredential: Sendable, Hashable, Codable {
    public let hostId: String
    public let deviceId: String
    /// The 32-byte device key. It lives in the Keychain; this copy only while connected.
    public let key: Data

    public init(hostId: String, deviceId: String, key: Data) {
        self.hostId = hostId
        self.deviceId = deviceId
        self.key = key
    }
}

/// What a session proves it holds: a pairing link's key, or a device's key.
public enum Credential: Sendable {
    case pair(PairingLink)
    case device(DeviceCredential)

    var hostId: String {
        switch self {
        case .pair(let link): link.hostId
        case .device(let device): device.hostId
        }
    }
}

public enum ConnectionPhase: Sendable, Equatable {
    case connecting
    /// The relay answers; the computer is off, asleep, or has o7 closed.
    case away
    case ready
    /// This pairing is over. `refused` when the computer turned the key away: the person decides
    /// whether to try again or forget it, since nothing proves who sent a refusal.
    case ended(reason: String, refused: Bool)
}

/// How far a session got, for a pairing screen that shows each step as it happens.
public enum SessionStep: Sendable, Equatable {
    /// The relay answered: the computer is there (or away).
    case relay
    /// The computer opened this side's hello and proved it holds the same key.
    case proved
}

/// What a connection tells the app, in order.
public enum HostEvent: Sendable {
    case phase(ConnectionPhase)
    case step(SessionStep)
    /// A session opened: the computer proved it holds the key.
    case ready(Ready)
    /// A pairing link became a device. Save it before anything else.
    case paired(DeviceCredential, features: [String])
    case chat(ChatStatusUpdate)
    case frame(EventBatch)
    case lost(chatId: String)
    case reset
}

/// Why a request failed, in words a person can act on.
public enum RemoteFailure: Error, Sendable, Equatable, LocalizedError {
    case refused(code: String, message: String)
    case timeout
    case unreachable
    case sessionRestarted
    /// The computer's o7 is older and does not offer this.
    case notOffered

    public var errorDescription: String? { message }

    public var message: String {
        switch self {
        case .refused(_, let message): message
        case .timeout: "The computer did not answer in time."
        case .unreachable: "The computer is not reachable right now."
        case .sessionRestarted: "The session started again."
        case .notOffered: "The o7 on that computer does not offer this yet. Update o7 there."
        }
    }

    public var code: String {
        switch self {
        case .refused(let code, _): code
        case .timeout: "timeout"
        case .unreachable: "unreachable"
        case .sessionRestarted: "restarted"
        case .notOffered: "not_offered"
        }
    }
}

/// How long things wait (`client.ts`). Tests make them short.
public struct Timing: Sendable {
    public var ping: Duration = .seconds(25)
    public var pong: Duration = .seconds(10)
    public var probe: Duration = .seconds(3)
    public var backoffCap: Duration = .seconds(15)
    public init() {}
}

/// How long a request may wait for its answer.
public enum OpTimeout: Sendable {
    /// Reads and settings.
    case standard
    /// A message, a stop or an answer acts on the computer: giving up early invites sending it twice.
    case act
    /// Starting a chat or handing it to another agent waits on an agent.
    case slow

    var duration: Duration {
        switch self {
        case .standard: .seconds(15)
        case .act: .seconds(30)
        case .slow: .seconds(45)
        }
    }
}

/// This device, as the computer's Settings will list it.
public struct ClientInfo: Sendable {
    public var name: String
    public var version: String
    public var model: String
    public init(name: String, version: String, model: String) {
        self.name = name
        self.version = version
        self.model = model
    }
}

/// One computer's session through the relay (`apps/web/app/remote/client.ts`, natively).
///
/// - The socket carries text frames: `ping`/`pong`, `h1`/`h0` (the computer is there or away),
///   `x` (the relay ends this socket), and `m` followed by a hello `H`, a welcome `W`, a refusal
///   `R` or a sealed message `E`.
/// - Every sealed message is sealed in this actor in the order it is sent, and one writer sends
///   them in that order, so their numbers always match.
/// - Before the computer accepts the socket the relay lets it send four small messages, the hello
///   included, so only one request goes out until a reply proves the computer accepted it.
public actor HostConnection {
    public nonisolated let events: AsyncStream<HostEvent>
    private let emit: AsyncStream<HostEvent>.Continuation

    public let hostId: String
    private var credential: Credential
    private let endpoint: RelayEndpoint
    private let transport: any WebSocketTransport
    private let client: ClientInfo
    private let timing: Timing

    private var link: (any WebSocketLink)?
    private var generation = 0
    private var outbox: AsyncStream<String>.Continuation?
    private var tasks: [Task<Void, Never>] = []

    private var channel: SessionChannel?
    private var nonce: Data?
    private var readySeen = false
    private var accepted = false
    private var held: [String] = []
    private var pending: [UInt32: CheckedContinuation<Data, any Error>] = [:]
    private var deadlines: [UInt32: Task<Void, Never>] = [:]
    private var nextId: UInt32 = 1

    private var retries = 0
    private var retry: Task<Void, Never>?
    private var pong: Task<Void, Never>?
    private var stopped = false
    private var ended = false
    private(set) public var phase: ConnectionPhase = .connecting
    private(set) public var features: [String] = []

    public init(
        credential: Credential, endpoint: RelayEndpoint, transport: any WebSocketTransport,
        client: ClientInfo, timing: Timing = Timing()
    ) {
        (events, emit) = AsyncStream.makeStream(bufferingPolicy: .unbounded)
        self.credential = credential
        self.hostId = credential.hostId
        self.endpoint = endpoint
        self.transport = transport
        self.client = client
        self.timing = timing
    }

    // MARK: Life

    public func start() {
        stopped = false
        connect()
    }

    public func stop() {
        stopped = true
        retry?.cancel()
        closeLink()
        sessionOver(.unreachable)
        emit.finish()
    }

    /// The app came back to the front, or the network came back: a socket that says it is open may
    /// be dead, so the relay is asked for a quick answer and a new session starts when none comes.
    public func wake() {
        guard !stopped, !ended else { return }
        if link == nil {
            retries = 0
            retry?.cancel()
            connect()
        } else {
            probe(within: timing.probe)
        }
    }

    // MARK: Requests

    /// Sends `op` and waits for its answer, typed as `R`.
    public func send<R: Decodable & Sendable>(
        _ op: RemoteOp, as type: R.Type, timeout: OpTimeout = .standard
    ) async throws -> R {
        if let feature = op.feature, !features.contains(feature) { throw RemoteFailure.notOffered }
        let data = try await request(op, timeout: timeout)
        do {
            return try JSONDecoder().decode(R.self, from: data)
        } catch {
            throw RemoteFailure.refused(
                code: "failed", message: "This computer's o7 answered unexpectedly.")
        }
    }

    /// Sends `op` and waits until it is done, ignoring what it returns.
    public func perform(_ op: RemoteOp, timeout: OpTimeout = .standard) async throws {
        _ = try await send(op, as: JSONValue.self, timeout: timeout)
    }

    private func request(_ op: RemoteOp, timeout: OpTimeout) async throws -> Data {
        guard var sealing = channel, readySeen, link != nil else { throw RemoteFailure.unreachable }
        let id = nextId
        nextId &+= 1
        let text = "mE" + (try sealing.seal(op.json(id: id)))
        channel = sealing
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            deadlines[id] = Task { [weak self, timing] in
                try? await Task.sleep(for: timeout.duration)
                guard !Task.isCancelled else { return }
                await self?.expire(id, probeWithin: timing.probe)
            }
            // Before the computer accepts this socket, one request at a time.
            if accepted || pending.count == 1 { outbox?.yield(text) } else { held.append(text) }
        }
    }

    private func expire(_ id: UInt32, probeWithin: Duration) {
        guard let continuation = pending.removeValue(forKey: id) else { return }
        deadlines.removeValue(forKey: id)
        continuation.resume(throwing: RemoteFailure.timeout)
        // An answer that never came may mean a dead socket: check it, and start over if so.
        probe(within: probeWithin)
    }

    private func settle(_ id: UInt32, _ result: Result<Data, RemoteRefusal>) {
        if !accepted {
            accepted = true
            for text in held { outbox?.yield(text) }
            held.removeAll()
        }
        deadlines.removeValue(forKey: id)?.cancel()
        guard let continuation = pending.removeValue(forKey: id) else { return }
        switch result {
        case .success(let data): continuation.resume(returning: data)
        case .failure(let refusal):
            continuation.resume(
                throwing: refusal.code == "not_offered"
                    ? RemoteFailure.notOffered
                    : RemoteFailure.refused(code: refusal.code, message: refusal.message))
        }
    }

    // MARK: The socket

    private func setPhase(_ next: ConnectionPhase) {
        guard phase != next else { return }
        phase = next
        emit.yield(.phase(next))
    }

    private func connect() {
        guard !stopped, !ended else { return }
        closeLink()
        if phase != .away { setPhase(.connecting) }
        generation += 1
        let current = generation
        let url = endpoint.url(hostId: hostId)
        let opening = Task { [weak self, transport] in
            do {
                let link = try await transport.open(url)
                await self?.opened(link, generation: current)
            } catch {
                await self?.linkClosed(generation: current)
            }
        }
        tasks.append(opening)
    }

    private func opened(_ opened: any WebSocketLink, generation current: Int) {
        guard current == generation, !stopped, !ended else {
            opened.close()
            return
        }
        link = opened
        let (stream, writer) = AsyncStream<String>.makeStream(bufferingPolicy: .unbounded)
        outbox = writer
        tasks.append(
            Task { [weak self] in
                do {
                    for await text in stream { try await opened.send(text) }
                } catch {
                    await self?.linkClosed(generation: current)
                }
            })
        tasks.append(
            Task { [weak self] in
                do {
                    while true {
                        let text = try await opened.receive()
                        await self?.received(text, generation: current)
                    }
                } catch {
                    await self?.linkClosed(generation: current)
                }
            })
        tasks.append(
            Task { [weak self, timing] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: timing.ping)
                    guard !Task.isCancelled else { return }
                    await self?.keepAlive(generation: current)
                }
            })
    }

    private func keepAlive(generation current: Int) {
        guard current == generation, link != nil, pong == nil else { return }
        probe(within: timing.pong)
    }

    /// Asks the relay to answer within `within`; a new session starts when it does not.
    private func probe(within: Duration) {
        guard link != nil else { return }
        pong?.cancel()
        let current = generation
        pong = Task { [weak self] in
            try? await Task.sleep(for: within)
            guard !Task.isCancelled else { return }
            await self?.unanswered(generation: current)
        }
        outbox?.yield("ping")
    }

    private func unanswered(generation current: Int) {
        guard current == generation else { return }
        pong = nil
        retries = 0
        connect()
    }

    private func closeLink() {
        for task in tasks { task.cancel() }
        tasks.removeAll()
        pong?.cancel()
        pong = nil
        outbox?.finish()
        outbox = nil
        link?.close()
        link = nil
    }

    private func linkClosed(generation current: Int) {
        guard current == generation else { return }
        closeLink()
        sessionOver(.unreachable)
        guard !ended, !stopped else { return }
        if phase == .ready { setPhase(.connecting) }
        // At once, then 1, 2, 4, 8, then every 15 seconds.
        let wait: Duration =
            retries == 0
            ? .zero : min(timing.backoffCap, .seconds(1 << min(retries - 1, 4)))
        retries += 1
        retry?.cancel()
        retry = Task { [weak self] in
            if wait > .zero { try? await Task.sleep(for: wait) }
            guard !Task.isCancelled else { return }
            await self?.reconnect()
        }
    }

    private func reconnect() { connect() }

    /// The sealed session is gone; whatever waited on it fails.
    private func sessionOver(_ failure: RemoteFailure) {
        channel = nil
        nonce = nil
        readySeen = false
        accepted = false
        held.removeAll()
        for task in deadlines.values { task.cancel() }
        deadlines.removeAll()
        let waiting = pending
        pending.removeAll()
        for continuation in waiting.values { continuation.resume(throwing: failure) }
    }

    private func end(_ reason: String, refused: Bool = false) {
        ended = true
        retry?.cancel()
        closeLink()
        sessionOver(.unreachable)
        setPhase(.ended(reason: reason, refused: refused))
    }

    // MARK: Messages

    private func received(_ text: String, generation current: Int) {
        guard current == generation else { return }
        switch text {
        case "pong":
            pong?.cancel()
            pong = nil
        case "h0":
            sessionOver(.unreachable)
            emit.yield(.step(.relay))
            setPhase(.away)
        case "h1":
            emit.yield(.step(.relay))
            hello()
        case "x":
            // The relay ended this socket; the close that follows connects again.
            break
        default:
            guard text.hasPrefix("m"), text.count >= 2 else { return }
            let kind = text.dropFirst().first
            let body = String(text.dropFirst(2))
            switch kind {
            case "R": refused(body)
            case "W": welcome(body)
            case "E": sealed(body)
            default: break
            }
        }
    }

    private func hello() {
        sessionOver(.sessionRestarted)
        let fresh = randomBytes(16)
        nonce = fresh
        let (kind, id) =
            switch credential {
            case .pair(let link): ("pair", link.pairingId)
            case .device(let device): ("device", device.deviceId)
            }
        let hello: JSONValue = .object([
            "v": .number(Double(protocolVersion)), "kind": .string(kind), "id": .string(id),
            "nonce": .string(Base64URL.encode(fresh)),
        ])
        guard let encoded = try? JSONEncoder().encode(hello) else { return }
        outbox?.yield("mH" + String(decoding: encoded, as: UTF8.self))
        if phase == .away { setPhase(.connecting) }
    }

    /// The computer's answer to this side's hello. Exactly one is accepted per hello: a second one,
    /// replayed by whoever carries the messages, would derive the same keys again and start their
    /// numbers over, which must never happen. So the nonce is spent the moment it is used.
    private func welcome(_ body: String) {
        let spent = nonce
        nonce = nil
        struct Welcome: Decodable {
            let v: Int?
            let nonce: String?
        }
        guard let spent, channel == nil,
            let welcome = try? JSONDecoder().decode(Welcome.self, from: Data(body.utf8)),
            welcome.v == protocolVersion,
            let hostNonce = welcome.nonce.flatMap({ Base64URL.decode($0, length: 16) })
        else {
            return protocolBroken()
        }
        let (secret, context) =
            switch credential {
            case .pair(let link): (link.key, "pair|\(link.pairingId)")
            case .device(let device): (device.key, "device|\(device.deviceId)")
            }
        channel = SessionChannel(
            side: .client, secret: SymmetricKey(data: secret), clientNonce: spent,
            hostNonce: hostNonce, context: context)
    }

    private func protocolBroken() {
        let current = generation
        linkClosed(generation: current)
    }

    private func refused(_ body: String) {
        struct Refusal: Decodable { let reason: String? }
        let reason = (try? JSONDecoder().decode(Refusal.self, from: Data(body.utf8)))?.reason
        switch (reason, credential) {
        case ("busy", _):
            // The relay closes the socket next; the retry backs off.
            return
        case ("version", _):
            end("This app and the o7 on that computer do not match. Update o7 there, or this app.")
        case ("elsewhere", .device):
            end(
                "That computer has another workspace open. Open the workspace this iPhone was paired with in o7, or pair again from the one that is open.",
                refused: true)
        case (_, .device):
            // Nothing proves who sent a refusal, so it deletes nothing: the person decides.
            end(
                "That computer no longer accepts this iPhone. If you removed it in o7's Settings, pair again from there.",
                refused: true)
        case (_, .pair):
            end(
                "This pairing code was already used or has expired. Make a new one in o7: Settings, Remote access, Pair a device.")
        }
    }

    private func sealed(_ body: String) {
        guard var opening = channel else { return }
        let text: String
        do {
            text = try opening.open(body)
            channel = opening
        } catch {
            // A message this session cannot open ends it; a new one starts on reconnect.
            return protocolBroken()
        }
        guard let message = try? HostMessage.parse(text) else { return protocolBroken() }
        // A session's first sealed message is the computer's "ready", once.
        if case .ready = message { guard !readySeen else { return protocolBroken() } } else {
            guard readySeen else { return protocolBroken() }
        }
        switch message {
        case .ready(let ready):
            readySeen = true
            features = ready.features
            emit.yield(.step(.proved))
            // Pairing waits on a reply that this same reader delivers: begin runs beside it.
            let current = generation
            Task { await self.begin(ready, generation: current) }
        case .reply(let re, let result):
            settle(re, result)
        case .chat(let update): emit.yield(.chat(update))
        case .frame(let batch): emit.yield(.frame(batch))
        case .lost(let chatId): emit.yield(.lost(chatId: chatId))
        case .reset: emit.yield(.reset)
        case .other: break
        }
    }

    /// The session is up: pair if this was a pairing link, then say the computer is ready.
    private func begin(_ ready: Ready, generation current: Int) async {
        guard current == generation else { return }
        retries = 0
        if case .pair(let link) = credential {
            let paired: Paired
            do {
                paired = try await send(
                    .pair(name: client.name, kind: "ios", version: client.version, model: client.model),
                    as: Paired.self)
            } catch let failure as RemoteFailure {
                // The computer refused to pair, in its own words (its devices are full, the link
                // closed): that ends this link, and reconnecting would only spend it again.
                if case .refused(_, let message) = failure { return end(message) }
                return protocolBroken()
            } catch {
                return protocolBroken()
            }
            guard let key = Base64URL.decode(paired.deviceKey, length: 32) else {
                return protocolBroken()
            }
            let device = DeviceCredential(hostId: link.hostId, deviceId: paired.deviceId, key: key)
            // The session carries on as the device; the next connection uses its own key.
            credential = .device(device)
            if let given = paired.features { features = given }
            emit.yield(.paired(device, features: features))
        }
        var current = ready
        current.features = features
        emit.yield(.ready(current))
        setPhase(.ready)
    }
}
