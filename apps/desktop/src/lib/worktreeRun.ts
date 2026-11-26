/**
 * Run in a worktree chat: the command saved in its folder's setup, started in the chat's worktree in
 * the docked terminal panel, with the worktree's first port in GHOST_PORT and PORT. Rust builds
 * and starts the process (`worktree_run_start`); nothing here constructs a command.
 */
import { commands, type SetupCommand, type WorktreeRun } from "../bindings";
import { settle } from "./ipc";
import { type TerminalTransport, terminalTransport } from "./terminal";

export interface WorktreeRunApi {
  /** The chat's Run command and port; `null` when it does not work in a worktree of its own. */
  info(chatId: string): Promise<WorktreeRun | null>;
  terminal(chatId: string): TerminalTransport;
}

export const worktreeRunApi: WorktreeRunApi = {
  info: (chatId) => settle(commands.worktreeRun(chatId)),
  terminal: (chatId) => ({
    ...terminalTransport,
    // Run takes one of the project shells' slots, eight in the app.
    reserve: () => settle(commands.projectTerminalReserve()),
    start: (id, _agent, size) =>
      settle(commands.worktreeRunStart(chatId, id, size.rows, size.cols)),
  }),
};

/** Longer labels end in an ellipsis: the button's tooltip, not the whole script. */
const LABEL_LIMIT = 80;

/** "pnpm dev": the program's name without its folder or `.exe`, then its arguments. */
export function runLabel(command: SetupCommand): string {
  const name = (command.program.split(/[\\/]/).at(-1) ?? command.program).replace(/\.exe$/i, "");
  const label = [name, ...command.args].join(" ");
  return label.length > LABEL_LIMIT ? `${label.slice(0, LABEL_LIMIT - 1).trimEnd()}…` : label;
}
