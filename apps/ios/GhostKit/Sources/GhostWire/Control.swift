public import Foundation

// What the iPhone app's later features read: changes, a chat's branch and pull request, a
// project's brief, a screen, tasks, uploads. Each comes with its feature (`ready.features`): a
// computer whose Ghost does not offer one is never asked.

extension RemoteOp {
    public static func archived(before: Int64? = nil, limit: Int = 50) -> RemoteOp {
        var params: [String: JSONValue] = ["limit": .number(Double(limit))]
        if let before { params["before"] = .number(Double(before)) }
        return RemoteOp("archived", params, feature: "control")
    }
    public static func setAccess(_ chatId: String, access: String) -> RemoteOp {
        RemoteOp("set_access", ["chat_id": .string(chatId), "access": .string(access)], feature: "control")
    }
    public static func rename(_ chatId: String, title: String) -> RemoteOp {
        RemoteOp("rename", ["chat_id": .string(chatId), "title": .string(title)], feature: "control")
    }
    public static func archive(_ chatId: String, archived: Bool) -> RemoteOp {
        RemoteOp("archive", ["chat_id": .string(chatId), "archived": .bool(archived)], feature: "control")
    }
    public static func startAgent(_ chatId: String) -> RemoteOp {
        RemoteOp("start_agent", ["chat_id": .string(chatId)], feature: "control")
    }
    public static func changes(_ chatId: String, scope: ChangeScope = .turn) -> RemoteOp {
        RemoteOp("changes", ["chat_id": .string(chatId), "scope": .string(scope.rawValue)], feature: "review")
    }
    public static func diff(_ chatId: String, path: String, scope: ChangeScope = .turn) -> RemoteOp {
        RemoteOp(
            "diff", ["chat_id": .string(chatId), "path": .string(path), "scope": .string(scope.rawValue)],
            feature: "review")
    }
    public static func facts(_ chatId: String) -> RemoteOp {
        RemoteOp("facts", ["chat_id": .string(chatId)], feature: "review")
    }
    public static func memory(_ projectId: String) -> RemoteOp {
        RemoteOp("memory", ["project_id": .string(projectId)], feature: "memory")
    }
    public static func setMemory(_ projectId: String, markdown: String, base: String?) -> RemoteOp {
        var params: [String: JSONValue] = ["project_id": .string(projectId), "markdown": .string(markdown)]
        if let base { params["base_revision"] = .string(base) }
        return RemoteOp("set_memory", params, feature: "memory")
    }
    public static func picture(_ chatId: String, of: ScreenKind) -> RemoteOp {
        RemoteOp("picture", ["chat_id": .string(chatId), "of": .string(of.rawValue)], feature: "screens")
    }
    public static func task(
        goal: String, projectId: String?, agentId: String?, review: Bool, tonight: Bool,
        access: String?, attachments: [String] = []
    ) -> RemoteOp {
        var params: [String: JSONValue] = [
            "goal": .string(goal), "review": .bool(review), "when": .string(tonight ? "tonight" : "now"),
        ]
        if let projectId { params["project_id"] = .string(projectId) }
        if let agentId { params["agent_id"] = .string(agentId) }
        if let access { params["access"] = .string(access) }
        if !attachments.isEmpty { params["attachments"] = .array(attachments.map(JSONValue.string)) }
        return RemoteOp("task", params, feature: "tasks")
    }
    public static let tasks = RemoteOp("tasks", feature: "tasks")
    public static func taskCancel(_ queuedId: String) -> RemoteOp {
        RemoteOp("task_cancel", ["queued_id": .string(queuedId)], feature: "tasks")
    }
    public static func uploadBegin(name: String, mime: String, size: Int, sha256: String) -> RemoteOp {
        RemoteOp(
            "upload_begin",
            [
                "name": .string(name), "mime": .string(mime), "size": .number(Double(size)),
                "sha256": .string(sha256),
            ], feature: "attachments")
    }
    public static func uploadChunk(_ uploadId: String, index: Int, data: Data) -> RemoteOp {
        RemoteOp(
            "upload_chunk",
            [
                "upload_id": .string(uploadId), "index": .number(Double(index)),
                "data": .string(data.base64EncodedString()),
            ], feature: "attachments")
    }
    public static func uploadAbort(_ uploadId: String) -> RemoteOp {
        RemoteOp("upload_abort", ["upload_id": .string(uploadId)], feature: "attachments")
    }
    public static func create(folderId: String, agentId: String?, text: String, attachments: [String]) -> RemoteOp {
        var op = RemoteOp.create(folderId: folderId, agentId: agentId, text: text)
        guard !attachments.isEmpty else { return op }
        var params = op.params
        params["attachments"] = .array(attachments.map(JSONValue.string))
        op = RemoteOp("create", params, feature: "attachments")
        return op
    }
}

public enum ChangeScope: String, Sendable, Hashable, CaseIterable {
    /// What the last turn changed.
    case turn
    /// Everything the chat's own branch changed since it started.
    case branch
}

public enum ScreenKind: String, Sendable, Hashable, CaseIterable {
    case browser, device
}

/// Archived chats, newest first, a page at a time.
public struct ArchivedChats: Codable, Sendable {
    public var chats: [WorkspaceChat]
    public var more: Bool
}

/// The files a chat changed.
public struct ChangedFiles: Codable, Sendable {
    public struct File: Codable, Sendable, Hashable, Identifiable {
        public var path: String
        /// `added`, `modified` or `deleted`.
        public var kind: String
        public var id: String { path }
    }
    public var files: [File]
    /// Why there is nothing to list, in the computer's words.
    public var unavailable: String?
    public var scope: String?
    public var base: String?
}

/// One changed file, before and after.
public struct FileDiff: Codable, Sendable {
    public var before: String?
    public var after: String?
    public var binary: Bool
    public var truncated: Bool
}

/// What a chat stands on: its branch, what is not committed, its agent's usage, its pull request.
public struct ChatFacts: Codable, Sendable, Hashable {
    public struct Usage: Codable, Sendable, Hashable {
        public var window: String
        /// How much is used, 0 to 100.
        public var used: Double
    }
    public struct Note: Codable, Sendable, Hashable {
        public var author: String
        public var kind: String
        public var body: String
        /// Written by the repository's owner, a member or a collaborator.
        public var trusted: Bool
    }
    public struct PullRequest: Codable, Sendable, Hashable {
        public var number: Int
        public var url: String
        /// `open`, `merged` or `closed`.
        public var state: String
        /// `none`, `pending`, `passed` or `failed`.
        public var checks: String
        public var failed: [String]
        public var notes: [Note]
    }
    public var branch: String?
    public var worktree: Bool
    public var uncommitted: Int?
    public var usage: [Usage]
    public var pullRequest: PullRequest?

    enum CodingKeys: String, CodingKey {
        case branch, worktree, uncommitted, usage
        case pullRequest = "pull_request"
    }

    public init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        branch = try c.decodeIfPresent(String.self, forKey: .branch)
        worktree = try c.decodeIfPresent(Bool.self, forKey: .worktree) ?? false
        uncommitted = try c.decodeIfPresent(Int.self, forKey: .uncommitted)
        usage = (try? c.decodeIfPresent([Usage].self, forKey: .usage)) ?? []
        pullRequest = try? c.decodeIfPresent(PullRequest.self, forKey: .pullRequest)
    }
}

/// A project's brief: the short part of its memory every chat there receives.
public struct ProjectBrief: Codable, Sendable {
    public var markdown: String
    public var revision: String?
}

public struct SavedBrief: Codable, Sendable {
    public var revision: String?
}

/// A picture of a chat's browser tab or emulator screen.
public struct ScreenPicture: Codable, Sendable {
    public var mime: String
    /// Standard base64.
    public var data: String
}

/// A task that started now.
public struct StartedTask: Codable, Sendable {
    public var chatId: String?
    public var branch: String?
    public var reviewAgent: String?
    /// The queued task's id, for work left for tonight.
    public var queued: String?
    enum CodingKeys: String, CodingKey {
        case branch, queued
        case chatId = "chat_id", reviewAgent = "review_agent"
    }
}

/// Tasks handed to Ghost (by the phone or a bot), the queue for tonight, and the night's limits.
public struct TasksSnapshot: Codable, Sendable {
    public struct Task: Codable, Sendable, Hashable, Identifiable {
        public var chatId: String
        public var goal: String
        public var branch: String?
        public var reviewAgent: String?
        public var reviewerChat: String?
        public var createdAt: Int64?
        public var by: String?
        public var id: String { chatId }
        enum CodingKeys: String, CodingKey {
            case goal, branch, by
            case chatId = "chat_id", reviewAgent = "review_agent", reviewerChat = "reviewer_chat"
            case createdAt = "created_at"
        }
    }
    public struct Queued: Codable, Sendable, Hashable, Identifiable {
        public var id: String
        public var goal: String
        public var projectId: String?
        public var agentId: String?
        public var review: Bool?
        public var createdAt: Int64?
        public var failed: String?
        public var failedAt: Int64?
        public var by: String?
        enum CodingKeys: String, CodingKey {
            case id, goal, review, failed, by
            case projectId = "project_id", agentId = "agent_id", createdAt = "created_at"
            case failedAt = "failed_at"
        }
    }
    public struct Night: Codable, Sendable, Hashable {
        public var from: Int
        public var to: Int
        public var budget: Int
        public var started: Int
    }
    public var tasks: [Task]
    public var queued: [Queued]
    public var night: Night?
}

public struct UploadBegun: Codable, Sendable {
    public var uploadId: String
    public var chunk: Int
    enum CodingKeys: String, CodingKey {
        case chunk
        case uploadId = "upload_id"
    }
}

public struct UploadProgress: Codable, Sendable {
    public var received: Int
    public var complete: Bool
}
