public import Foundation

/// A chat's process, as the desktop reports it (`SessionStatus`).
public enum SessionStatus: String, LenientEnum {
    case stopped, starting, ready, running, cancelling, stopping, failed
    case authRequired = "auth_required"
    /// A newer desktop's word this copy does not know: drawn as stopped.
    public static let fallback = SessionStatus.stopped

    /// A turn is going, or about to stop.
    public var isWorking: Bool { self == .running || self == .starting || self == .cancelling || self == .stopping }
}

public enum FolderState: String, LenientEnum {
    case ok, missing
    case notAllowed = "not_allowed"
    public static let fallback = FolderState.ok
}

public enum FolderKind: String, LenientEnum {
    case local, git
    public static let fallback = FolderKind.local
}

/// Where a chat's agent runs: on this computer, or in WSL on Windows.
public struct AgentHost: Codable, Hashable, Sendable {
    public var type: String
    public var distro: String?
}

/// A chat as the chat list shows it (`ChatSummary`).
public struct ChatSummary: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var title: String?
    public var agentId: String
    public var agentHost: AgentHost?
    public var agentName: String
    public var folder: String
    public var folderState: FolderState
    public var status: SessionStatus
    /// A permission card or question waits for the person.
    public var needsAttention: Bool
    public var createdAt: Int64
    public var updatedAt: Int64

    enum CodingKeys: String, CodingKey {
        case id, title, folder, status
        case agentId = "agent_id", agentHost = "agent_host", agentName = "agent_name"
        case folderState = "folder_state", needsAttention = "needs_attention"
        case createdAt = "created_at", updatedAt = "updated_at"
    }

    public init(
        id: String, title: String?, agentId: String, agentHost: AgentHost? = nil,
        agentName: String, folder: String, folderState: FolderState = .ok,
        status: SessionStatus, needsAttention: Bool, createdAt: Int64, updatedAt: Int64
    ) {
        self.id = id
        self.title = title
        self.agentId = agentId
        self.agentHost = agentHost
        self.agentName = agentName
        self.folder = folder
        self.folderState = folderState
        self.status = status
        self.needsAttention = needsAttention
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }
}

/// A chat with where it belongs (`WorkspaceChat`).
public struct WorkspaceChat: Codable, Hashable, Sendable, Identifiable {
    public var chat: ChatSummary
    public var projectId: String
    public var folderId: String?
    public var pinned: Bool
    public var archived: Bool
    /// The agent, model and thinking level the chat plans and builds with; read as it came.
    public var roles: JSONValue?

    public var id: String { chat.id }

    enum CodingKeys: String, CodingKey {
        case chat, pinned, archived, roles
        case projectId = "project_id", folderId = "folder_id"
    }

    public init(
        chat: ChatSummary, projectId: String, folderId: String?, pinned: Bool = false,
        archived: Bool = false, roles: JSONValue? = nil
    ) {
        self.chat = chat
        self.projectId = projectId
        self.folderId = folderId
        self.pinned = pinned
        self.archived = archived
        self.roles = roles
    }
}

public struct ProjectFolder: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var projectId: String
    public var displayName: String
    /// The computer's path; nil where it is not known there.
    public var path: String?
    public var folderState: FolderState
    public var unavailableReason: String?
    public var kind: FolderKind
    public var gitRemoteUrl: String?
    public var gitDefaultBranch: String?
    /// A worktree o7 made: the folder it was made from.
    public var worktreeOf: String?

    enum CodingKeys: String, CodingKey {
        case id, path, kind
        case projectId = "project_id", displayName = "display_name", folderState = "folder_state"
        case unavailableReason = "unavailable_reason", gitRemoteUrl = "git_remote_url"
        case gitDefaultBranch = "git_default_branch", worktreeOf = "worktree_of"
    }

    public init(
        id: String, projectId: String, displayName: String, path: String?,
        folderState: FolderState = .ok, unavailableReason: String? = nil, kind: FolderKind = .git,
        gitRemoteUrl: String? = nil, gitDefaultBranch: String? = nil, worktreeOf: String? = nil
    ) {
        self.id = id
        self.projectId = projectId
        self.displayName = displayName
        self.path = path
        self.folderState = folderState
        self.unavailableReason = unavailableReason
        self.kind = kind
        self.gitRemoteUrl = gitRemoteUrl
        self.gitDefaultBranch = gitDefaultBranch
        self.worktreeOf = worktreeOf
    }
}

public struct ProjectSummary: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var name: String
    public var color: String?
    public var defaultAgent: String?
    public var defaultModel: String?
    public var folders: [ProjectFolder]
    public var createdAt: Int64
    public var updatedAt: Int64

    enum CodingKeys: String, CodingKey {
        case id, name, color, folders
        case defaultAgent = "default_agent", defaultModel = "default_model"
        case createdAt = "created_at", updatedAt = "updated_at"
    }

    public init(
        id: String, name: String, color: String? = nil, defaultAgent: String? = nil,
        defaultModel: String? = nil, folders: [ProjectFolder], createdAt: Int64 = 0,
        updatedAt: Int64 = 0
    ) {
        self.id = id
        self.name = name
        self.color = color
        self.defaultAgent = defaultAgent
        self.defaultModel = defaultModel
        self.folders = folders
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }
}

/// An agent installed on the computer.
public struct RemoteAgent: Codable, Hashable, Sendable, Identifiable {
    public var id: String
    public var name: String
    /// It can run there now.
    public var available: Bool

    public init(id: String, name: String, available: Bool) {
        self.id = id
        self.name = name
        self.available = available
    }
}

/// The computer's projects, chats and agents (`overview`).
public struct Overview: Codable, Hashable, Sendable {
    public var hostName: String
    public var projects: [ProjectSummary]
    public var chats: [WorkspaceChat]
    public var agents: [RemoteAgent]
    public var defaultAgent: String?

    enum CodingKeys: String, CodingKey {
        case projects, chats, agents
        case hostName = "host_name", defaultAgent = "default_agent"
    }

    public init(
        hostName: String, projects: [ProjectSummary], chats: [WorkspaceChat],
        agents: [RemoteAgent], defaultAgent: String?
    ) {
        self.hostName = hostName
        self.projects = projects
        self.chats = chats
        self.agents = agents
        self.defaultAgent = defaultAgent
    }
}

/// A chat's status changed (`chat` event).
public struct ChatStatusUpdate: Codable, Hashable, Sendable {
    public var chatId: String
    public var status: SessionStatus
    public var needsAttention: Bool
    public var title: String?
    public var updatedAt: Int64

    enum CodingKeys: String, CodingKey {
        case status, title
        case chatId = "chat_id", needsAttention = "needs_attention", updatedAt = "updated_at"
    }

    public init(
        chatId: String, status: SessionStatus, needsAttention: Bool, title: String?,
        updatedAt: Int64
    ) {
        self.chatId = chatId
        self.status = status
        self.needsAttention = needsAttention
        self.title = title
        self.updatedAt = updatedAt
    }
}

/// The computer took a message: `queued` when it waits behind a running turn.
public struct PromptAck: Codable, Hashable, Sendable {
    public var promptId: String
    public var queued: Bool

    enum CodingKeys: String, CodingKey {
        case queued
        case promptId = "prompt_id"
    }

    public init(promptId: String, queued: Bool) {
        self.promptId = promptId
        self.queued = queued
    }
}
