public import Foundation

/// Unpadded base64url, read only in its one canonical spelling (`channel.ts` `decode`,
/// `crypto.rs` `decode`): any other spelling of the same bytes is refused, so a message can never
/// be changed without its seal failing.
public enum Base64URL {
    public static func encode(_ bytes: some DataProtocol) -> String {
        Data(bytes).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    /// The bytes `text` spells, or nil. With `length`, exactly that many bytes.
    public static func decode(_ text: String, length: Int? = nil) -> Data? {
        let allowed = text.utf8.allSatisfy { byte in
            (byte >= 0x41 && byte <= 0x5A) || (byte >= 0x61 && byte <= 0x7A)
                || (byte >= 0x30 && byte <= 0x39) || byte == 0x2D || byte == 0x5F
        }
        guard allowed, text.utf8.count % 4 != 1 else { return nil }
        var standard = text.replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        standard += String(repeating: "=", count: (4 - standard.utf8.count % 4) % 4)
        guard let bytes = Data(base64Encoded: standard), encode(bytes) == text else { return nil }
        if let length, bytes.count != length { return nil }
        return bytes
    }
}
