/** Synthetic Git review fixture. It never invokes native commands or touches a repository. */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type { GitWorkspace, ManagedWorktree, WorktreeActionPreview } from "../bindings";
import { GitDialog } from "../components/chat/GitDialog";
import type { FolderBusyApi } from "../lib/folderBusy";
import type { GitWorkspaceApi } from "../lib/gitWorkspace";
import { CommandError } from "../lib/ipc";
import type { ProjectToolsApi } from "../lib/projectTools";

const params = new URLSearchParams(location.search);
const calls: string[] = [];
const workspace: GitWorkspace = {
  root: "C:/projects/orbit",
  branch: "main",
  head: "5a4d64755cbb57ee734ad9502760eb225274491a",
  files: [],
  blockers: [],
  has_submodules: false,
  is_linked_worktree: false,
};
/** `&changes`: the folder has a modified file, a renamed one and a new one to commit. */
if (params.has("changes"))
  workspace.files = [
    { path: "src/orbit.rs", index: ".", worktree: "M", original_path: null },
    { path: "src/launch.rs", index: "R", worktree: ".", original_path: "src/start.rs" },
    { path: "docs/checklist.md", index: "?", worktree: "?", original_path: null },
  ];
/** `?preview=git-missing`: the chat's worktree folder was deleted by hand; its branch remains. */
const missing = params.get("preview") === "git-missing";
/**
 * `?preview=git-conflict`: a worktree chat whose merge into main conflicts in two files. With
 * `&no-editors` no editor is detected; with `&refuse-send` the message is not sent.
 */
const conflicted = params.get("preview") === "git-conflict";
let worktrees: ManagedWorktree[] = missing
  ? [
      {
        id: "worktree-one",
        chat_id: "chat",
        source: workspace.root,
        path: "C:/auracode/worktrees/owned-one",
        branch: "ghost/release-readiness",
        base_commit: workspace.head ?? "",
        state: "missing",
        detail:
          "This worktree was removed outside the app. Its branch and conversation remain available.",
        recovery: null,
        created_at: 1,
      },
    ]
  : [];
if (conflicted)
  worktrees = [
    {
      id: "worktree-one",
      chat_id: "chat",
      source: workspace.root,
      path: "C:/auracode/worktrees/owned-one",
      branch: "ghost/release-readiness",
      base_commit: workspace.head ?? "",
      state: "ready",
      detail: null,
      recovery: null,
      created_at: 1,
    },
  ];
let preview: WorktreeActionPreview | null = null;
let sequence = 0;
let currentFolder = conflicted ? "C:/auracode/worktrees/owned-one" : workspace.root;
let initialized = !params.has("non-git");
let relocationTarget = "";
const makeWorktree = (): ManagedWorktree => ({
  id: "worktree-one",
  chat_id: "chat",
  source: workspace.root,
  path: "C:/auracode/worktrees/owned-one",
  branch: "ghost/release-readiness",
  base_commit: workspace.head ?? "",
  state: "ready",
  detail: null,
  recovery: null,
  created_at: 1,
});
const api: GitWorkspaceApi = {
  inspect: async () => {
    if (!initialized)
      throw new Error(
        "This folder is not a Git working tree. Initialize it explicitly before creating isolation.",
      );
    return conflicted
      ? {
          ...structuredClone(workspace),
          root: currentFolder,
          branch: "ghost/release-readiness",
          is_linked_worktree: true,
        }
      : { ...structuredClone(workspace), root: currentFolder };
  },
  initializePreview: async (_chat, branch) => {
    calls.push("review-initialize");
    return {
      token: "initialize-once",
      folder: workspace.root,
      initial_branch: branch,
      expires_at: Date.now() + 600000,
      warnings: [
        "Creates only a new .git directory. Existing working files remain unchanged; nothing is staged or committed.",
      ],
    };
  },
  initializeApply: async (_chat, token) => {
    calls.push(`apply-initialize:${token}`);
    if (params.has("stale-init"))
      throw new Error("The folder changed since review. Review Git initialization again.");
    initialized = true;
    workspace.head = null;
    workspace.blockers = [
      "This repository has no commit yet. Create its first commit before isolation.",
    ];
    return structuredClone(workspace);
  },
  list: async () => structuredClone(worktrees),
  commitPreview: async (_chat, input) => {
    calls.push(`review-commit:${input.paths.join(",")}`);
    const files = workspace.files.filter((file) => input.paths.includes(file.path));
    return {
      token: `commit-${++sequence}`,
      branch: workspace.branch ?? "main",
      parent: workspace.head,
      author: "Fixture <fixture@localhost>",
      signed: params.has("signed"),
      files,
      message: `${input.message.trim()}\n`,
      diff_stat: `${files.map((file) => ` ${file.path} | 4 ++--`).join("\n")}\n ${files.length} files changed`,
      warnings: [
        "Git hooks do not run: Ghost commits exactly the reviewed files. Run your project's checks yourself if it relies on them.",
      ],
      expires_at: Date.now() + 600_000,
    };
  },
  commitApply: async (_chat, token) => {
    calls.push(`apply-commit:${token}`);
    // `&blocked`: another chat works in the folder until it is stopped, as Rust names it.
    if (params.has("blocked") && !otherStopped)
      throw new CommandError(
        "folder_busy",
        "\u201cFix the tests\u201d in orbit is working in this folder. Stop it, then try again.",
        {
          chats: [
            {
              chat_id: "other",
              title: "Fix the tests",
              agent_name: "Codex",
              project: "orbit",
              state: "working",
            },
          ],
          terminals: 0,
        },
      );
    if (sessionStorage.getItem("git-stale"))
      throw new Error("The files changed since the review. Review the commit again.");
    const commit = "7e1f0c9a3b5d2e8f4a6c1b9d0e3f5a7c2b4d6e8f";
    workspace.files = workspace.files.filter((file) => file.path === "docs/checklist.md");
    workspace.head = commit;
    return { commit, branch: workspace.branch ?? "main", warning: null };
  },
  pushPreview: async () => {
    calls.push("review-push");
    return {
      token: `push-${++sequence}`,
      branch: workspace.branch ?? "main",
      remote: "origin",
      remote_urls: ["https://github.com/fixture/orbit.git"],
      target: workspace.branch ?? "main",
      head: workspace.head ?? "",
      remote_head: "5a4d64755cbb57ee734ad9502760eb225274491a",
      commits: ["7e1f0c9 Add the release checklist"],
      count: 1,
      sets_upstream: false,
      warnings: [
        "Push sends only these commits. It never forces: if the remote branch moves first, Git refuses and nothing changes.",
      ],
      expires_at: Date.now() + 600_000,
    };
  },
  pushApply: async (_chat, token) => {
    calls.push(`apply-push:${token}`);
    return {
      remote: "origin",
      target: workspace.branch ?? "main",
      head: workspace.head ?? "",
      created: false,
      warning: null,
    };
  },
  isolationPreview: async (_chat, input) => {
    calls.push("review-isolation");
    return {
      token: `isolation-${++sequence}`,
      source: structuredClone(workspace),
      branch: `ghost/${input.slug}`,
      path: "C:/auracode/worktrees/owned-one",
      bring_changes: input.bring_changes,
      warnings: ["Uncommitted changes stay in the source folder."],
      expires_at: Date.now() + 600_000,
    };
  },
  isolationApply: async () => {
    calls.push("apply-isolation");
    const worktree = makeWorktree();
    worktrees = [worktree];
    return { worktree, warning: null };
  },
  actionPreview: async (_chat, _id, action) => {
    calls.push(`review-${action.type}`);
    preview = {
      token: `action-${++sequence}`,
      worktree: structuredClone(worktrees[0] ?? makeWorktree()),
      action,
      target: action.type === "merge" ? structuredClone(workspace) : null,
      changes:
        action.type === "merge"
          ? "src/orbit.rs | 18 +++++++++++++-----\n1 file changed, 13 insertions(+), 5 deletions(-)"
          : "All working files stay in a retained directory.",
      conflicts:
        action.type !== "merge"
          ? []
          : conflicted
            ? ["src/orbit.rs", "apps/desktop/src/components/chat/Sidebar.tsx"]
            : sessionStorage.getItem("git-conflict")
              ? ["src/orbit.rs"]
              : [],
      warnings: [
        action.type === "merge"
          ? "The reviewed target branch and files will change."
          : "The branch, ignored files and uncommitted work are retained.",
      ],
      expires_at: Date.now() + 600_000,
    };
    return structuredClone(preview);
  },
  actionApply: async () => {
    calls.push("apply-action");
    if (sessionStorage.getItem("git-stale"))
      throw new Error("The repository changed after this review. Review it again before applying.");
    const tree = worktrees[0] ?? makeWorktree();
    const worktree =
      preview?.action.type === "discard"
        ? { ...tree, state: "retired" as const, path: "C:/auracode/retained/owned-one" }
        : tree;
    worktrees = [worktree];
    return {
      worktree,
      recovery: "The branch and recovery refs remain available.",
      commit: preview?.action.type === "merge" ? workspace.head : null,
      error: null,
    };
  },
  recoveryPreview: async (_chat, _id, action) => {
    calls.push(`review-${action}`);
    return {
      token: `recovery-${++sequence}`,
      worktree: structuredClone(worktrees[0] ?? makeWorktree()),
      action,
      folder: { ...workspace, root: "C:/auracode/retained/owned-one" },
      target: null,
      destination: "C:/auracode/worktrees/owned-one",
      warnings: ["Restore keeps the retained files and branch."],
      expires_at: Date.now() + 600_000,
    };
  },
  recoveryApply: async () => {
    calls.push("apply-recovery");
    const worktree = makeWorktree();
    worktrees = [worktree];
    return {
      worktree,
      recovery: "The retained worktree is available again.",
      commit: null,
      error: null,
    };
  },
  relocationPreview: async (_chat, _id, destination) => {
    calls.push(`review-move-${destination}`);
    const tree = worktrees[0] ?? makeWorktree();
    relocationTarget = destination === "isolated" ? tree.path : tree.source;
    return {
      token: `relocation-${++sequence}`,
      worktree: structuredClone(tree),
      from: currentFolder,
      to: relocationTarget,
      handoff:
        "Continue release readiness in the reviewed folder. Keep the existing changes and inspect the destination before editing.",
      warnings: ["The next prompt starts a new native session with this visible context."],
      expires_at: Date.now() + 600_000,
    };
  },
  relocationApply: async () => {
    calls.push("apply-relocation");
    if (sessionStorage.getItem("git-stale"))
      throw new Error("The repository changed after this review. Review it again before applying.");
    currentFolder = relocationTarget;
    return { chat_id: "chat", prompt: null };
  },
  pullRequestPreview: async (_chat, id, input) => {
    calls.push("review-pr");
    return {
      token: "fixture-pr",
      worktree_id: id,
      repository: "fixture/orbit",
      repository_url: "https://github.com/fixture/orbit",
      head_branch: makeWorktree().branch,
      head_commit: workspace.head ?? "",
      base_branch: input.base_branch ?? "main",
      title: input.title,
      body: input.body,
      will_push: true,
      existing_url: null,
      diff_stat: " src/app.ts | 2 +-",
      warnings: ["Only the reviewed commit and PR text will be published."],
      expires_at: Date.now() + 600_000,
    };
  },
  pullRequestApply: async () => {
    calls.push("apply-pr");
    if (sessionStorage.getItem("git-stale"))
      throw new Error("The repository changed after this review. Review it again before applying.");
    return {
      url: "https://github.com/fixture/orbit/pull/1",
      created: true,
      head_commit: workspace.head ?? "",
      warning: null,
    };
  },
  recreatePreview: async (_chat, id) => {
    calls.push("review-recreate");
    const tree = worktrees.find((entry) => entry.id === id) ?? makeWorktree();
    const commit = "9c41e07d2b8a6f13e5c0d4b7a2f98e16c3d5b0a4";
    return {
      token: `recreate-${++sequence}`,
      worktree: structuredClone(tree),
      path: tree.path,
      branch: tree.branch,
      commit,
      warnings: [
        "Uncommitted files that were in the deleted folder are gone. Everything committed on the branch is kept.",
        `The folder is recreated at its original app-owned path from the last commit on ${tree.branch} (${commit.slice(0, 12)}).`,
        "Setup files such as .env are not restored. Review worktree setup again afterwards.",
      ],
      expires_at: Date.now() + 600_000,
    };
  },
  recreateApply: async (_chat, id, token) => {
    calls.push(`apply-recreate:${token}`);
    if (sessionStorage.getItem("git-stale")) {
      sessionStorage.removeItem("git-stale");
      throw new Error("This recreate preview expired or was already used.");
    }
    const tree = worktrees.find((entry) => entry.id === id) ?? makeWorktree();
    const worktree: ManagedWorktree = {
      ...tree,
      state: "ready",
      detail:
        "Recreated from its branch after the folder was removed outside the app. Uncommitted files from the removed folder could not be restored.",
    };
    worktrees = [worktree];
    return {
      worktree,
      recovery: worktree.path,
      commit: "9c41e07d2b8a6f13e5c0d4b7a2f98e16c3d5b0a4",
      error: null,
    };
  },
};
let sent = "";
let reviews = 0;
/** Detected editors for the chat's folder; opening one only records it. */
const projectTools: Pick<ProjectToolsApi, "review" | "openEditor"> = {
  review: async () => {
    calls.push("review-tools");
    return {
      token: `tools-${++reviews}`,
      folder: currentFolder,
      shells: [],
      editors: params.has("no-editors") ? [] : ["cursor", "zed", "rustrover"],
      expires_at: Date.now() + 600_000,
    };
  },
  openEditor: async (_chat, token, editor) => {
    calls.push(`editor:${editor}:${token}`);
  },
};
let otherStopped = false;
const folderBusy: Pick<FolderBusyApi, "stop"> = {
  stop: async (chatIds) => {
    calls.push(`stop-chats:${chatIds.join(",")}`);
    otherStopped = true;
    return { stopped: chatIds, kept: [], stopping: [] };
  },
};
Object.assign(window, { gitFixture: { calls: () => [...calls], sent: () => sent } });
function Preview() {
  const [live, setLive] = useState(params.has("live"));
  const [open, setOpen] = useState(true);
  const ask = async (text: string) => {
    calls.push("ask-agent");
    if (params.has("refuse-send"))
      return "Saving is paused, so this message was not sent. Try again when saving resumes.";
    sent = text;
    setOpen(false);
    return null;
  };
  return (
    <main className="g-root" style={{ height: "100%", background: "var(--g-bg)" }}>
      <h1 className="sr-only">Git fixture workspace</h1>
      {open ? (
        <GitDialog
          chatId="chat"
          title="Release readiness"
          api={api}
          live={live}
          agentName="Claude Code"
          onAskAgent={ask}
          projectTools={projectTools}
          folderBusy={folderBusy}
          allowIsolation
          onStop={async () => {
            calls.push("stop");
            setLive(false);
          }}
          onChanged={() => {
            calls.push("refresh-chat");
          }}
          onClose={() => setOpen(false)}
        />
      ) : (
        <button type="button" onClick={() => setOpen(true)}>
          Open Git workspace
        </button>
      )}
    </main>
  );
}
export function mountGitPreview(container: HTMLElement): void {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
