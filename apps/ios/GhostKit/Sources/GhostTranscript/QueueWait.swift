public import GhostWire

// Why the queued messages wait (`queueWait.ts`), in the words the queue and the composer show,
// with the one action that helps. The session gives the reason as `SessionInfo.queueWait`'s raw
// JSON; `QueueWait(json:)` reads it.

public enum WorktreeSetupState: String, LenientEnum {
    case awaiting, running, failed, interrupted, succeeded
    public static let fallback = WorktreeSetupState.awaiting
}

public enum HoldCause: String, LenientEnum {
    /// Ghost restarted (or reopened the chat) with prompts still queued.
    case restart
    /// The person stopped the turn or the agent.
    case stop
    /// A turn failed, the agent crashed, or a checkpoint could not be saved.
    case error
    /// The agent asked for its own sign-in.
    case signIn = "sign_in"
    /// The chat's project or folder was removed.
    case workspaceRemoved = "workspace_removed"
    /// The agent reached its usage limit.
    case usageLimit = "usage_limit"
    public static let fallback = HoldCause.error
}

public enum QueueWait: Sendable, Hashable {
    case setup(state: WorktreeSetupState, detail: String?)
    case starting
    case working
    case finishingTurn(since: Int64?, sendsAt: Int64?)
    case needsAnswer(question: Bool)
    case savingCheckpoint
    case stopping
    case chatChanging
    case startFailed
    case signIn
    case stopped
    case folderMissing
    case workspaceRemoved
    case held(after: HoldCause)
    /// A reason a newer computer gives that this copy does not know.
    case unknown(reason: String)

    /// The wire name of the reason.
    public var reason: String {
        switch self {
        case .setup: "setup"
        case .starting: "starting"
        case .working: "working"
        case .finishingTurn: "finishing_turn"
        case .needsAnswer: "needs_answer"
        case .savingCheckpoint: "saving_checkpoint"
        case .stopping: "stopping"
        case .chatChanging: "chat_changing"
        case .startFailed: "start_failed"
        case .signIn: "sign_in"
        case .stopped: "stopped"
        case .folderMissing: "folder_missing"
        case .workspaceRemoved: "workspace_removed"
        case .held: "held"
        case .unknown(let reason): reason
        }
    }

    /// The wait as `SessionInfo.queueWait` gives it; nil for no wait.
    public init?(json: JSONValue?) {
        guard let json, case .object = json, let reason = json["reason"]?.string else { return nil }
        switch reason {
        case "setup":
            self = .setup(
                state: json["state"]?.string.flatMap(WorktreeSetupState.init(rawValue:)) ?? .fallback,
                detail: json["detail"]?.string)
        case "starting": self = .starting
        case "working": self = .working
        case "finishing_turn":
            self = .finishingTurn(
                since: json["since"]?.number.map { Int64($0) }, sendsAt: json["sends_at"]?.number.map { Int64($0) })
        case "needs_answer": self = .needsAnswer(question: json["question"]?.bool ?? false)
        case "saving_checkpoint": self = .savingCheckpoint
        case "stopping": self = .stopping
        case "chat_changing": self = .chatChanging
        case "start_failed": self = .startFailed
        case "sign_in": self = .signIn
        case "stopped": self = .stopped
        case "folder_missing": self = .folderMissing
        case "workspace_removed": self = .workspaceRemoved
        case "held": self = .held(after: json["after"]?.string.flatMap(HoldCause.init(rawValue:)) ?? .fallback)
        default: self = .unknown(reason: reason)
        }
    }
}

/// The one thing a person can do about a wait, when there is one.
public enum QueueAction: String, Sendable, Hashable, Codable, CaseIterable {
    /// Moves the next message ahead now: interrupts the running turn, or stops waiting for idle.
    case sendNow = "send_now"
    /// The held message everything waits behind: Send it now, or Discard (and Send all when more
    /// than one is held). A held message never goes on its own.
    case sendHeld = "send_held"
    /// Starts the agent again after a failed start, or starts a stopped one.
    case start
    /// Recreates this chat's missing worktree folder.
    case recreate
    case setup
}

public struct QueueWaitView: Sendable, Hashable, Codable {
    public var reason: String
    /// Why the next message waits, in plain words.
    public var text: String
    /// More about it, for the head's tooltip.
    public var detail: String?
    public var action: QueueAction?
    /// A failure the person has to clear (drawn red), rather than a wait that passes.
    public var failed: Bool
}

public struct QueueWaitContext: Sendable, Hashable, Codable {
    /// The agent can interrupt a running turn (its capabilities say so, or are not known yet).
    public var canInterrupt: Bool
    /// The missing folder is this chat's own worktree, which Recreate folder can bring back.
    public var canRecreate: Bool

    public init(canInterrupt: Bool, canRecreate: Bool) {
        self.canInterrupt = canInterrupt
        self.canRecreate = canRecreate
    }
}

private func heldText(_ after: HoldCause, _ agent: String) -> String {
    switch after {
    case .restart: "Held after a restart"
    case .stop: "Held after you stopped \(agent)"
    case .error: "Held after an error"
    case .signIn: "Held until you sign in to \(agent)"
    case .workspaceRemoved: "Held while this project or folder is removed"
    case .usageLimit: "Held at \(agent)'s usage limit"
    }
}

/// Why the queued messages wait, with the one action that helps. Nil when nothing waits (or the
/// session has not said why yet, or gave a reason this copy does not know).
public func queueWaitView(_ wait: QueueWait?, agent: String, context: QueueWaitContext) -> QueueWaitView? {
    guard let wait else { return nil }
    func view(_ text: String, _ action: QueueAction? = nil, _ detail: String? = nil, _ failed: Bool = false)
        -> QueueWaitView
    {
        QueueWaitView(reason: wait.reason, text: text, detail: detail, action: action, failed: failed)
    }
    switch wait {
    case .setup(let state, let detail):
        let failed = state == .failed || state == .interrupted
        return view(
            failed ? (detail ?? "Setup failed. Retry setup above.") : state == .running ? "Setup is running" : "Waiting for setup review",
            failed ? .setup : nil,
            "Your messages stay queued until setup succeeds.",
            failed)
    case .starting:
        return view("Starting \(agent)", nil, "Your message goes as soon as it is ready.")
    case .working:
        return view(
            "\(agent) is still working",
            context.canInterrupt ? .sendNow : nil,
            context.canInterrupt
                ? "Send now stops the current turn and sends the next message."
                : "Your message goes when this turn ends.")
    case .finishingTurn:
        return view(
            "Waiting for \(agent) to finish its turn",
            .sendNow,
            "\(agent) still reports work after its reply (a hook or a background task). The next message goes on its own within a few seconds, or now with Send now.")
    case .needsAnswer(let question):
        return view(question ? "Answer \(agent)'s question first" : "Answer the permission request first")
    case .savingCheckpoint:
        return view("Saving a checkpoint before sending", nil, "Your message goes right after.")
    case .stopping:
        return view("Stopping the current turn", nil, "The next message goes once it has stopped.")
    case .chatChanging:
        return view("Waiting for a change to this chat to finish")
    case .startFailed:
        return view("\(agent) could not start", .start, "The error above says why.", true)
    case .signIn:
        return view("Sign in to \(agent) first", nil, nil, true)
    case .stopped:
        return view("\(agent) is stopped", .start)
    case .folderMissing:
        return view(
            context.canRecreate ? "This worktree's folder is missing" : "This chat's folder is missing",
            context.canRecreate ? .recreate : nil,
            context.canRecreate
                ? "Recreate folder brings the worktree back on its branch." : "Put the folder back to continue.",
            true)
    case .workspaceRemoved:
        return view(
            "This project or folder was removed", nil,
            "Restore it from Removed projects and folders to continue.", true)
    case .held(let after):
        return view(
            heldText(after, agent),
            .sendHeld,
            "A held message waits for you, and every message after it waits behind it. Send it now, or discard it.")
    case .unknown:
        return nil
    }
}

/// The composer's placeholder: what Enter does now, in words. `status` is the session's status as
/// its wire name.
public func composerPlaceholder(status: String, agent: String) -> String {
    if status == "starting" { return "Message \(agent). It goes once \(agent) has started" }
    if status == "running" { return "Queue a message for \(agent)" }
    return "Message \(agent)"
}

public func composerPlaceholder(status: SessionStatus, agent: String) -> String {
    composerPlaceholder(status: status.rawValue, agent: agent)
}
