public import Foundation
import Security

/// Where device keys are kept. On the phone that is the Keychain; tests and the app's mock mode
/// keep them in memory.
public protocol SecretStore: Sendable {
    func save(_ secret: Data, account: String) throws
    func load(account: String) -> Data?
    func delete(account: String)
    /// Every account this store holds.
    func accounts() -> [String]
}

public enum SecretError: Error, Sendable {
    case keychain(OSStatus)
}

/// Device keys in the Keychain: this device only, never synced or backed up to another device,
/// and readable once the phone was unlocked after it started (so a notification can be opened on
/// a locked phone later).
public struct KeychainSecretStore: SecretStore {
    public let service: String
    public let accessGroup: String?

    public init(service: String = "codes.o7.mobile.device", accessGroup: String? = nil) {
        self.service = service
        self.accessGroup = accessGroup
    }

    private func query(_ account: String? = nil) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecUseDataProtectionKeychain as String: true,
        ]
        if let account { query[kSecAttrAccount as String] = account }
        if let accessGroup { query[kSecAttrAccessGroup as String] = accessGroup }
        return query
    }

    public func save(_ secret: Data, account: String) throws {
        delete(account: account)
        var item = query(account)
        item[kSecValueData as String] = secret
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        item[kSecAttrSynchronizable as String] = false
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw SecretError.keychain(status) }
    }

    public func load(account: String) -> Data? {
        var item = query(account)
        item[kSecReturnData as String] = true
        item[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(item as CFDictionary, &result) == errSecSuccess else { return nil }
        return result as? Data
    }

    public func delete(account: String) {
        SecItemDelete(query(account) as CFDictionary)
    }

    public func accounts() -> [String] {
        var item = query()
        item[kSecReturnAttributes as String] = true
        item[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: AnyObject?
        guard SecItemCopyMatching(item as CFDictionary, &result) == errSecSuccess,
            let found = result as? [[String: Any]]
        else { return [] }
        return found.compactMap { $0[kSecAttrAccount as String] as? String }
    }

    /// Everything this store holds goes: a fresh install starts with nothing, even though the
    /// Keychain keeps items across a reinstall.
    public func deleteAll() {
        SecItemDelete(query() as CFDictionary)
    }
}

/// Secrets in memory, for tests and the app's mock mode.
public final class MemorySecretStore: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var secrets: [String: Data] = [:]

    public init() {}

    public func save(_ secret: Data, account: String) throws {
        lock.withLock { secrets[account] = secret }
    }

    public func load(account: String) -> Data? {
        lock.withLock { secrets[account] }
    }

    public func delete(account: String) {
        _ = lock.withLock { secrets.removeValue(forKey: account) }
    }

    public func accounts() -> [String] {
        lock.withLock { Array(secrets.keys) }
    }
}
