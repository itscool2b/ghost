public import CryptoKit
public import Foundation

/// Why a sealed message did not open. Any of these ends the session.
public enum ChannelError: Error, Sendable, Equatable {
    case malformed
    case notOpened
    case notText
}

/// One session's two keys and message counters (`channel.ts`, `crypto.rs`).
///
/// Both sides hold one 32-byte secret: a pairing key or a device key. Each derives two
/// AES-256-GCM keys with HKDF-SHA256 over that secret, salted with both sides' nonces, one key per
/// direction. Every message's nonce is its number in its direction, so a message opens only as the
/// next one, from the other side, under the same credential: never replayed, reordered or echoed.
public struct SessionChannel: Sendable {
    public enum Side: Sendable { case client, host }

    static let toHost = "auracode remote v1 client to host"
    static let toClient = "auracode remote v1 host to client"
    static let prefix = "auracode remote v1|"

    private let sealKey: SymmetricKey
    private let openKey: SymmetricKey
    private let associated: Data
    private var sent: UInt64 = 0
    private var received: UInt64 = 0

    /// `context` names the credential: `pair|<id>` or `device|<id>`.
    public init(side: Side, secret: SymmetricKey, clientNonce: Data, hostNonce: Data, context: String) {
        let salt = clientNonce + hostNonce
        func key(_ label: String) -> SymmetricKey {
            HKDF<SHA256>.deriveKey(
                inputKeyMaterial: secret, salt: salt, info: Data(label.utf8), outputByteCount: 32)
        }
        let hostward = key(Self.toHost)
        let clientward = key(Self.toClient)
        sealKey = side == .client ? hostward : clientward
        openKey = side == .client ? clientward : hostward
        associated = Data((Self.prefix + context).utf8)
    }

    /// Seals the next message this side sends: base64url of the ciphertext and its 16-byte tag.
    public mutating func seal(_ plaintext: String) throws -> String {
        let box = try AES.GCM.seal(
            Data(plaintext.utf8), using: sealKey, nonce: Self.nonce(sent), authenticating: associated)
        sent += 1
        return Base64URL.encode(box.ciphertext + box.tag)
    }

    /// Opens the next message the other side sent. Anything else, in any other order, throws.
    public mutating func open(_ sealed: String) throws -> String {
        guard let bytes = Base64URL.decode(sealed), bytes.count >= 16 else {
            throw ChannelError.malformed
        }
        let plain: Data
        do {
            let box = try AES.GCM.SealedBox(
                nonce: Self.nonce(received), ciphertext: bytes.dropLast(16), tag: bytes.suffix(16))
            plain = try AES.GCM.open(box, using: openKey, authenticating: associated)
        } catch {
            throw ChannelError.notOpened
        }
        guard let text = String(data: plain, encoding: .utf8) else { throw ChannelError.notText }
        received += 1
        return text
    }

    /// The 96-bit nonce of a direction's `n`th message: four zero bytes, then `n` big-endian.
    static func nonce(_ n: UInt64) -> AES.GCM.Nonce {
        var bytes = Data(count: 4)
        withUnsafeBytes(of: n.bigEndian) { bytes.append(contentsOf: $0) }
        // Twelve bytes always make a valid nonce.
        return try! AES.GCM.Nonce(data: bytes)
    }
}

/// Fresh random bytes from the system's source.
public func randomBytes(_ count: Int) -> Data {
    var bytes = [UInt8](repeating: 0, count: count)
    var generator = SystemRandomNumberGenerator()
    for index in bytes.indices { bytes[index] = generator.next() }
    return Data(bytes)
}
