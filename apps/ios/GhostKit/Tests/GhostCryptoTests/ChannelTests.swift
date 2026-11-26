import CryptoKit
import Foundation
import Testing

@testable import GhostCrypto

/// The vectors the desktop's half (`crypto.rs`) and the browser's half (`channel.ts`) are held to.
struct Vectors: Decodable {
    let secret: String
    let clientNonce: String
    let hostNonce: String
    let context: String
    let clientToHost: [[String]]
    let hostToClient: [[String]]

    enum CodingKeys: String, CodingKey {
        case secret, context
        case clientNonce = "client_nonce", hostNonce = "host_nonce"
        case clientToHost = "client_to_host", hostToClient = "host_to_client"
    }

    static func load() throws -> Vectors {
        // apps/ios/GhostKit/Tests/GhostCryptoTests/ChannelTests.swift -> the repository root.
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let file = root.appendingPathComponent("crates/ghost-remote/tests/vectors.json")
        return try JSONDecoder().decode(Vectors.self, from: Data(contentsOf: file))
    }
}

func channels(
    _ vectors: Vectors, context: String? = nil, secret: Data? = nil
) -> (client: SessionChannel, host: SessionChannel) {
    let key = SymmetricKey(data: secret ?? Base64URL.decode(vectors.secret, length: 32)!)
    let client = Base64URL.decode(vectors.clientNonce, length: 16)!
    let host = Base64URL.decode(vectors.hostNonce, length: 16)!
    let named = context ?? vectors.context
    return (
        SessionChannel(side: .client, secret: key, clientNonce: client, hostNonce: host, context: named),
        SessionChannel(side: .host, secret: key, clientNonce: client, hostNonce: host, context: named)
    )
}

@Test func sealsAndOpensExactlyWhatTheDesktopDoes() throws {
    let vectors = try Vectors.load()
    var (client, host) = channels(vectors)
    #expect(vectors.clientToHost.count >= 3 && vectors.hostToClient.count >= 3)
    for pair in vectors.clientToHost {
        #expect(try client.seal(pair[0]) == pair[1])
        #expect(try host.open(pair[1]) == pair[0])
    }
    for pair in vectors.hostToClient {
        #expect(try host.seal(pair[0]) == pair[1])
        #expect(try client.open(pair[1]) == pair[0])
    }
}

@Test func opensOnlyTheNextMessageSealedForTheSameCredential() throws {
    let vectors = try Vectors.load()
    var (client, host) = channels(vectors)
    let first = try client.seal("one")
    let second = try client.seal("two")
    // Out of order, echoed back, changed or not base64: refused, and nothing is lost.
    #expect(throws: ChannelError.self) { try host.open(second) }
    #expect(throws: ChannelError.self) { try client.open(first) }
    var changed = Base64URL.decode(first)!
    changed[0] ^= 1
    #expect(throws: ChannelError.self) { try host.open(Base64URL.encode(changed)) }
    #expect(throws: ChannelError.self) { try host.open("not base64!") }
    #expect(try host.open(first) == "one")
    // Replayed.
    #expect(throws: ChannelError.self) { try host.open(first) }
    #expect(try host.open(second) == "two")

    var fresh = channels(vectors).client
    let sealed = try fresh.seal("for this device")
    var other = channels(vectors, context: "device|another").host
    #expect(throws: ChannelError.self) { try other.open(sealed) }
    var wrongKey = channels(vectors, secret: Data(count: 32)).host
    #expect(throws: ChannelError.self) { try wrongKey.open(sealed) }
    var right = channels(vectors).host
    #expect(try right.open(sealed) == "for this device")
}

@Test func aMessageThatIsNotTextNeverOpens() throws {
    let vectors = try Vectors.load()
    let key = SymmetricKey(data: Base64URL.decode(vectors.secret, length: 32)!)
    let nonce = SessionChannel.nonce(0)
    let box = try AES.GCM.seal(
        Data([0xFF, 0xFE]), using: HKDF<SHA256>.deriveKey(
            inputKeyMaterial: key,
            salt: Base64URL.decode(vectors.clientNonce)! + Base64URL.decode(vectors.hostNonce)!,
            info: Data(SessionChannel.toClient.utf8), outputByteCount: 32),
        nonce: nonce, authenticating: Data((SessionChannel.prefix + vectors.context).utf8))
    var client = channels(vectors).client
    #expect(throws: ChannelError.notText) {
        try client.open(Base64URL.encode(box.ciphertext + box.tag))
    }
}

@Test func idsAndKeysDecodeOnlyInTheirOneCanonicalSpelling() {
    let key = randomBytes(32)
    let text = Base64URL.encode(key)
    #expect(text.count == 43)
    #expect(Base64URL.decode(text, length: 32) == key)
    #expect(Base64URL.decode(text, length: 16) == nil)
    #expect(Base64URL.decode(text + "=") == nil)
    #expect(Base64URL.decode("a+b/") == nil)
    #expect(Base64URL.decode("A") == nil)
    #expect(Base64URL.decode("AA") == Data([0]))
    #expect(Base64URL.decode("AB") == nil)
    #expect(Base64URL.decode("") == Data())
}

@Test func pairingLinksAreReadOnlyFromGhostAndWhole() throws {
    let host = Base64URL.encode(Data(repeating: 1, count: 16))
    let pairing = Base64URL.encode(Data(repeating: 2, count: 8))
    let key = Base64URL.encode(Data(repeating: 3, count: 32))
    let link = try #require(PairingLink.parse(" https://onghost.dev/remote#\(host).\(pairing).\(key)\n"))
    #expect(link.site.absoluteString == "https://onghost.dev")
    #expect(link.hostId == host && link.pairingId == pairing)
    #expect(link.key == Data(repeating: 3, count: 32))
    // Another site, another path, a missing part, a wrong length, credentials in the address.
    #expect(PairingLink.parse("https://evil.example/remote#\(host).\(pairing).\(key)") == nil)
    #expect(PairingLink.parse("https://onghost.dev/elsewhere#\(host).\(pairing).\(key)") == nil)
    #expect(PairingLink.parse("https://onghost.dev/remote#\(host).\(pairing)") == nil)
    #expect(PairingLink.parse("https://onghost.dev/remote#\(host).\(pairing).\(key).x") == nil)
    #expect(PairingLink.parse("https://onghost.dev/remote#\(host).\(pairing).\(key)A") == nil)
    #expect(PairingLink.parse("https://me@onghost.dev/remote#\(host).\(pairing).\(key)") == nil)
    #expect(PairingLink.parse("http://onghost.dev/remote#\(host).\(pairing).\(key)") == nil)
    // Development builds also pair with a computer on this machine, on any port.
    let local = "http://localhost:1437/remote#\(host).\(pairing).\(key)"
    #expect(PairingLink.parse(local) == nil)
    let developed = try #require(PairingLink.parse(local, sites: PairingLink.developmentSites))
    #expect(developed.site.absoluteString == "http://localhost:1437")
}
