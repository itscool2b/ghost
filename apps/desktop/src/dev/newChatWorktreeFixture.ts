/**
 * New chat's worktree scenarios for the development harness: a Git project folder (the default
 * worktree), one with a saved setup (the setup terminal), one with no commit yet (Local with its
 * reason) and a plain folder (no chip). Rust is imitated, never called: branch names are checked
 * the way `worktree_chat_preview` checks them, and no folder, branch or process is created.
 */
import type {
  GitWorkspace,
  ProjectFolder,
  ProjectSummary,
  TerminalChunk,
  WorktreeChatInput,
  WorktreeChatPlan,
  WorktreeSetupInput,
} from "../bindings";
import type { TerminalTransport } from "../lib/terminal";
import { WorkspaceStore } from "../lib/workspace";
import type { WorktreeChatApi } from "../lib/worktreeChat";
import { fixtureFolderMatches } from "./folderMatches";

export type NewChatScenario = "worktree" | "setup" | "unavailable" | "plain";

export const FIXTURE_FOLDER = "/workspace/auracode";
const BRANCHES = [
  "main",
  "release/1.4",
  "feature/sidebar-scan",
  "fix/pacing-test",
  "docs/changelog",
];
const GENERATED = ["ghost/7f3a9c2e", "ghost/b41d06a7", "ghost/2e9c5f18"];
const SETUP: WorktreeSetupInput = {
  files: [".env.local", "config/development.json"],
  command: { program: "pnpm", args: ["install", "--frozen-lockfile"] },
};
const SOURCE: GitWorkspace = {
  root: FIXTURE_FOLDER,
  branch: "main",
  head: "5a4d64755cbb57ee734ad9502760eb225274491a",
  files: [{ path: "src/components/Sidebar.tsx", index: " ", worktree: "M", original_path: null }],
  blockers: [],
  has_submodules: false,
  is_linked_worktree: false,
};

const unsupported = async (): Promise<never> => {
  throw new Error("Unavailable in this UI fixture.");
};

/** Why a typed name is refused, in the words Rust uses. */
function refusal(name: string): string | null {
  if (!/^[A-Za-z0-9._/-]+$/.test(name) || name.startsWith("-"))
    return "Use letters, numbers, hyphens, underscores, dots and slashes for the branch name.";
  if (name.endsWith("/") || name.includes("..") || name.endsWith(".lock"))
    return "Git does not accept this branch name. Choose another name.";
  const lower = name.toLowerCase();
  if (
    BRANCHES.some(
      (branch) =>
        branch.toLowerCase() === lower ||
        lower.startsWith(`${branch.toLowerCase()}/`) ||
        branch.toLowerCase().startsWith(`${lower}/`),
    )
  )
    return "A branch with this name, or one that overlaps it, already exists. Choose another name; existing branches are never reused.";
  return null;
}

export function newChatFixture(scenario: NewChatScenario): {
  readonly workspace: WorkspaceStore;
  readonly api: WorktreeChatApi;
  readonly calls: string[];
} {
  const calls: string[] = [];
  let reviews = 0;
  let generated = 0;
  const folder: ProjectFolder = {
    id: "folder-auracode",
    project_id: "project-auracode",
    display_name: "auracode",
    path: FIXTURE_FOLDER,
    folder_state: "ok",
    unavailable_reason: null,
    kind: "local",
    git_remote_url: null,
    git_default_branch: scenario === "plain" ? null : "main",
  };
  const projects: ProjectSummary[] = [
    {
      id: "project-auracode",
      name: "auracode",
      color: null,
      default_agent: "claude",
      default_model: null,
      created_at: 1,
      updated_at: 1,
      folders: [folder],
    },
  ];
  const workspace = new WorkspaceStore({
    snapshot: async () => ({ projects: structuredClone(projects), chats: [] }),
    createProject: unsupported,
    updateProject: unsupported,
    addFolder: unsupported,
    rename: unsupported,
    pin: unsupported,
    setRole: unsupported,
    archive: unsupported,
    move: unsupported,
    preview: async () => ({ folder: structuredClone(folder), busy_chats: [] }),
    folderMatches: fixtureFolderMatches(() => ({ projects, chats: [] })),
    createChat: async (request) => {
      calls.push(`create-local:${request.folder_id}`);
      return {
        chat: chatOf("local-chat", FIXTURE_FOLDER),
        project_id: "project-auracode",
        folder_id: request.folder_id,
        pinned: false,
        archived: false,
      };
    },
  });
  const plans = new Map<string, WorktreeChatPlan>();
  let read: ((chunk: TerminalChunk) => void) | null = null;
  const terminal: TerminalTransport = {
    reserve: async () => "setup-terminal",
    start: async () => {
      calls.push("setup-start");
      return "setup-terminal";
    },
    read: () =>
      new Promise((resolve) => {
        if (calls.filter((call) => call === "setup-read").length === 0) {
          calls.push("setup-read");
          resolve({
            bytes: [
              ...new TextEncoder().encode(
                "\x1b[2m$\x1b[0m pnpm install --frozen-lockfile\r\nLockfile is up to date, resolution step is skipped\r\nProgress: resolved 191, reused 191, downloaded 0, added 112\r\n",
              ),
            ],
            done: false,
            exit_code: null,
            truncated: false,
          });
          return;
        }
        read = resolve;
      }),
    write: async () => {},
    resize: async () => {},
    close: async () => {
      calls.push("setup-close");
      read?.({ bytes: [], done: true, exit_code: null, truncated: false });
    },
  };
  const api: WorktreeChatApi = {
    preview: async (input: WorktreeChatInput) => {
      calls.push(
        `review:${input.start_branch ?? "current"}:${input.branch ?? "generated"}:${input.bring_changes}:${input.run_setup}`,
      );
      await new Promise((resolve) => setTimeout(resolve, 120));
      if (scenario === "plain")
        return {
          type: "not_git_repository",
          folder: FIXTURE_FOLDER,
          message: "This folder is not a Git repository. The chat works directly in the folder.",
          chip: {
            mode: "direct",
            starting_branch: null,
            branch: null,
            label: "Direct in Ghost",
          },
        };
      if (scenario === "unavailable")
        throw new Error(
          "The starting branch has no commit yet. Create a first commit before starting a worktree chat.",
        );
      const start = input.start_branch ?? "main";
      if (!BRANCHES.includes(start))
        throw new Error("Choose an existing local branch to start from.");
      if (input.bring_changes && start !== "main")
        throw new Error(
          "Uncommitted changes can be brought only when the worktree starts from the source folder's current commit.",
        );
      const typed = input.branch?.trim() || null;
      if (typed) {
        const reason = refusal(typed);
        if (reason) throw new Error(reason);
      }
      const branch = typed ?? GENERATED[generated++ % GENERATED.length] ?? "ghost/7f3a9c2e";
      const setup = scenario === "setup" && input.run_setup ? SETUP : null;
      const warnings: string[] = [];
      if (input.bring_changes)
        warnings.push(
          "Reviewed working-file contents, deletions and nonignored untracked files are copied as unstaged changes. The source files and index are preserved; source staging is not recreated.",
        );
      else
        warnings.push(
          `Uncommitted changes stay in the source folder. The worktree starts from the last commit on ${start}.`,
        );
      if (setup) {
        warnings.push(
          `After creation, saved setup copies ${SETUP.files.length} listed ${SETUP.files.length === 1 ? "file" : "files"} from the source folder. Existing files are never overwritten. Environment files may contain application secrets; their contents are not shown, logged or stored.`,
        );
        warnings.push(
          `Then the saved setup command (${SETUP.command?.program} ${SETUP.command?.args.join(" ")}) is ready to run in an owned terminal in the new worktree, before the agent starts. It can modify the worktree and uses your normal command environment.`,
        );
      } else if (scenario === "setup")
        warnings.push(
          "The saved worktree setup is skipped for this chat. It can be reviewed later from the chat's Git panel.",
        );
      const id = `${++reviews}`.padStart(4, "0");
      const plan: WorktreeChatPlan = {
        token: `fixture-${reviews}`,
        source: structuredClone(SOURCE),
        starting_branch: start,
        starting_commit: SOURCE.head ?? "",
        branch,
        branch_generated: typed === null,
        path: `/Users/you/.auracode/worktrees/auracode-${id}`,
        bring_changes: input.bring_changes,
        local_branches: [...BRANCHES],
        setup,
        chip: {
          mode: "worktree",
          starting_branch: start,
          branch,
          label: `Worktree from ${start} on ${branch}`,
        },
        warnings,
        expires_at: Date.now() + 600_000,
      };
      plans.set(plan.token, plan);
      return { type: "worktree", plan };
    },
    apply: async (token) => {
      calls.push(`apply:${token}`);
      await new Promise((resolve) => setTimeout(resolve, 240));
      if (sessionStorage.getItem("new-chat-stale")) {
        sessionStorage.removeItem("new-chat-stale");
        throw new Error("This worktree chat preview expired or was already used. Review it again.");
      }
      const plan = plans.get(token);
      if (!plan) throw new Error("This worktree chat preview expired or was already used.");
      plans.delete(token);
      const command = plan.setup?.command ? "fixture-setup-command" : null;
      return {
        chat: {
          chat: chatOf("worktree-chat", plan.path),
          project_id: "project-auracode",
          folder_id: "folder-worktree",
          pinned: false,
          archived: false,
        },
        worktree: {
          id: "worktree-fixture",
          chat_id: "worktree-chat",
          source: FIXTURE_FOLDER,
          path: plan.path,
          branch: plan.branch,
          base_commit: plan.starting_commit,
          state: "ready",
          detail: null,
          recovery: null,
          created_at: 1,
        },
        setup: plan.setup
          ? { copied: [...plan.setup.files], command_token: command, warning: null }
          : null,
        started: command === null,
        warnings: [],
      };
    },
    start: async (chatId) => {
      calls.push(`start:${chatId}`);
    },
    setupTerminal: () => terminal,
  };
  return { workspace, api, calls };
}

function chatOf(id: string, folder: string) {
  return {
    id,
    title: null,
    agent_id: "claude",
    agent_host: { type: "native" as const },
    agent_name: "Claude Code",
    folder,
    folder_state: "ok" as const,
    status: "starting" as const,
    needs_attention: false,
    created_at: 1,
    updated_at: 1,
  };
}
