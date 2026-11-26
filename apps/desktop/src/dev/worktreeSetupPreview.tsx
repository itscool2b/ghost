import { createRoot } from "react-dom/client";
import type { TerminalChunk, WorktreeSetupInput } from "../bindings";
import { Modal } from "../components/chat/Modal";
import { WorktreeSetupPanel } from "../components/chat/WorktreeSetupPanel";
import type { WorktreeSetupApi } from "../lib/worktreeSetup";
import "../components/chat/git-dialog.css";

let saved: WorktreeSetupInput = { files: [], command: null, run: null };
const calls: string[] = [];
let read: ((chunk: TerminalChunk) => void) | null = null;
const api: WorktreeSetupApi = {
  read: async () => ({ source: "C:/Projects/orbit", input: saved }),
  save: async (_chatId, input) => {
    calls.push("save");
    saved = input;
    Object.assign(window, { setupSaved: input });
    return { source: "C:/Projects/orbit", input };
  },
  preview: async (_chatId, worktreeId, commandOnly) => {
    calls.push(commandOnly ? "command-review" : "review");
    return {
      token: "fixture-review",
      worktree_id: worktreeId,
      source: "C:/Projects/orbit",
      destination: "C:/Projects/worktrees/orbit",
      files: commandOnly ? [] : saved.files.map((path) => ({ path, bytes: 42 })),
      command: saved.command
        ? { ...saved.command, program: "C:/Program Files/nodejs/node.exe" }
        : null,
      warnings: [
        "Existing destination files are kept.",
        "The command starts only after your explicit Start action.",
      ],
      expires_at: Date.now() + 600000,
    };
  },
  apply: async () => {
    calls.push("apply");
    if (new URLSearchParams(location.search).get("case") === "stale")
      throw new Error("A setup file changed. Review it again.");
    if (new URLSearchParams(location.search).get("case") === "partial")
      return {
        copied: saved.files.slice(0, 1),
        command_token: null,
        warning:
          "One file was copied. Another destination appeared, so setup stopped. Review the worktree before retrying.",
      };
    return {
      copied: saved.files,
      command_token: saved.command ? "fixture-command" : null,
      warning: null,
    };
  },
  terminal: () => ({
    reserve: async () => {
      calls.push("reserve");
      return "terminal";
    },
    start: async () => {
      calls.push("start");
      return "terminal";
    },
    read: () =>
      new Promise((resolve) => {
        read = resolve;
      }),
    write: async () => {},
    resize: async () => {},
    close: async () => {
      calls.push("close");
      read?.({ bytes: [], done: true, exit_code: null, truncated: false });
    },
  }),
};
export function mountWorktreeSetupPreview(container: HTMLElement) {
  Object.assign(window, { setupCalls: calls });
  // The panel lives in the Git workspace dialog, so the fixture shows it there.
  createRoot(container).render(
    <main className="g-root">
      <h1 className="sr-only">Worktree setup fixture</h1>
      <Modal title="Git workspace" size="lg" className="git-dialog" onClose={() => {}}>
        <div className="git-dialog-body">
          <WorktreeSetupPanel
            chatId="fixture-chat"
            worktreeId="fixture-tree"
            branch="ghost/orbit"
            api={api}
            onBack={() => {}}
          />
        </div>
      </Modal>
    </main>,
  );
}
