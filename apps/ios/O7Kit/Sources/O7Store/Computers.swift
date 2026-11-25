public import Foundation
public import O7Client
public import O7Wire

/// A computer this phone is paired with, as the app lists it. Its key is not here: it is in the
/// Keychain, under the computer's id.
public struct ComputerRecord: Codable, Hashable, Sendable, Identifiable {
    public var hostId: String
    public var hostName: String
    public var deviceId: String
    /// The site it was paired through, such as `https://o7.codes`: its relay is there.
    public var site: String
    public var pairedAt: Date
    /// How the computer looks, as it last said, so the app wears it before it answers.
    public var look: Look?
    /// What the computer offered when the app last talked to it.
    public var features: [String]
    /// The order the person put the computers in, or when it was last opened.
    public var lastOpened: Date

    public var id: String { hostId }

    public init(
        hostId: String, hostName: String, deviceId: String, site: String, pairedAt: Date = .now,
        look: Look? = nil, features: [String] = [], lastOpened: Date = .now
    ) {
        self.hostId = hostId
        self.hostName = hostName
        self.deviceId = deviceId
        self.site = site
        self.pairedAt = pairedAt
        self.look = look
        self.features = features
        self.lastOpened = lastOpened
    }
}

/// The paired computers and their keys, kept together: a record is saved only with its key, and
/// forgetting a computer removes both.
public final class Computers: @unchecked Sendable {
    private let file: URL
    private let secrets: any SecretStore
    private let lock = NSLock()

    /// `folder` holds the list: the App Group's container on the phone, so the notification
    /// extension and the widgets read the same names.
    public init(folder: URL, secrets: any SecretStore) {
        file = folder.appendingPathComponent("computers.json")
        self.secrets = secrets
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    }

    /// The App Group's folder when the app has one, else its own Application Support folder
    /// (a simulator build without a team has no App Group).
    public static func defaultFolder(group: String = "group.codes.o7.mobile") -> URL {
        if let shared = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group) {
            return shared.appendingPathComponent("o7", isDirectory: true)
        }
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        return support.appendingPathComponent("o7", isDirectory: true)
    }

    /// Every paired computer whose key is still here, most recently opened first.
    public func all() -> [ComputerRecord] {
        lock.withLock {
            read().filter { secrets.load(account: $0.hostId) != nil }
                .sorted { $0.lastOpened > $1.lastOpened }
        }
    }

    public func credential(for hostId: String) -> DeviceCredential? {
        lock.withLock {
            guard let record = read().first(where: { $0.hostId == hostId }),
                let key = secrets.load(account: hostId), key.count == 32
            else { return nil }
            return DeviceCredential(hostId: hostId, deviceId: record.deviceId, key: key)
        }
    }

    /// Saves a new pairing. Pairing a computer again replaces its record and its key: the device
    /// before is returned so the app can ask the computer to forget it.
    @discardableResult
    public func paired(
        _ device: DeviceCredential, hostName: String, site: String, look: Look?, features: [String]
    ) throws -> DeviceCredential? {
        try lock.withLock {
            var records = read()
            let before = records.first { $0.hostId == device.hostId }
            let replaced = before.flatMap { record in
                secrets.load(account: device.hostId).map {
                    DeviceCredential(hostId: device.hostId, deviceId: record.deviceId, key: $0)
                }
            }
            try secrets.save(device.key, account: device.hostId)
            records.removeAll { $0.hostId == device.hostId }
            records.append(
                ComputerRecord(
                    hostId: device.hostId, hostName: hostName, deviceId: device.deviceId, site: site,
                    look: look, features: features))
            try write(records)
            return replaced?.deviceId == device.deviceId ? nil : replaced
        }
    }

    /// Updates what the computer said about itself: its name, look and features.
    public func update(_ hostId: String, _ change: (inout ComputerRecord) -> Void) {
        lock.withLock {
            var records = read()
            guard let at = records.firstIndex(where: { $0.hostId == hostId }) else { return }
            change(&records[at])
            try? write(records)
        }
    }

    public func forget(_ hostId: String) {
        lock.withLock {
            secrets.delete(account: hostId)
            var records = read()
            records.removeAll { $0.hostId == hostId }
            try? write(records)
        }
    }

    private func read() -> [ComputerRecord] {
        guard let data = try? Data(contentsOf: file) else { return [] }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .millisecondsSince1970
        return (try? decoder.decode([ComputerRecord].self, from: data)) ?? []
    }

    private func write(_ records: [ComputerRecord]) throws {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        try encoder.encode(records).write(to: file, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }
}
