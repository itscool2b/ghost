public import CryptoKit
public import Foundation
import O7Crypto
import Security

/// Where the app and its notification extension meet: the App Group.
public enum SharedPlace {
    public static let group = "group.codes.o7.mobile"

    /// The App Group's folder for o7's files, or the app's own Application Support folder when
    /// there is no group (tests on a Mac).
    public static func folder(group: String = SharedPlace.group) -> URL {
        if let shared = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) {
            return shared.appendingPathComponent("o7", isDirectory: true)
        }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent("o7", isDirectory: true)
    }
}

/// The keys notifications open with, one per paired computer, by the device id the computer knows
/// the phone by. They live in the App Group's Keychain group, so the notification extension reads
/// them; the device keys themselves stay in the app's own Keychain, which the extension cannot read.
/// Each is derived from its device key (`PushSeal.key`), so it opens notifications and nothing else.
public protocol PushKeyStore: Sendable {
    func save(_ key: SymmetricKey, deviceId: String) throws
    func load(deviceId: String) -> SymmetricKey?
    func delete(deviceId: String)
}

public struct KeychainPushKeys: PushKeyStore {
    public let service: String
    public let accessGroup: String?

    public init(service: String = "codes.o7.mobile.push", accessGroup: String? = SharedPlace.group) {
        self.service = service
        self.accessGroup = accessGroup
    }

    private func query(_ deviceId: String) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: deviceId,
            kSecUseDataProtectionKeychain as String: true,
        ]
        if let accessGroup { query[kSecAttrAccessGroup as String] = accessGroup }
        return query
    }

    public func save(_ key: SymmetricKey, deviceId: String) throws {
        delete(deviceId: deviceId)
        var item = query(deviceId)
        item[kSecValueData as String] = key.withUnsafeBytes { Data($0) }
        // Readable on a locked phone once it was unlocked after starting: a notification arrives
        // while the phone sits in a pocket.
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        item[kSecAttrSynchronizable as String] = false
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw PushKeyError.keychain(status) }
    }

    public func load(deviceId: String) -> SymmetricKey? {
        var item = query(deviceId)
        item[kSecReturnData as String] = true
        item[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(item as CFDictionary, &result) == errSecSuccess, let data = result as? Data,
            data.count == 32
        else { return nil }
        return SymmetricKey(data: data)
    }

    public func delete(deviceId: String) {
        SecItemDelete(query(deviceId) as CFDictionary)
    }

    /// Every push key goes: a fresh install starts with none, though the Keychain keeps them.
    public func deleteAll() {
        var everything: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecUseDataProtectionKeychain as String: true,
        ]
        if let accessGroup { everything[kSecAttrAccessGroup as String] = accessGroup }
        SecItemDelete(everything as CFDictionary)
    }
}

public enum PushKeyError: Error, Sendable {
    case keychain(OSStatus)
}

/// Keys in memory, for tests and the app's mock mode.
public final class MemoryPushKeys: PushKeyStore, @unchecked Sendable {
    private let lock = NSLock()
    private var keys: [String: Data] = [:]

    public init() {}

    public func save(_ key: SymmetricKey, deviceId: String) throws {
        lock.withLock { keys[deviceId] = key.withUnsafeBytes { Data($0) } }
    }

    public func load(deviceId: String) -> SymmetricKey? {
        lock.withLock { keys[deviceId].map(SymmetricKey.init(data:)) }
    }

    public func delete(deviceId: String) {
        _ = lock.withLock { keys.removeValue(forKey: deviceId) }
    }
}
