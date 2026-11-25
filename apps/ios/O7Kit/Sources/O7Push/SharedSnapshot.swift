public import Foundation

/// What the widgets, the Live Activity and the badge show, written to the App Group by the app
/// while it runs and by the notification extension when a notification says a count changed. It
/// holds counts and the few chats a widget lists, never what a chat says.
public struct SharedSnapshot: Codable, Sendable, Hashable {
    public struct Chat: Codable, Sendable, Hashable, Identifiable {
        public var id: String
        public var title: String
        public var agent: String
        /// `asking`, `working`, `done` or `failed`.
        public var state: String
        public var updatedAt: Int64

        public init(id: String, title: String, agent: String, state: String, updatedAt: Int64) {
            self.id = id
            self.title = title
            self.agent = agent
            self.state = state
            self.updatedAt = updatedAt
        }
    }

    public struct Computer: Codable, Sendable, Hashable, Identifiable {
        public var hostId: String
        public var hostName: String
        public var asking: Int
        public var working: Int
        /// Up to three, what needs the person first.
        public var chats: [Chat]
        /// The computer's theme, for the widgets: `claude`, `codex`, ...
        public var theme: String?
        public var dark: Bool?
        public var id: String { hostId }

        public init(
            hostId: String, hostName: String, asking: Int, working: Int, chats: [Chat], theme: String? = nil,
            dark: Bool? = nil
        ) {
            self.hostId = hostId
            self.hostName = hostName
            self.asking = asking
            self.working = working
            self.chats = chats
            self.theme = theme
            self.dark = dark
        }
    }

    public var computers: [Computer]
    public var updatedAt: Date

    public init(computers: [Computer] = [], updatedAt: Date = .now) {
        self.computers = computers
        self.updatedAt = updatedAt
    }

    /// What needs the person, on every computer: the app's badge.
    public var asking: Int { computers.reduce(0) { $0 + $1.asking } }
    public var working: Int { computers.reduce(0) { $0 + $1.working } }

    static func file(_ folder: URL) -> URL { folder.appendingPathComponent("snapshot.json") }

    public static func load(folder: URL = SharedPlace.folder()) -> SharedSnapshot {
        guard let data = try? Data(contentsOf: file(folder)),
            let snapshot = try? JSONDecoder().decode(SharedSnapshot.self, from: data)
        else { return SharedSnapshot() }
        return snapshot
    }

    public func save(folder: URL = SharedPlace.folder()) {
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try? JSONEncoder().encode(self).write(
            to: Self.file(folder), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    /// A notification said how many chats on a computer wait now.
    public mutating func setAsking(_ count: Int, host: String, name: String) {
        if let at = computers.firstIndex(where: { $0.hostId == host }) {
            computers[at].asking = max(0, count)
        } else {
            computers.append(Computer(hostId: host, hostName: name, asking: max(0, count), working: 0, chats: []))
        }
        updatedAt = .now
    }
}
