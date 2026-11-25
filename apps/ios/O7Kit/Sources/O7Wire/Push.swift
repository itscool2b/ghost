public import Foundation

/// What a sealed notification says once the phone opens it (`push.rs` `Payload`). The relay only
/// ever sees it sealed; Apple shows the fixed "o7" and "Something changed on your computer." until
/// the phone's notification extension opens it and writes these words instead.
public struct PushPayload: Codable, Sendable, Hashable {
    /// Why the computer sent it.
    public enum Kind: String, Codable, Sendable, Hashable {
        /// A request waits: a permission or a question.
        case asking
        case finished
        case failed
        /// A task's review finished.
        case reviewed
        /// The chat's pull request changed: checks, a review, merged.
        case pullRequest = "pull_request"
        /// Work queued for tonight could not start.
        case queuedFailed = "queued_failed"
        /// What was asked was answered elsewhere: the notification goes quietly.
        case resolved
        /// From the computer's Settings, to see that notifications arrive.
        case test
        case unknown

        public init(from decoder: any Decoder) throws {
            self = Kind(rawValue: try decoder.singleValueContainer().decode(String.self)) ?? .unknown
        }
    }

    /// A request the notification can answer: Allow and Deny on the lock screen.
    public struct Request: Codable, Sendable, Hashable {
        public var id: String
        /// `permission`, or a question's kind (`confirm`, `select`, `input`, `form`).
        public var type: String
        /// The option Allow picks: only ever one that allows once.
        public var allow: String?
        /// The option Deny picks.
        public var deny: String?

        public init(id: String, type: String, allow: String? = nil, deny: String? = nil) {
            self.id = id
            self.type = type
            self.allow = allow
            self.deny = deny
        }
    }

    /// The payload's version: 1.
    public var v: Int
    /// Unique per notification: a repeat is dropped.
    public var id: String
    /// When the computer sent it, in milliseconds: an older one than the last seen is dropped.
    public var at: Int64
    /// The computer's name, as its o7 calls it.
    public var host: String
    public var kind: Kind
    public var chat: String?
    public var title: String?
    public var project: String?
    public var agent: String?
    /// The ask, or the last line of what happened.
    public var line: String?
    /// How many chats on that computer wait on the person.
    public var askingCount: Int?
    public var request: Request?
    /// For a review: the reviewer's chat.
    public var review: String?
    /// For queued work that could not start: its id.
    public var queued: String?

    enum CodingKeys: String, CodingKey {
        case v, id, at, host, kind, chat, title, project, agent, line, request, review, queued
        case askingCount = "asking_count"
    }

    public init(
        v: Int = 1, id: String, at: Int64, host: String, kind: Kind, chat: String? = nil, title: String? = nil,
        project: String? = nil, agent: String? = nil, line: String? = nil, askingCount: Int? = nil,
        request: Request? = nil, review: String? = nil, queued: String? = nil
    ) {
        self.v = v
        self.id = id
        self.at = at
        self.host = host
        self.kind = kind
        self.chat = chat
        self.title = title
        self.project = project
        self.agent = agent
        self.line = line
        self.askingCount = askingCount
        self.request = request
        self.review = review
        self.queued = queued
    }
}

/// The outside of a notification as Apple delivers it: the fixed words and o7's sealed part.
public struct PushEnvelope: Sendable, Hashable {
    /// The payload version.
    public var v: Int
    /// The device the computer sealed it for.
    public var device: String
    /// The sealed payload.
    public var blob: String

    /// Reads `userInfo["o7"]` as the relay writes it: `{"v":1,"d":"<device id>","b":"<blob>"}`.
    public init?(userInfo: [AnyHashable: Any]) {
        guard let o7 = userInfo["o7"] as? [String: Any], let device = o7["d"] as? String,
            let blob = o7["b"] as? String
        else { return nil }
        v = (o7["v"] as? Int) ?? 1
        self.device = device
        self.blob = blob
    }

    public init(v: Int = 1, device: String, blob: String) {
        self.v = v
        self.device = device
        self.blob = blob
    }
}

/// Which notifications a phone wants from a computer (`push_register`'s `prefs`).
public struct PushPrefs: Codable, Sendable, Hashable {
    /// A chat waits on the person.
    public var asking: Bool
    public var finished: Bool
    public var failed: Bool
    /// A task's review is in, or queued work could not start.
    public var tasks: Bool
    /// `all` chats, or only `mine`: those started or answered from this phone.
    public var scope: String

    public init(asking: Bool = true, finished: Bool = true, failed: Bool = true, tasks: Bool = true, scope: String = "all") {
        self.asking = asking
        self.finished = finished
        self.failed = failed
        self.tasks = tasks
        self.scope = scope
    }

    public static let all = PushPrefs()
}

/// What `push_register` says: whether the relay in front of the computer sends notifications.
public struct PushRegistered: Codable, Sendable, Hashable {
    public var relay: Bool
}

extension RemoteOp {
    /// Where Apple delivers this phone's notifications, for this computer to send to. `env` is
    /// `sandbox` for development builds, `production` for the App Store's.
    public static func pushRegister(token: String, env: String, prefs: PushPrefs) -> RemoteOp {
        RemoteOp(
            "push_register",
            [
                "token": .string(token), "env": .string(env),
                "prefs": .object([
                    "asking": .bool(prefs.asking), "finished": .bool(prefs.finished), "failed": .bool(prefs.failed),
                    "tasks": .bool(prefs.tasks), "scope": .string(prefs.scope),
                ]),
            ], feature: "push")
    }
    public static let pushUnregister = RemoteOp("push_unregister", feature: "push")
    public static let pushTest = RemoteOp("push_test", feature: "push")
    /// The token Apple gave this computer's Live Activity, for its updates.
    public static func activity(token: String, env: String) -> RemoteOp {
        RemoteOp("activity", ["token": .string(token), "env": .string(env)], feature: "activity")
    }
}
