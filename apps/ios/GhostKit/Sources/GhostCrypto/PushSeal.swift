public import CryptoKit
public import Foundation

/// What a notification carries, sealed by the computer for one phone (`push.rs`).
///
/// The computer seals each notification's details with a key only it and the phone know: HKDF-SHA256
/// over the device key, with no salt and the label `o7 push v1`. The relay and Apple pass on an
/// opaque blob, `base64url(nonce ‖ ciphertext ‖ tag)` with a random 12-byte nonce, bound to the
/// computer and the device it was sealed for. The phone's notification extension holds only this
/// derived key, never the device key itself.
public enum PushSeal {
    static let label = "o7 push v1"

    /// The key notifications are sealed with, from a device key.
    public static func key(deviceKey: Data) -> SymmetricKey {
        HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: deviceKey), salt: Data(), info: Data(label.utf8),
            outputByteCount: 32)
    }

    /// What the seal is bound to: the computer and the device.
    static func associated(hostId: String, deviceId: String) -> Data {
        Data("\(label)|\(hostId)|\(deviceId)".utf8)
    }

    /// The payload sealed in `blob`, or nil when it is not one sealed with `key` for this computer
    /// and device.
    public static func open(_ blob: String, key: SymmetricKey, hostId: String, deviceId: String) -> Data? {
        guard let bytes = Base64URL.decode(blob), bytes.count > 12 + 16,
            let box = try? AES.GCM.SealedBox(combined: bytes)
        else { return nil }
        return try? AES.GCM.open(box, using: key, authenticating: associated(hostId: hostId, deviceId: deviceId))
    }

    /// Seals `payload` as the computer does. The app never seals: this is for tests and tools.
    public static func seal(
        _ payload: Data, key: SymmetricKey, hostId: String, deviceId: String, nonce: Data? = nil
    ) throws -> String {
        let box = try AES.GCM.seal(
            payload, using: key, nonce: try nonce.map(AES.GCM.Nonce.init(data:)) ?? AES.GCM.Nonce(),
            authenticating: associated(hostId: hostId, deviceId: deviceId))
        guard let combined = box.combined else { throw ChannelError.malformed }
        return Base64URL.encode(combined)
    }
}
