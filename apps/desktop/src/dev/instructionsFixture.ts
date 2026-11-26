/**
 * Development-only imitation of Rust's Instructions answers, for previews and browser checks. It
 * keeps a small in-memory folder, reviews changes the way ghost-config does for these cases, and
 * changes state only on Apply. Never touches real files.
 */
import type {
  InstructionsAgent,
  InstructionsAgentStatus,
  InstructionsChange,
  InstructionsFile,
  InstructionsReview,
  InstructionsReviewFile,
  InstructionsSnapshot,
} from "../bindings";
import type { InstructionsApi } from "../lib/instructions";

export type InstructionsScenario =
  | "universal"
  | "per-agent"
  | "conflict"
  | "drift"
  | "large"
  | "recovery"
  | "review";

const SHARED = `# Ghost

Rust for every backend piece; React and TypeScript for interfaces.

## Commands

- \`pnpm lint\`, \`pnpm typecheck\` and \`pnpm test\` before every commit.
- \`cargo test -p ghost-config\` for configuration changes.

## Style

- Plain sentences. No em dashes.
- Keep functions small and name things plainly.
- One primary button per view.
`;

const CLAUDE_OWN = `# Claude Code

Prefer small commits with a clear message.
Run the focused tests first, then the full suite.
`;

const OVERRIDE = `# Codex only

Ask before running migrations.
`;

const NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  pi: "Pi",
  omp: "oh-my-pi",
  opencode: "OpenCode",
  hermes: "Hermes",
  cursor: "Cursor",
  grok: "Grok Build",
  antigravity: "Antigravity",
};
const ORDER = Object.keys(NAMES);
const INSTALLED = new Set(["claude", "codex", "pi", "opencode", "cursor"]);

function lines(text: string): number {
  return text === "" ? 0 : text.replace(/\n$/, "").split("\n").length;
}

function file(
  key: string,
  path: string,
  text: string | null,
  extra: Partial<InstructionsFile> = {},
): InstructionsFile {
  const bytes = text === null ? 0 : new TextEncoder().encode(text).length;
  return {
    key,
    path,
    kind: text === null ? "missing" : "file",
    disk_name: null,
    link_to: null,
    text,
    encoding: text === null ? null : { bom: false, line_ending: "lf", crlf_majority: false },
    bytes,
    lines: text === null ? 0 : lines(text),
    revision: text === null ? null : `rev-${key}-${bytes}`,
    read_only: false,
    empty: text === "",
    editable: true,
    import_of: null,
    imports: [],
    managed: null,
    copy_of: null,
    readers: [],
    git: text === null ? null : "tracked",
    note: null,
    ...extra,
  };
}

function agent(
  id: string,
  status: InstructionsAgentStatus,
  fileKey: string | null,
  detail: string,
  extra: Partial<InstructionsAgent> = {},
): InstructionsAgent {
  return {
    id,
    name: NAMES[id] ?? id,
    installed: INSTALLED.has(id),
    status,
    file: fileKey,
    via: null,
    also: [],
    shared_with: [],
    detail,
    warnings: [],
    override_file: null,
    ...extra,
  };
}

interface State {
  scope: "project" | "global";
  mode: "universal" | "per_agent";
  agents: string;
  claude: string | null;
  override: string | null;
  drift: boolean;
  large: boolean;
}

function universalSnapshot(state: State): InstructionsSnapshot {
  const agents = file("project:AGENTS.md", "AGENTS.md", state.agents, {
    readers: ORDER,
  });
  const claude = file("project:CLAUDE.md", "CLAUDE.md", "@AGENTS.md\n", {
    import_of: "project:AGENTS.md",
    imports: ["AGENTS.md"],
    readers: ["claude", "cursor", "grok"],
    git: "untracked",
    note: "Not committed, so new worktree chats won't have it.",
  });
  const shared = ["codex", "pi", "omp", "opencode", "hermes", "cursor", "grok", "antigravity"];
  const bytes = agents.bytes.toLocaleString("en-US");
  const largeWarnings = state.large
    ? {
        codex: [
          `Codex reads the first 32 KiB. This text is ${(agents.bytes / 1024).toFixed(1)} KiB, so the end is cut.`,
        ],
        claude: [`Claude Code suggests under 200 lines. AGENTS.md has ${agents.lines}.`],
        hermes: [
          `Hermes may cut AGENTS.md on smaller models, keeping its start and end. It has ${bytes} characters; the floor is 20,000.`,
        ],
        antigravity: [`Antigravity cuts each file at 24,000 bytes. AGENTS.md is ${bytes} bytes.`],
      }
    : {};
  return {
    scope: "project",
    folder: "C:/code/auracode",
    mode: "universal",
    source: agents.key,
    default_source: agents.key,
    source_choices: [agents.key],
    files: [
      agents,
      claude,
      file("project:AGENTS.override.md", "AGENTS.override.md", null),
      file("project:GEMINI.md", "GEMINI.md", null),
    ],
    agents: ORDER.map((id) => {
      const warnings = (largeWarnings as Record<string, string[]>)[id] ?? [];
      if (id === "claude")
        return agent(id, "import", claude.key, "Reads CLAUDE.md, which imports AGENTS.md.", {
          via: agents.key,
          warnings,
          override_file: claude.key,
        });
      return agent(id, "direct", agents.key, "Reads AGENTS.md.", {
        shared_with: shared.filter((other) => other !== id),
        warnings,
        override_file:
          id === "codex" || id === "pi"
            ? "project:AGENTS.override.md"
            : id === "antigravity"
              ? "project:GEMINI.md"
              : null,
      });
    }),
    conflict: [],
    nested: [
      { path: "apps/web/AGENTS.md", bytes: 812, readers: ["claude", "codex", "cursor", "grok"] },
    ],
    parents: [],
    notices: [],
    git_repository: true,
    worktree_branch: null,
    revision: `universal-${state.agents.length}`,
  };
}

function perAgentSnapshot(state: State): InstructionsSnapshot {
  const agents = file("project:AGENTS.md", "AGENTS.md", state.agents, {
    readers: ["codex", "pi", "omp", "opencode", "hermes", "cursor", "grok", "antigravity"],
  });
  const claude = file("project:CLAUDE.md", "CLAUDE.md", state.claude, {
    readers: ["claude", "cursor", "grok"],
    git: "untracked",
    note: "Not committed, so new worktree chats won't have it.",
  });
  const override = file("project:AGENTS.override.md", "AGENTS.override.md", state.override, {
    readers: state.override ? ["codex", "pi", "hermes"] : [],
  });
  const conflict = [agents.key, claude.key, ...(state.override ? [override.key] : [])];
  return {
    scope: "project",
    folder: "C:/code/auracode",
    mode: "per_agent",
    source: null,
    default_source: agents.key,
    source_choices: [agents.key],
    files: [agents, claude, override, file("project:GEMINI.md", "GEMINI.md", null)],
    agents: ORDER.map((id) => {
      if (id === "claude")
        return agent(id, "own", claude.key, "Reads CLAUDE.md.", {
          shared_with: ["cursor", "grok"],
          override_file: claude.key,
        });
      if (state.override && (id === "codex" || id === "pi" || id === "hermes"))
        return agent(
          id,
          "override",
          override.key,
          "Reads AGENTS.override.md instead of the shared file.",
          {
            shared_with: ["codex", "pi", "hermes"].filter((other) => other !== id),
            override_file: id === "hermes" ? null : override.key,
          },
        );
      if (id === "cursor" || id === "grok")
        return agent(id, "own", agents.key, "Reads AGENTS.md.", {
          also: [claude.key],
          shared_with: ["omp", "opencode", "antigravity"],
        });
      return agent(id, "own", agents.key, "Reads AGENTS.md.", {
        shared_with: ["omp", "opencode", "cursor", "grok", "antigravity"].filter(
          (other) => other !== id,
        ),
        override_file: id === "antigravity" ? "project:GEMINI.md" : null,
      });
    }),
    conflict,
    nested: [],
    parents: [
      {
        path: "~/code/CLAUDE.md",
        bytes: 214,
        readers: ["claude", "pi", "omp", "opencode", "grok"],
      },
    ],
    notices: [
      "~/code/CLAUDE.md is in a parent folder, so Claude Code reads CLAUDE files here instead of AGENTS.md.",
    ],
    git_repository: true,
    worktree_branch: null,
    revision: `per-agent-${state.agents.length}-${state.claude?.length ?? 0}`,
  };
}

function globalSnapshot(state: State): InstructionsSnapshot {
  const codex = file("codex:AGENTS.md", "~/.codex/AGENTS.md", state.agents, {
    readers: ["codex"],
    git: null,
  });
  const pi = file(
    "pi:AGENTS.md",
    "~/.pi/agent/AGENTS.md",
    state.drift ? `${state.agents}\nAlways answer in British English.\n` : state.agents,
    {
      readers: ["pi"],
      git: null,
      managed: { source: codex.key, edited: state.drift, source_changed: false },
    },
  );
  const opencode = file("opencode:AGENTS.md", "~/.config/opencode/AGENTS.md", state.agents, {
    readers: ["opencode"],
    git: null,
    managed: { source: codex.key, edited: false, source_changed: false },
  });
  const claude = file("claude:CLAUDE.md", "~/.claude/CLAUDE.md", "@~/.codex/AGENTS.md\n", {
    readers: ["claude"],
    git: null,
    import_of: codex.key,
    imports: ["~/.codex/AGENTS.md"],
  });
  return {
    scope: "global",
    folder: null,
    mode: "universal",
    source: codex.key,
    default_source: codex.key,
    source_choices: [codex.key, claude.key, pi.key, opencode.key],
    files: [codex, claude, pi, opencode],
    agents: ORDER.map((id) => {
      switch (id) {
        case "claude":
          return agent(
            id,
            "import",
            claude.key,
            "Reads ~/.claude/CLAUDE.md, which imports ~/.codex/AGENTS.md.",
            {
              via: codex.key,
            },
          );
        case "codex":
          return agent(id, "direct", codex.key, "Reads ~/.codex/AGENTS.md.");
        case "pi":
          return state.drift
            ? agent(
                id,
                "drift",
                pi.key,
                "Reads ~/.pi/agent/AGENTS.md, a copy of ~/.codex/AGENTS.md that was edited by hand.",
                { via: codex.key },
              )
            : agent(
                id,
                "copy",
                pi.key,
                "Reads ~/.pi/agent/AGENTS.md, a copy of ~/.codex/AGENTS.md that Ghost keeps in sync.",
                { via: codex.key },
              );
        case "opencode":
          return agent(
            id,
            "copy",
            opencode.key,
            "Reads ~/.config/opencode/AGENTS.md, a copy of ~/.codex/AGENTS.md that Ghost keeps in sync.",
            { via: codex.key },
          );
        case "hermes":
          return agent(
            id,
            "unsupported",
            null,
            "Hermes has no user-level instructions file. Its SOUL.md sets the agent's identity, not project rules.",
          );
        case "cursor":
          return agent(
            id,
            "unsupported",
            null,
            "Cursor keeps user rules in its settings and in ~/.cursor/rules, not in one file.",
          );
        case "grok":
          // Installed after the shared file was set up: it reads nothing yet and offers Set up.
          return agent(id, "not_set", null, "Reads nothing here yet.", { installed: true });
        default:
          return agent(id, "not_set", null, "Reads nothing here yet.");
      }
    }),
    conflict: [],
    nested: [],
    parents: [],
    notices: [],
    git_repository: false,
    worktree_branch: null,
    revision: `global-${state.drift}`,
  };
}

function snapshotOf(state: State): InstructionsSnapshot {
  if (state.scope === "global") return globalSnapshot(state);
  return state.mode === "universal" ? universalSnapshot(state) : perAgentSnapshot(state);
}

function reviewFile(
  key: string,
  path: string,
  action: InstructionsReviewFile["action"],
  before: string | null,
  after: string | null,
  how: string | null,
  notes: string[] = [],
): InstructionsReviewFile {
  return { key, path, action, before, after, how, notes, backup: action !== "create" };
}

const SESSIONS =
  "Agents read instructions when a session starts. Running chats keep what they started with; new sessions read the new files.";

export function createInstructionsFixture(scenario: InstructionsScenario) {
  const calls: string[] = [];
  const reviews = new Map<string, { change: InstructionsChange; next: Partial<State> }>();
  let counter = 0;
  const large = `${SHARED}\n${"Keep functions small and name things plainly. Prefer the narrowest change that solves it.\n".repeat(460)}`;
  const state: State = {
    scope: scenario === "drift" ? "global" : "project",
    mode: scenario === "per-agent" || scenario === "conflict" ? "per_agent" : "universal",
    agents: scenario === "large" ? large : SHARED,
    claude: scenario === "per-agent" || scenario === "conflict" ? CLAUDE_OWN : null,
    override: scenario === "per-agent" || scenario === "conflict" ? OVERRIDE : null,
    drift: scenario === "drift",
    large: scenario === "large",
  };
  const review = (change: InstructionsChange): InstructionsReview => {
    const id = `review-${++counter}`;
    const snapshot = snapshotOf(state);
    const path = (key: string) => snapshot.files.find((item) => item.key === key)?.path ?? key;
    const text = (key: string) => snapshot.files.find((item) => item.key === key)?.text ?? null;
    let result: InstructionsReview;
    let next: Partial<State> = {};
    switch (change.type) {
      case "save": {
        result = {
          id,
          title: `Save ${path(change.file)}`,
          summary:
            snapshot.mode === "universal"
              ? "Claude Code, Codex, Pi, OpenCode and Cursor read AGENTS.md."
              : `${path(change.file)} is read by the agents named under it.`,
          files: [
            reviewFile(
              change.file,
              path(change.file),
              "replace",
              text(change.file),
              change.text,
              null,
            ),
          ],
          notes: [SESSIONS],
          optional_removals: [],
        };
        next = change.file.endsWith("CLAUDE.md")
          ? { claude: change.text }
          : { agents: change.text };
        break;
      }
      case "universal": {
        const content =
          change.content.type === "combine"
            ? `${SHARED.trimEnd()}\n\n${CLAUDE_OWN.trimEnd()}\n\n${OVERRIDE.trimEnd()}\n`
            : change.content.type === "file"
              ? (text(change.content.file) ?? SHARED)
              : state.agents;
        const files: InstructionsReviewFile[] = [];
        if (content !== state.agents)
          files.push(
            reviewFile(
              "project:AGENTS.md",
              "AGENTS.md",
              "replace",
              state.agents,
              content,
              "Holds the shared text",
            ),
          );
        files.push(
          reviewFile(
            "project:CLAUDE.md",
            "CLAUDE.md",
            "replace",
            state.claude,
            "@AGENTS.md\n",
            "Claude Code imports AGENTS.md",
            ["Its earlier text is backed up in Ghost's data folder."],
          ),
        );
        if (change.remove.includes("project:AGENTS.override.md"))
          files.push(
            reviewFile(
              "project:AGENTS.override.md",
              "AGENTS.override.md",
              "remove",
              state.override,
              null,
              null,
              ["Backed up in Ghost's data folder."],
            ),
          );
        result = {
          id,
          title: "Use one file for every agent",
          summary:
            "AGENTS.md holds the shared text. OpenCode and Cursor read it directly. Claude Code reads it through an import.",
          files,
          notes: [
            ...(change.remove.includes("project:AGENTS.override.md") || !state.override
              ? []
              : [
                  "Codex keeps reading AGENTS.override.md instead of AGENTS.md. Remove it to change that; it is backed up.",
                ]),
            "Cursor and Grok Build read CLAUDE.md as plain text, so they see only its import line next to AGENTS.md.",
            SESSIONS,
          ],
          optional_removals: state.override ? ["project:AGENTS.override.md"] : [],
        };
        next = {
          mode: "universal",
          agents: content,
          claude: null,
          override: change.remove.length ? null : state.override,
        };
        break;
      }
      case "per_agent":
        result = {
          id,
          title: "Use a file per agent",
          summary:
            "Each agent keeps the text it reads now, as a file of its own. AGENTS.md stays as it is.",
          files: [
            reviewFile(
              "project:CLAUDE.md",
              "CLAUDE.md",
              "replace",
              "@AGENTS.md\n",
              state.agents,
              "Own copy of the text in AGENTS.md",
              ["Edit it separately from now on."],
            ),
          ],
          notes: [SESSIONS],
          optional_removals: [],
        };
        next = { mode: "per_agent", claude: state.agents, override: null };
        break;
      case "restore":
      case "adopt": {
        const edited = text("pi:AGENTS.md");
        result = {
          id,
          title:
            change.type === "restore"
              ? "Restore ~/.pi/agent/AGENTS.md"
              : "Use the edit in ~/.pi/agent/AGENTS.md",
          summary:
            change.type === "restore"
              ? "~/.pi/agent/AGENTS.md gets the text of ~/.codex/AGENTS.md again."
              : "~/.codex/AGENTS.md and every managed copy take the text edited in ~/.pi/agent/AGENTS.md.",
          files:
            change.type === "restore"
              ? [
                  reviewFile(
                    "pi:AGENTS.md",
                    "~/.pi/agent/AGENTS.md",
                    "replace",
                    edited,
                    state.agents,
                    "Managed copy of ~/.codex/AGENTS.md",
                    ["The edited text is backed up in Ghost's data folder."],
                  ),
                ]
              : [
                  reviewFile(
                    "codex:AGENTS.md",
                    "~/.codex/AGENTS.md",
                    "replace",
                    state.agents,
                    edited,
                    "Holds the shared text",
                  ),
                  reviewFile(
                    "opencode:AGENTS.md",
                    "~/.config/opencode/AGENTS.md",
                    "replace",
                    state.agents,
                    edited,
                    "Managed copy of ~/.codex/AGENTS.md",
                  ),
                ],
          notes: [SESSIONS],
          optional_removals: [],
        };
        next =
          change.type === "restore"
            ? { drift: false }
            : { drift: false, agents: edited ?? state.agents };
        break;
      }
      case "rename":
        throw new Error("This fixture has no file with another spelling.");
    }
    reviews.set(id, { change, next });
    return result;
  };
  const api: InstructionsApi = {
    snapshot: async () => {
      calls.push("snapshot");
      const snapshot = snapshotOf(state);
      if (scenario === "recovery") {
        snapshot.notices.push(
          "An interrupted Instructions change needs recovery. Unrecognized files will be preserved. Its journal and backups are kept at C:/fixture/auracode/instructions/backups/pending.",
          "An interrupted Instructions change was recovered. Its backup is kept at C:/fixture/auracode/instructions/backups/recovered.",
        );
      }
      return snapshot;
    },
    review: async (_target, change) => {
      calls.push(
        `review:${change.type}${change.type === "universal" ? `:${change.content.type}` : ""}`,
      );
      return review(change);
    },
    cancel: async (id) => {
      calls.push("cancel");
      reviews.delete(id);
    },
    apply: async (id) => {
      calls.push("apply");
      const pending = reviews.get(id);
      if (!pending)
        throw new Error(
          "The item changed after it was displayed. Refresh the Library and try again.",
        );
      reviews.delete(id);
      if (scenario === "recovery") {
        return {
          id,
          changed: [],
          rolled_back: false,
          recovery_required: true,
          message:
            "This change overlaps an interrupted Instructions change that still needs attention. External edits were preserved.",
          backup: "C:/fixture/auracode/instructions/backups/pending",
        };
      }
      Object.assign(state, pending.next);
      return {
        id,
        changed: [],
        rolled_back: false,
        recovery_required: false,
        message: null,
        backup: null,
      };
    },
    includeInWorktrees: async () => {
      calls.push("include");
    },
    commit: async () => {
      calls.push("commit");
      return "4f2a91c";
    },
  };
  return { api, calls: () => [...calls] };
}
