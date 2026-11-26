import CoreGraphics
import CryptoKit
public import Foundation
import ImageIO
public import GhostWire
import UniformTypeIdentifiers

/// The made-up computer's answers to the app's later operations: changes, a branch and its pull
/// request, a project's brief, a screen, tasks, uploads.
struct MockState {
    var briefs: [String: (markdown: String, revision: Int)] = [
        "p1": ("# Ghost\n\nShort sentences. No em dashes. Run `pnpm lint` before a commit.", 1),
        "p2": ("# Website\n\nThe site deploys when main changes.", 1),
    ]
    var archived: [WorkspaceChat] = []
    var queued: [JSONValue] = [
        .object([
            "id": .string("q1"), "goal": .string("Update the dependencies and fix what breaks"),
            "project_id": .string("p1"), "agent_id": .string("codex"), "review": .bool(true),
            "created_at": .number(Double(Script.now - 3_600_000)), "failed": .null, "failed_at": .null,
            "by": .string("device"),
        ]),
        .object([
            "id": .string("q2"), "goal": .string("Write the release notes for 0.6"), "project_id": .string("p2"),
            "agent_id": .string("claude-code"), "review": .bool(false),
            "created_at": .number(Double(Script.now - 30 * 3_600_000)),
            "failed": .string("The computer was asleep between 1 and 6."),
            "failed_at": .number(Double(Script.now - 20 * 3_600_000)), "by": .string("bot"),
        ]),
    ]
    var tasks: [JSONValue] = [
        .object([
            "chat_id": .string("c2"), "goal": .string("Ship the iPhone app"),
            "branch": .string("ghost/ship-the-iphone-app"), "review_agent": .string("claude-code"),
            "reviewer_chat": .null, "created_at": .number(Double(Script.now - 2 * 3_600_000)),
            "by": .string("device"),
        ])
    ]
    var uploads: [String: (size: Int, sha256: String, received: Data)] = [:]
    /// Uploads that arrived whole and matched their hash, by id.
    var completed: [String: Data] = [:]
    /// Where the phone said its notifications go.
    var pushToken: String?
    /// The next queued task's number: ids are never reused.
    var nextQueued = 3
    var access: [String: String] = [:]
}

extension MockComputer {
    /// Answers an operation of the app's later features, or says it does not know it.
    func control(_ op: String, _ request: JSONValue, re: Double, session id: Int) -> Bool {
        let chatId = request["chat_id"]?.string ?? ""
        switch op {
        case "archived":
            reply(re, .object(["chats": (try? JSONValue.of(state.archived)) ?? .array([]), "more": .bool(false)]), to: id)
        case "set_access":
            let access = request["access"]?.string ?? ""
            guard ["ask", "auto_edit", "full_auto"].contains(access) else {
                refuse(re, "invalid", "That is not an access level.", to: id)
                return true
            }
            state.access[chatId] = access
            reply(re, .object([:]), to: id)
        case "rename":
            let title = (request["title"]?.string ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !title.isEmpty, let at = chats.firstIndex(where: { $0.id == chatId }) else {
                refuse(re, "empty", "Give the chat a name.", to: id)
                return true
            }
            chats[at].summary.chat.title = title
            reply(re, .object([:]), to: id)
            announce(chatId)
        case "archive":
            let archived = request["archived"]?.bool ?? true
            if archived, let at = chats.firstIndex(where: { $0.id == chatId }) {
                var chat = chats.remove(at: at).summary
                chat.archived = true
                state.archived.insert(chat, at: 0)
            } else if !archived, let at = state.archived.firstIndex(where: { $0.chat.id == chatId }) {
                var chat = state.archived.remove(at: at)
                chat.archived = false
                chats.insert(MockChat(summary: chat, events: []), at: 0)
            }
            reply(re, .object([:]), to: id)
            announce(chatId)
        case "start_agent":
            reply(re, .object([:]), to: id)
        case "changes":
            let branch = request["scope"]?.string == "branch"
            reply(
                re,
                .object([
                    "files": .array([
                        .object(["path": .string("src/login.ts"), "kind": .string("modified")]),
                        .object(["path": .string("src/login.test.ts"), "kind": .string("added")]),
                    ] + (branch ? [.object(["path": .string("README.md"), "kind": .string("modified")])] : [])),
                    "unavailable": .null, "scope": .string(branch ? "branch" : "turn"),
                    "base": branch ? .string("a1b2c3d") : .null,
                ]), to: id)
        case "diff":
            let path = request["path"]?.string ?? ""
            if path == "README.md" {
                // A long file with two changes far apart: the stretch between folds away.
                let lines = (1...40).map { "Line \($0) of the guide, unchanged." }
                var changed = lines
                changed[11] = "Run `pnpm test --run` before you push."
                changed.insert("A new note near the end.", at: 34)
                var original = lines
                original[11] = "Run `pnpm test` before you push."
                reply(
                    re,
                    .object([
                        "before": .string(original.joined(separator: "\n") + "\n"),
                        "after": .string(changed.joined(separator: "\n") + "\n"), "binary": .bool(false),
                        "truncated": .bool(false),
                    ]), to: id)
                return true
            }
            let (before, after): (JSONValue, JSONValue) =
                path.hasSuffix("test.ts")
                ? (.null, .string("test(\"refuses an empty email\", () => {\n  expect(submit(\"\")).toBe(false);\n});\n"))
                : (
                    .string("export function submit(email: string) {\n  post(email);\n  return true;\n}\n"),
                    .string(
                        "export function submit(email: string) {\n  if (!email.includes(\"@\")) {\n    setError(\"Enter your email address.\");\n    return false;\n  }\n  post(email);\n  return true;\n}\n")
                )
            reply(re, .object(["before": before, "after": after, "binary": .bool(false), "truncated": .bool(false)]), to: id)
        case "facts":
            let usage: JSONValue = .array([.object(["window": .string("5 hours"), "used": .number(42)])])
            switch chatId {
            case "c1":
                // A task on its own branch, with a pull request whose checks passed.
                reply(
                    re,
                    .object([
                        "branch": .string("ghost/fix-the-login-form"), "worktree": .bool(true), "uncommitted": .number(1),
                        "usage": usage,
                        "pull_request": .object([
                            "number": .number(12), "url": .string("https://github.com/you/ghost/pull/12"),
                            "state": .string("open"), "checks": .string("passed"), "failed": .array([]),
                            "notes": .array([
                                .object([
                                    "author": .string("you"), "kind": .string("approved"),
                                    "body": .string("Looks right. Ship it."), "trusted": .bool(true),
                                ])
                            ]),
                        ]),
                    ]), to: id)
            case "c2":
                reply(
                    re,
                    .object([
                        "branch": .string("ghost/ship-the-iphone-app"), "worktree": .bool(true),
                        "uncommitted": .number(3), "usage": usage,
                    ]), to: id)
            default:
                // A chat in the project's own checkout: no branch of its own to show.
                reply(re, .object(["branch": .string("main"), "worktree": .bool(false), "usage": usage]), to: id)
            }
        case "memory":
            let brief = state.briefs[request["project_id"]?.string ?? ""] ?? ("", 0)
            reply(re, .object(["markdown": .string(brief.markdown), "revision": .string("r\(brief.revision)")]), to: id)
        case "set_memory":
            let project = request["project_id"]?.string ?? ""
            let current = state.briefs[project] ?? ("", 0)
            if let base = request["base_revision"]?.string, base != "r\(current.revision)" {
                refuse(re, "conflict", "The brief changed on the computer since you opened it.", to: id)
                return true
            }
            let markdown = request["markdown"]?.string ?? ""
            guard markdown.utf8.count <= 4096 else {
                refuse(re, "too_large", "A brief holds at most 4 KB.", to: id)
                return true
            }
            state.briefs[project] = (markdown, current.revision + 1)
            reply(re, .object(["revision": .string("r\(current.revision + 1)")]), to: id)
        case "picture":
            reply(re, .object(["mime": .string("image/png"), "data": .string(Self.screen().base64EncodedString())]), to: id)
        case "task":
            let goal = request["goal"]?.string ?? ""
            if request["when"]?.string == "tonight" {
                let queuedId = "q\(state.nextQueued)"
                state.nextQueued += 1
                state.queued.append(
                    .object([
                        "id": .string(queuedId), "goal": .string(goal), "project_id": request["project_id"] ?? .null,
                        "agent_id": request["agent_id"] ?? .null, "review": request["review"] ?? .bool(true),
                        "created_at": .number(Double(Script.now)), "failed": .null, "failed_at": .null,
                        "by": .string("device"),
                    ]))
                reply(re, .object(["queued": .string(queuedId)]), to: id)
                return true
            }
            let chat = newChat(folder: request["project_id"]?.string == "p2" ? "f2" : "f1", agent: request["agent_id"]?.string)
            let branch = "ghost/" + goal.lowercased().split(separator: " ").prefix(4).joined(separator: "-")
            state.tasks.insert(
                .object([
                    "chat_id": .string(chat.chat.id), "goal": .string(goal), "branch": .string(branch),
                    "review_agent": .string("codex"), "reviewer_chat": .null,
                    "created_at": .number(Double(Script.now)), "by": .string("device"),
                ]), at: 0)
            reply(re, .object(["chat_id": .string(chat.chat.id), "branch": .string(branch), "review_agent": .string("codex")]), to: id)
        case "tasks":
            reply(
                re,
                .object([
                    "tasks": .array(state.tasks), "queued": .array(state.queued),
                    "night": .object(["from": .number(1), "to": .number(6), "budget": .number(3), "started": .number(0)]),
                ]), to: id)
        case "task_cancel":
            let queuedId = request["queued_id"]?.string ?? ""
            let before = state.queued.count
            state.queued.removeAll { $0["id"]?.string == queuedId }
            if state.queued.count == before {
                refuse(re, "not_found", "Nothing is queued under that name.", to: id)
            } else {
                reply(re, .object([:]), to: id)
            }
        case "upload_begin":
            let uploadId = "u\(state.uploads.count + state.completed.count + 1)"
            state.uploads[uploadId] = (Int(request["size"]?.number ?? 0), request["sha256"]?.string ?? "", Data())
            reply(re, .object(["upload_id": .string(uploadId), "chunk": .number(49_152)]), to: id)
        case "upload_chunk":
            let uploadId = request["upload_id"]?.string ?? ""
            guard var upload = state.uploads[uploadId],
                let data = Data(base64Encoded: request["data"]?.string ?? "")
            else {
                refuse(re, "not_found", "That upload is gone.", to: id)
                return true
            }
            upload.received.append(data)
            let complete = upload.received.count >= upload.size
            if complete {
                state.uploads[uploadId] = nil
                guard Self.hex(SHA256.hash(data: upload.received)) == upload.sha256 else {
                    refuse(re, "corrupt", "The photo arrived damaged. Send it again.", to: id)
                    return true
                }
                state.completed[uploadId] = upload.received
            } else {
                state.uploads[uploadId] = upload
            }
            reply(
                re, .object(["received": .number(Double(upload.received.count)), "complete": .bool(complete)]),
                to: id)
        case "upload_abort":
            state.uploads[request["upload_id"]?.string ?? ""] = nil
            reply(re, .object([:]), to: id)
        case "push_register":
            let token = request["token"]?.string ?? ""
            guard (64...200).contains(token.count), token.allSatisfy(\.isHexDigit) else {
                refuse(re, "invalid", "That is not a notification token.", to: id)
                return true
            }
            state.pushToken = token
            // The demo has no relay to send through.
            reply(re, .object(["relay": .bool(false)]), to: id)
        case "push_unregister":
            state.pushToken = nil
            reply(re, .object([:]), to: id)
        case "push_test", "activity":
            reply(re, .object([:]), to: id)
        default:
            return false
        }
        return true
    }

    /// What an upload brought, once it arrived whole.
    public func uploaded(_ uploadId: String) -> Data? { state.completed[uploadId] }

    static func hex(_ digest: SHA256.Digest) -> String {
        digest.map { String(format: "%02x", $0) }.joined()
    }

    /// A made-up browser page, as the computer's picture of a chat's tab.
    static func screen() -> Data {
        let width = 780
        let height = 1500
        guard
            let context = CGContext(
                data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0,
                space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { return Data() }
        context.setFillColor(CGColor(red: 0.97, green: 0.97, blue: 0.98, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        context.setFillColor(CGColor(red: 0.12, green: 0.13, blue: 0.16, alpha: 1))
        context.fill(CGRect(x: 0, y: height - 120, width: width, height: 120))
        context.setFillColor(CGColor(red: 0.85, green: 0.47, blue: 0.34, alpha: 1))
        context.fill(CGRect(x: 60, y: height - 420, width: width - 120, height: 90))
        context.setFillColor(CGColor(red: 0.8, green: 0.81, blue: 0.84, alpha: 1))
        for row in 0..<6 {
            context.fill(CGRect(x: 60, y: height - 560 - row * 110, width: width - 120 - (row % 2) * 160, height: 44))
        }
        guard let image = context.makeImage() else { return Data() }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil) else {
            return Data()
        }
        CGImageDestinationAddImage(destination, image, nil)
        CGImageDestinationFinalize(destination)
        return data as Data
    }
}
