public import Foundation
public import O7Wire
#if canImport(UserNotifications)
public import UserNotifications
#endif

/// The categories the app registers: what a notification's buttons are.
public enum PushCategory {
    /// A permission: Allow once and Deny, both behind the phone's own authentication.
    public static let permission = "o7.permission"
    /// A yes-or-no question.
    public static let confirm = "o7.question.confirm"
    /// A question answered in words.
    public static let input = "o7.question.input"

    public enum Action {
        public static let allow = "o7.allow"
        public static let deny = "o7.deny"
        public static let yes = "o7.yes"
        public static let no = "o7.no"
        public static let answer = "o7.answer"
    }
}

/// How a notification reads and behaves, worked out from what the computer sealed.
public struct NotificationWords: Sendable, Hashable {
    public enum Level: Sendable, Hashable {
        /// Breaks through a Focus that allows time-sensitive notifications: an agent waits.
        case timeSensitive
        case active
        /// Into Notification Center without a sound or a banner.
        case passive
    }

    public var title: String
    public var subtitle: String
    public var body: String
    /// Set only for a notification that opened: its buttons act on what the person can read.
    public var category: String?
    public var level: Level
    /// Its place in the summary: needs-you first.
    public var relevance: Double
    public var sound: Bool
    /// One thread per chat: `<computer>:<chat>`.
    public var thread: String

    public init(_ opened: OpenedPush) {
        let payload = opened.payload
        title = Self.clip(payload.title?.nonEmpty ?? opened.hostName.nonEmpty ?? "o7", 120)
        var context = [payload.project, payload.agent].compactMap { $0?.nonEmpty }
        if opened.computers > 1 { context.append(opened.hostName) }
        subtitle = context.joined(separator: " · ")
        let line = payload.line?.nonEmpty
        switch payload.kind {
        case .asking:
            body = line ?? "Waiting on you."
            level = .timeSensitive
            relevance = 1
        case .finished:
            body = line ?? "Done."
            level = .active
            relevance = 0.5
        case .failed:
            body = line ?? "It stopped with an error."
            level = .active
            relevance = 0.8
        case .reviewed:
            body = line ?? "The review is in."
            level = .active
            relevance = 0.6
        case .pullRequest:
            body = line ?? "The pull request changed."
            level = .active
            relevance = 0.6
        case .queuedFailed:
            body = line.map { "Could not start: \($0)" } ?? "Work queued for tonight could not start."
            level = .active
            relevance = 0.7
        case .resolved:
            body = line ?? "Answered on the computer."
            level = .passive
            relevance = 0
        case .test:
            body = line ?? "Notifications from \(opened.hostName) arrive here."
            level = .active
            relevance = 0.1
        case .unknown:
            body = line ?? "Something changed on your computer."
            level = .active
            relevance = 0.3
        }
        body = Self.clip(Self.plain(body), 400)
        title = Self.plain(title)
        sound = level != .passive && !opened.stale
        if opened.stale { level = .passive }
        thread = "\(opened.hostId):\(payload.chat ?? "computer")"
        category = Self.category(payload)
    }

    /// The buttons a request gets: Allow only when the computer named an option that allows once.
    static func category(_ payload: PushPayload) -> String? {
        guard payload.kind == .asking, let request = payload.request else { return nil }
        switch request.type {
        case "permission": return request.allow != nil || request.deny != nil ? PushCategory.permission : nil
        case "confirm": return PushCategory.confirm
        case "input": return PushCategory.input
        default: return nil
        }
    }

    /// Markdown's marks out: a notification draws plain text.
    static func plain(_ text: String) -> String {
        text.replacingOccurrences(of: "`", with: "").replacingOccurrences(of: "**", with: "")
    }

    static func clip(_ text: String, _ limit: Int) -> String {
        text.count > limit ? String(text.prefix(limit - 1)) + "…" : text
    }
}

extension String {
    var nonEmpty: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}

#if canImport(UserNotifications)
extension NotificationWords {
    /// Writes these words over Apple's fixed ones.
    public func apply(to content: UNMutableNotificationContent, badge: Int?) {
        content.title = title
        content.subtitle = subtitle
        content.body = body
        content.threadIdentifier = thread
        content.targetContentIdentifier = thread
        content.relevanceScore = relevance
        content.categoryIdentifier = category ?? ""
        content.sound = sound ? .default : nil
        content.interruptionLevel =
            switch level {
            case .timeSensitive: .timeSensitive
            case .active: .active
            case .passive: .passive
            }
        if let badge { content.badge = NSNumber(value: badge) }
    }

    /// The categories, registered by the app at launch.
    public static var categories: Set<UNNotificationCategory> {
        let allow = UNNotificationAction(
            identifier: PushCategory.Action.allow, title: "Allow once", options: [.authenticationRequired])
        let deny = UNNotificationAction(
            identifier: PushCategory.Action.deny, title: "Deny", options: [.authenticationRequired, .destructive])
        let yes = UNNotificationAction(
            identifier: PushCategory.Action.yes, title: "Yes", options: [.authenticationRequired])
        let no = UNNotificationAction(
            identifier: PushCategory.Action.no, title: "No", options: [.authenticationRequired])
        let answer = UNTextInputNotificationAction(
            identifier: PushCategory.Action.answer, title: "Answer", options: [.authenticationRequired],
            textInputButtonTitle: "Send", textInputPlaceholder: "Your answer")
        let hidden = "Waiting on you"
        return [
            UNNotificationCategory(
                identifier: PushCategory.permission, actions: [allow, deny], intentIdentifiers: [],
                hiddenPreviewsBodyPlaceholder: hidden, options: []),
            UNNotificationCategory(
                identifier: PushCategory.confirm, actions: [yes, no], intentIdentifiers: [],
                hiddenPreviewsBodyPlaceholder: hidden, options: []),
            UNNotificationCategory(
                identifier: PushCategory.input, actions: [answer], intentIdentifiers: [],
                hiddenPreviewsBodyPlaceholder: hidden, options: []),
        ]
    }
}
#endif
