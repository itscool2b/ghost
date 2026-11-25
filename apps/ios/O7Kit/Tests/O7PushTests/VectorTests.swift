import CryptoKit
import Foundation
import O7Crypto
import Testing

/// The computer's own seals (`crates/o7-remote/tests/push-vectors.json`, written by `push.rs`'s
/// tests): the phone derives the same key and opens every one to the same words, and seals them
/// back to the same bytes with the same nonce.
struct PushVectors: Decodable {
    struct Vector: Decodable {
        var deviceKey: String
        var pushKey: String
        var hostId: String
        var deviceId: String
        var nonce: String
        var plaintext: String
        var blob: String
        var chat: String?
        var collapse: String?

        enum CodingKeys: String, CodingKey {
            case nonce, plaintext, blob, chat, collapse
            case deviceKey = "device_key", pushKey = "push_key", hostId = "host_id", deviceId = "device_id"
        }
    }

    var vectors: [Vector]

    static func load() throws -> PushVectors? {
        // apps/ios/O7Kit/Tests/O7PushTests/VectorTests.swift -> the repository root.
        var root = URL(fileURLWithPath: #filePath)
        for _ in 0..<6 { root.deleteLastPathComponent() }
        let file = root.appendingPathComponent("crates/o7-remote/tests/push-vectors.json")
        guard FileManager.default.fileExists(atPath: file.path) else { return nil }
        return try JSONDecoder().decode(PushVectors.self, from: Data(contentsOf: file))
    }
}

private func hex(_ text: String) -> Data {
    var bytes: [UInt8] = []
    var index = text.startIndex
    while index < text.endIndex {
        let next = text.index(index, offsetBy: 2)
        bytes.append(UInt8(text[index..<next], radix: 16)!)
        index = next
    }
    return Data(bytes)
}

@Test func opensWhatTheComputerSealed() throws {
    let file = try #require(try PushVectors.load(), "push-vectors.json is written by the computer's tests")
    #expect(!file.vectors.isEmpty)
    for vector in file.vectors {
        let key = PushSeal.key(deviceKey: hex(vector.deviceKey))
        #expect(key.withUnsafeBytes { Data($0) } == hex(vector.pushKey))
        let opened = try #require(
            PushSeal.open(vector.blob, key: key, hostId: vector.hostId, deviceId: vector.deviceId))
        #expect(String(decoding: opened, as: UTF8.self) == vector.plaintext)
        // Bound to the computer and the device it was sealed for.
        #expect(PushSeal.open(vector.blob, key: key, hostId: vector.hostId + "x", deviceId: vector.deviceId) == nil)
        #expect(PushSeal.open(vector.blob, key: key, hostId: vector.hostId, deviceId: vector.deviceId + "x") == nil)
        // The same nonce seals to the same bytes.
        let again = try PushSeal.seal(
            Data(vector.plaintext.utf8), key: key, hostId: vector.hostId, deviceId: vector.deviceId,
            nonce: hex(vector.nonce))
        #expect(again == vector.blob)
        if let chat = vector.chat, let collapse = vector.collapse {
            let mac = HMAC<SHA256>.authenticationCode(
                for: Data("o7 push collapse v1|\(chat)".utf8), using: SymmetricKey(data: hex(vector.deviceKey)))
            #expect(Base64URL.encode(Data(mac).prefix(12)) == collapse)
        }
    }
}
