public import CryptoKit
public import Foundation
public import O7Crypto
public import O7Wire

/// A notification the phone could open: its words, and the computer and device it came for.
public struct OpenedPush: Sendable, Hashable {
    public var payload: PushPayload
    public var hostId: String
    public var deviceId: String
    /// The computer's name as the phone knows it.
    public var hostName: String
    /// How many computers this phone is paired with: with more than one, a notification names its
    /// computer.
    public var computers: Int
    /// Seen before, or older than one already seen from that computer: it comes in quietly.
    public var stale: Bool
}

/// A paired computer as the notification extension reads it from the App Group: the app's
/// `computers.json`, without the keys it cannot read.
struct SharedRecord: Decodable {
    var hostId: String
    var hostName: String
    var deviceId: String
}

/// Opens what Apple delivers. It needs no network, no SwiftUI and no JavaScript: the
/// notification extension has a few megabytes and a few seconds.
public struct PushOpener: Sendable {
    let folder: URL
    let keys: any PushKeyStore
    let seen: SeenPushes

    public init(folder: URL = SharedPlace.folder(), keys: any PushKeyStore = KeychainPushKeys()) {
        self.folder = folder
        self.keys = keys
        seen = SeenPushes(file: folder.appendingPathComponent("seen.json"))
    }

    /// The opened notification, or nil when it is not one this phone can open (another install's,
    /// a computer forgotten since, a payload changed on the way, or a phone not yet unlocked since
    /// it started).
    public func open(_ userInfo: [AnyHashable: Any], remember: Bool = true) -> OpenedPush? {
        guard let envelope = PushEnvelope(userInfo: userInfo), envelope.v == 1 else { return nil }
        return open(envelope, remember: remember)
    }

    public func open(_ envelope: PushEnvelope, remember: Bool = true) -> OpenedPush? {
        let records = self.records()
        guard let record = records.first(where: { $0.deviceId == envelope.device }),
            let key = keys.load(deviceId: envelope.device),
            let plain = PushSeal.open(envelope.blob, key: key, hostId: record.hostId, deviceId: record.deviceId),
            let payload = try? JSONDecoder().decode(PushPayload.self, from: plain), payload.v == 1
        else { return nil }
        let stale = remember ? !seen.note(payload.id, at: payload.at, host: record.hostId) : false
        return OpenedPush(
            payload: payload, hostId: record.hostId, deviceId: record.deviceId, hostName: record.hostName,
            computers: records.count, stale: stale)
    }

    /// Why a notification did not open, in a word, for the extension's log. Never the payload.
    public func why(_ userInfo: [AnyHashable: Any]) -> String {
        guard let envelope = PushEnvelope(userInfo: userInfo) else { return "not o7's" }
        guard envelope.v == 1 else { return "version \(envelope.v)" }
        let records = self.records()
        guard let record = records.first(where: { $0.deviceId == envelope.device }) else {
            return "no computer for the device (\(records.count) known)"
        }
        guard let key = keys.load(deviceId: envelope.device) else { return "no key for the device" }
        guard PushSeal.open(envelope.blob, key: key, hostId: record.hostId, deviceId: record.deviceId) != nil else {
            return "the seal did not open"
        }
        return "the payload did not read"
    }

    /// The name of the computer that knows this phone as `device`.
    public func hostName(device: String) -> String? {
        records().first { $0.deviceId == device }?.hostName
    }

    func records() -> [SharedRecord] {
        guard let data = try? Data(contentsOf: folder.appendingPathComponent("computers.json")) else { return [] }
        return (try? JSONDecoder().decode([SharedRecord].self, from: data)) ?? []
    }
}

/// The notifications already shown, so a repeat or an older one than the newest from a computer
/// comes in quietly: the last 64 ids, and each computer's newest time.
final class SeenPushes: @unchecked Sendable {
    private struct State: Codable {
        var ids: [String] = []
        var newest: [String: Int64] = [:]
    }

    private let file: URL
    private let lock = NSLock()

    init(file: URL) {
        self.file = file
    }

    /// Notes a notification; false when it was seen before or is older than the newest seen from
    /// its computer.
    func note(_ id: String, at: Int64, host: String) -> Bool {
        lock.withLock {
            var state = read()
            let fresh = !state.ids.contains(id) && at >= (state.newest[host] ?? 0)
            if !state.ids.contains(id) { state.ids.append(id) }
            if state.ids.count > 64 { state.ids.removeFirst(state.ids.count - 64) }
            state.newest[host] = max(at, state.newest[host] ?? 0)
            write(state)
            return fresh
        }
    }

    private func read() -> State {
        guard let data = try? Data(contentsOf: file) else { return State() }
        return (try? JSONDecoder().decode(State.self, from: data)) ?? State()
    }

    private func write(_ state: State) {
        try? FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try? JSONEncoder().encode(state).write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
}
