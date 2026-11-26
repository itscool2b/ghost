import CryptoKit
import Foundation
import GhostCrypto
import GhostPush
import GhostWire
import Testing

/// A folder of its own for each test, with one paired computer in `computers.json`.
private func place(computers: [(host: String, name: String, device: String)]) throws -> URL {
    let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ghost-push-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let records = computers.map {
        ["hostId": $0.host, "hostName": $0.name, "deviceId": $0.device, "site": "https://onghost.dev"]
    }
    try JSONSerialization.data(withJSONObject: records).write(to: folder.appendingPathComponent("computers.json"))
    return folder
}

private let deviceKey = Data((0..<32).map { UInt8($0) })

private func sealed(_ payload: PushPayload, host: String = "HOST", device: String = "DEVICE") throws -> [AnyHashable: Any] {
    let blob = try PushSeal.seal(
        try JSONEncoder().encode(payload), key: PushSeal.key(deviceKey: deviceKey), hostId: host, deviceId: device)
    return ["aps": ["alert": ["title": "Ghost"]], "ghost": ["v": 1, "d": device, "b": blob]]
}

private func opener(_ folder: URL) throws -> PushOpener {
    let keys = MemoryPushKeys()
    try keys.save(PushSeal.key(deviceKey: deviceKey), deviceId: "DEVICE")
    return PushOpener(folder: folder, keys: keys)
}

@Test func aSealedNotificationOpensIntoItsWords() throws {
    let folder = try place(computers: [("HOST", "Studio", "DEVICE")])
    let payload = PushPayload(
        id: "n1", at: 1_000, host: "Studio", kind: .asking, chat: "c1", title: "Fix the login form",
        project: "Ghost", agent: "Claude Code", line: "Run `pnpm test`?", askingCount: 2,
        request: .init(id: "r1", type: "permission", allow: "allow_once", deny: "reject"))
    let opened = try #require(try opener(folder).open(try sealed(payload)))
    #expect(opened.payload == payload)
    #expect(!opened.stale)
    let words = NotificationWords(opened)
    #expect(words.title == "Fix the login form")
    #expect(words.subtitle == "Ghost · Claude Code")
    #expect(words.body == "Run pnpm test?")
    #expect(words.category == PushCategory.permission)
    #expect(words.level == .timeSensitive)
    #expect(words.thread == "HOST:c1")
}

@Test func aNotificationForAnotherDeviceOrChangedOnTheWayDoesNotOpen() throws {
    let folder = try place(computers: [("HOST", "Studio", "DEVICE")])
    let payload = PushPayload(id: "n1", at: 1_000, host: "Studio", kind: .finished, chat: "c1")
    let open = try opener(folder)
    // Sealed for another computer: the bound computer differs.
    #expect(open.open(try sealed(payload, host: "OTHER")) == nil)
    // A device this phone does not have.
    #expect(open.open(try sealed(payload, device: "STRANGER")) == nil)
    // One byte changed.
    var info = try sealed(payload)
    var inner = info["ghost"] as! [String: Any]
    var blob = Array((inner["b"] as! String).utf8)
    blob[20] = blob[20] == UInt8(ascii: "A") ? UInt8(ascii: "B") : UInt8(ascii: "A")
    inner["b"] = String(decoding: blob, as: UTF8.self)
    info["ghost"] = inner
    #expect(open.open(info) == nil)
    // Not Ghost's at all.
    #expect(open.open(["aps": ["alert": "hi"]]) == nil)
}

@Test func aRepeatOrAnOlderNotificationComesInQuietly() throws {
    let folder = try place(computers: [("HOST", "Studio", "DEVICE")])
    let open = try opener(folder)
    let first = PushPayload(id: "n1", at: 2_000, host: "Studio", kind: .finished, chat: "c1", line: "Done.")
    #expect(open.open(try sealed(first))?.stale == false)
    let again = try #require(open.open(try sealed(first)))
    #expect(again.stale)
    #expect(NotificationWords(again).level == .passive)
    #expect(!NotificationWords(again).sound)
    let older = PushPayload(id: "n0", at: 1_000, host: "Studio", kind: .finished, chat: "c1")
    #expect(open.open(try sealed(older))?.stale == true)
}

@Test func allowIsOfferedOnlyForWhatThePersonCanRead() throws {
    let folder = try place(computers: [("HOST", "Studio", "DEVICE"), ("HOST2", "Laptop", "DEVICE2")])
    let open = try opener(folder)
    // A question in words gets a text field; a form gets none.
    let input = PushPayload(
        id: "q1", at: 1, host: "Studio", kind: .asking, chat: "c2", title: "Name it",
        request: .init(id: "r2", type: "input"))
    #expect(NotificationWords(try #require(open.open(try sealed(input)))).category == PushCategory.input)
    let form = PushPayload(
        id: "q2", at: 2, host: "Studio", kind: .asking, chat: "c2", request: .init(id: "r3", type: "form"))
    #expect(NotificationWords(try #require(open.open(try sealed(form)))).category == nil)
    // A finished turn has no buttons, and names its computer when there are two.
    let done = PushPayload(id: "d1", at: 3, host: "Studio", kind: .finished, chat: "c2", project: "Ghost")
    let words = NotificationWords(try #require(open.open(try sealed(done))))
    #expect(words.category == nil)
    #expect(words.subtitle == "Ghost · Studio")
    // Resolved elsewhere: quiet, no buttons.
    let resolved = PushPayload(id: "x1", at: 4, host: "Studio", kind: .resolved, chat: "c2")
    let quiet = NotificationWords(try #require(open.open(try sealed(resolved))))
    #expect(quiet.level == .passive)
    #expect(quiet.body == "Answered on the computer.")
}

@Test func theBadgeCountsWhatWaitsOnEveryComputer() throws {
    let folder = try place(computers: [])
    var snapshot = SharedSnapshot.load(folder: folder)
    snapshot.setAsking(2, host: "HOST", name: "Studio")
    snapshot.setAsking(1, host: "HOST2", name: "Laptop")
    snapshot.save(folder: folder)
    var again = SharedSnapshot.load(folder: folder)
    #expect(again.asking == 3)
    again.setAsking(0, host: "HOST", name: "Studio")
    #expect(again.asking == 1)
}

@Test func unknownKindsStillRead() throws {
    let json = #"{"v":1,"id":"z","at":5,"host":"Studio","kind":"something_new","line":"Hello"}"#
    let payload = try JSONDecoder().decode(PushPayload.self, from: Data(json.utf8))
    #expect(payload.kind == .unknown)
}
