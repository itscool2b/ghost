/**
 * Which terminals each chat's docked panel had open, kept so that after Ghost restarts the
 * panel can offer them back. Only the shell, the tab's name and whether Run was going are kept:
 * never a command, never output (the panel says output is not saved, and it is not).
 */
import type { ProjectShell } from "../bindings";

export type RememberedTab =
  | {
      readonly kind: "shell";
      readonly shell: ProjectShell;
      readonly title: string;
      /** The person named it; otherwise the shell's own numbered name comes back. */
      readonly named: boolean;
    }
  | { readonly kind: "run" };

export interface RememberedPanel {
  readonly tabs: readonly RememberedTab[];
  /** The panel was showing, so it shows again with the offer. */
  readonly open: boolean;
  readonly savedAt: number;
}

export interface TerminalMemory {
  read(chatId: string): RememberedPanel | null;
  /** `null` forgets the chat's panel. */
  write(chatId: string, panel: RememberedPanel | null): void;
}

const PREFIX = "auracode.terminals.";
/** An offer older than this is no longer what anyone expects back. */
const MAX_AGE_MS = 30 * 24 * 60 * 60 * 1000;
const SHELLS: ReadonlySet<string> = new Set([
  "login",
  "power_shell",
  "command_prompt",
  "zsh",
  "bash",
]);
const MAX_TABS = 8;
const MAX_TITLE = 40;

/** A stored panel, checked field by field: anything malformed or stale reads as nothing. */
export function parseRemembered(text: string | null, now: number): RememberedPanel | null {
  if (!text) return null;
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const { tabs, open, savedAt } = value as Record<string, unknown>;
  if (!Array.isArray(tabs) || typeof savedAt !== "number" || now - savedAt > MAX_AGE_MS)
    return null;
  const kept: RememberedTab[] = [];
  for (const tab of tabs.slice(0, MAX_TABS)) {
    if (typeof tab !== "object" || tab === null) continue;
    const entry = tab as Record<string, unknown>;
    if (entry.kind === "run") kept.push({ kind: "run" });
    else if (
      entry.kind === "shell" &&
      typeof entry.shell === "string" &&
      SHELLS.has(entry.shell) &&
      typeof entry.title === "string" &&
      entry.title.length > 0 &&
      entry.title.length <= MAX_TITLE
    )
      kept.push({
        kind: "shell",
        shell: entry.shell as ProjectShell,
        title: entry.title,
        named: entry.named === true,
      });
  }
  return kept.length > 0 ? { tabs: kept, open: open === true, savedAt } : null;
}

/** The window's own storage; a page without one (tests, a locked-down webview) remembers nothing. */
export const localTerminalMemory: TerminalMemory = {
  read: (chatId) => {
    try {
      return parseRemembered(window.localStorage.getItem(PREFIX + chatId), Date.now());
    } catch {
      return null;
    }
  },
  write: (chatId, panel) => {
    try {
      if (panel === null || panel.tabs.length === 0)
        window.localStorage.removeItem(PREFIX + chatId);
      else window.localStorage.setItem(PREFIX + chatId, JSON.stringify(panel));
    } catch {
      // Storage full or unavailable: the next run simply has nothing to offer.
    }
  },
};

/** "PowerShell, Tests and Run": what the offer names. */
export function rememberedNames(tabs: readonly RememberedTab[]): string {
  const names = tabs.map((tab) => (tab.kind === "run" ? "Run" : tab.title));
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}
