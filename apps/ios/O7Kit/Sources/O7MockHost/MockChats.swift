public import Foundation
public import O7Wire

/// The made-up computer's chats: what each says, and the turn a new message starts. They read like
/// real work so the app's screens can be judged on them.
public struct MockChat: Sendable {
    public var summary: WorkspaceChat
    public var events: [HistoryItem]
    public var pending: PermissionRequest?

    var id: String { summary.chat.id }
}

enum Script {
    static let now: Int64 = 1_795_600_000_000

    static func summary(
        _ id: String, _ title: String, agent: (String, String), project: String, folder: String,
        status: SessionStatus, attention: Bool = false, minutesAgo: Int64
    ) -> WorkspaceChat {
        WorkspaceChat(
            chat: ChatSummary(
                id: id, title: title, agentId: agent.0, agentName: agent.1, folder: folder,
                status: status, needsAttention: attention, createdAt: now - minutesAgo * 60_000 - 3_600_000,
                updatedAt: now - minutesAgo * 60_000),
            projectId: project, folderId: project == "p1" ? "f1" : "f2")
    }

    static let claude = ("claude-code", "Claude Code")
    static let codex = ("codex", "Codex")

    static func history(_ chat: String, _ events: [O7Event], turn: String) -> [HistoryItem] {
        events.enumerated().map { index, event in
            HistoryItem(seq: Int64(index), turnId: turn, at: now - Int64(events.count - index) * 1_000, event: event)
        }
    }

    static let loginAnswer = """
        The submit handler never checks the email field, so an empty form posts. I added a check \
        before the request goes out:

        ```ts
        if (!email.includes("@")) {
          setError("Enter your email address.");
          return;
        }
        ```

        | Case | Before | After |
        | --- | --- | --- |
        | Empty | Posted | Refused |
        | `a@b` | Posted | Posted |

        The tests pass: **42 passed**, none failed.
        """

    static func chats() -> [MockChat] {
        let fix = summary(
            "c1", "Fix the login form", agent: claude, project: "p1", folder: "/Users/you/code/o7",
            status: .ready, minutesAgo: 12)
        let ship = summary(
            "c2", "Ship the iPhone app", agent: codex, project: "p1", folder: "/Users/you/code/o7",
            status: .running, minutesAgo: 0)
        let tidy = summary(
            "c3", "Tidy the changelog", agent: claude, project: "p2", folder: "/Users/you/code/site",
            status: .running, attention: true, minutesAgo: 1)
        let speed = summary(
            "c4", "Speed up the history page", agent: codex, project: "p2",
            folder: "/Users/you/code/site", status: .failed, minutesAgo: 95)
        let tidyRequest = PermissionRequest(
            requestId: "r3", toolCallId: "t3b", title: "Run `pnpm lint --fix`", kind: .execute,
            rawInput: .object(["command": .string("pnpm lint --fix")]),
            options: [
                PermissionOption(optionId: "allow", kind: .allowOnce, label: "Allow once"),
                PermissionOption(optionId: "always", kind: .allowAlways, label: "Always allow"),
                PermissionOption(optionId: "reject", kind: .rejectOnce, label: "Reject"),
            ])
        return [
            MockChat(
                summary: fix,
                events: history(
                    "c1",
                    [
                        .turnStarted(turnId: "c1-t0", promptId: nil),
                        .userMessageDelta(messageId: "c1-u", text: "Why does the login form submit empty emails?"),
                        .thoughtDelta(thoughtId: "c1-th", text: "Look at the submit handler first."),
                        .thoughtCompleted(thoughtId: "c1-th", durationMs: 2_400),
                        .toolCall(
                            ToolCall(
                                toolCallId: "c1-t1", kind: .read, title: "Read src/login.ts",
                                status: .completed,
                                locations: [ToolLocation(path: "src/login.ts", line: 12)])),
                        .toolCall(
                            ToolCall(
                                toolCallId: "c1-t2", kind: .edit, title: "Edit src/login.ts",
                                status: .completed,
                                content: [
                                    .diff(
                                        path: "src/login.ts", oldText: "submit(email);\n",
                                        newText: "if (!email.includes(\"@\")) return;\nsubmit(email);\n",
                                        truncated: nil)
                                ])),
                        .toolCall(
                            ToolCall(
                                toolCallId: "c1-t3", kind: .execute, title: "Run `pnpm test`",
                                status: .completed,
                                content: [.terminal(output: "42 passed\n", exitCode: 0, truncated: nil)])),
                        .messageDelta(messageId: "c1-m", text: loginAnswer),
                        .messageCompleted(messageId: "c1-m"),
                        .usageUpdated(UsageInfo(contextUsed: 82_000, contextSize: 200_000, cost: Cost(amount: 0.42, currency: "USD"))),
                        .turnEnded(turnId: "c1-t0", stopReason: .endTurn, usage: nil, durationMs: 192_000, recovered: false),
                    ], turn: "c1-t0")),
            MockChat(
                summary: ship,
                events: history(
                    "c2",
                    [
                        .turnStarted(turnId: "c2-t0", promptId: nil),
                        .userMessageDelta(messageId: "c2-u", text: "Build the release and run every check."),
                        .planUpdated(entries: [
                            PlanEntry(content: "Build the app", status: .completed, priority: .high),
                            PlanEntry(content: "Run the unit tests", status: .inProgress, priority: .high),
                            PlanEntry(content: "Take the screenshots", status: .pending, priority: .medium),
                        ]),
                        .toolCall(
                            ToolCall(
                                toolCallId: "c2-t1", kind: .execute, title: "Run `xcodebuild build`",
                                status: .completed,
                                content: [.terminal(output: "** BUILD SUCCEEDED **\n", exitCode: 0, truncated: nil)])),
                        .toolCall(
                            ToolCall(
                                toolCallId: "c2-t2", kind: .execute, title: "Run `swift test`",
                                status: .inProgress)),
                        .taskUpdated(
                            backgroundTask(
                                id: "c2-k1", title: "Review the diff", activity: "Reading 4 files",
                                prompt: "Read the branch's changes and list what is wrong, most serious first.")),
                    ], turn: "c2-t0")),
            MockChat(
                summary: tidy,
                events: history(
                    "c3",
                    [
                        .turnStarted(turnId: "c3-t0", promptId: nil),
                        .userMessageDelta(messageId: "c3-u", text: "Make every changelog line short and plain."),
                        .toolCall(
                            ToolCall(toolCallId: "c3-t1", kind: .read, title: "Read CHANGELOG.md", status: .completed)),
                        .messageDelta(messageId: "c3-m", text: "I shortened 14 lines. Before I commit, I want to run the linter."),
                        .messageCompleted(messageId: "c3-m"),
                        .toolCall(
                            ToolCall(toolCallId: "t3b", kind: .execute, title: "Run `pnpm lint --fix`", status: .pending)),
                        .permissionRequested(tidyRequest),
                    ], turn: "c3-t0"),
                pending: tidyRequest),
            MockChat(
                summary: speed,
                events: history(
                    "c4",
                    [
                        .turnStarted(turnId: "c4-t0", promptId: nil),
                        .userMessageDelta(messageId: "c4-u", text: "Make the history page load faster."),
                        .error(kind: .agentExited, message: "The agent stopped unexpectedly.", detail: "exit status 137", recoverable: true),
                        .turnEnded(turnId: "c4-t0", stopReason: .error, usage: nil, durationMs: 41_000, recovered: false),
                    ], turn: "c4-t0")),
        ]
    }

    /// A background subagent, as the desktop reports one.
    static func backgroundTask(id: String, title: String, activity: String, prompt: String) -> BackgroundTask {
        let json: JSONValue = .object([
            "task_id": .string(id), "kind": .string("agent"), "title": .string(title),
            "agent_type": .string("reviewer"), "tool_call_id": .null, "parent_task_id": .null,
            "backgrounded": .bool(true), "status": .string("running"), "activity": .string(activity),
            "tool_uses": .number(6), "tokens": .number(18_400), "duration_ms": .null,
            "started_at": .number(Double(now - 95_000)), "ended_at": .null, "prompt": .string(prompt),
            "summary": .null, "summary_truncated": .null,
        ])
        return try! json.decode(BackgroundTask.self)
    }

    static let projects = [
        ProjectSummary(
            id: "p1", name: "o7", defaultAgent: "claude-code",
            folders: [ProjectFolder(id: "f1", projectId: "p1", displayName: "o7", path: "/Users/you/code/o7")]),
        ProjectSummary(
            id: "p2", name: "Website", defaultAgent: "codex",
            folders: [ProjectFolder(id: "f2", projectId: "p2", displayName: "site", path: "/Users/you/code/site")]),
    ]

    static let agents = [
        RemoteAgent(id: "claude-code", name: "Claude Code", available: true),
        RemoteAgent(id: "codex", name: "Codex", available: true),
        RemoteAgent(id: "grok", name: "Grok Build", available: false),
    ]

    /// The turn a message starts: it reads, asks to run the tests, and finishes once answered.
    static func turn(chat: String, number: Int, text: String) -> (before: [O7Event], request: PermissionRequest, after: [O7Event]) {
        let turn = "\(chat)-t\(number)"
        let request = PermissionRequest(
            requestId: "\(turn)-p", toolCallId: "\(turn)-run", title: "Run `npm test`", kind: .execute,
            options: [
                PermissionOption(optionId: "allow", kind: .allowOnce, label: "Allow once"),
                PermissionOption(optionId: "reject", kind: .rejectOnce, label: "Reject"),
            ])
        let before: [O7Event] = [
            .turnStarted(turnId: turn, promptId: nil),
            .userMessageDelta(messageId: "\(turn)-u", text: text),
            .messageDelta(messageId: "\(turn)-m1", text: "Looking at the **login form** now."),
            .messageCompleted(messageId: "\(turn)-m1"),
            .toolCall(ToolCall(toolCallId: "\(turn)-read", kind: .read, title: "Read src/login.ts", status: .completed)),
            .toolCall(ToolCall(toolCallId: "\(turn)-run", kind: .execute, title: "Run `npm test`", status: .pending)),
            .permissionRequested(request),
        ]
        let after: [O7Event] = [
            .toolUpdate(ToolUpdate(toolCallId: "\(turn)-run", status: .completed, content: [.terminal(output: "12 passed\n", exitCode: 0, truncated: nil)])),
            .messageDelta(messageId: "\(turn)-m2", text: "Done. The form now checks the email before it submits:\n\n```ts\nif (!email.includes(\"@\")) return;\n```\n\nTry it with `a@b`."),
            .messageCompleted(messageId: "\(turn)-m2"),
            .turnEnded(turnId: turn, stopReason: .endTurn, usage: nil, durationMs: 4_000, recovered: false),
        ]
        return (before, request, after)
    }
}
