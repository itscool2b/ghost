import type { ProjectShell, ProjectShellChoice } from "../bindings";
import { errorMessage } from "./errors";
import type { ProjectToolsApi } from "./projectTools";
import type { TerminalState, TerminalTransport } from "./terminal";
import { localTerminalMemory, type RememberedTab, type TerminalMemory } from "./terminalMemory";

/** Rust allows eight project shells in the app (ghost-core terminal.rs); the panel says so first. */
export const PANEL_TERMINAL_LIMIT = 8;
/** A third of a 1440x900 window: room for a test run without hiding the conversation. */
export const PANEL_HEIGHT_DEFAULT = 280;
export const PANEL_HEIGHT_MIN = 120;
export const PANEL_HEIGHT_MAX = 800;
/** Room the transcript and the composer keep above the panel. */
export const PANEL_ROOM_ABOVE = 240;
const TITLE_LIMIT = 40;

export type TabStatus = { readonly state: "reviewing" } | TerminalState;

export interface PanelTab {
  readonly purpose?: "setup" | "run";
  readonly id: string;
  readonly chatId: string;
  /** The shell's name ("PowerShell 2") or the name the user gave the tab. */
  readonly title: string;
  readonly named: boolean;
  readonly shell: ProjectShellChoice | null;
  /** The folder Rust reviewed: the chat's own folder, so a worktree chat opens in its worktree. */
  readonly folder: string | null;
  readonly status: TabStatus;
  /** Its shell was asked to start (the tab was shown): only then does it hold a Rust slot. */
  readonly started?: boolean;
  /** A Run tab's address: the worktree's first port on this machine. */
  readonly url?: string;
}

export interface ChatPanel {
  readonly open: boolean;
  readonly height: number;
  readonly tabs: readonly string[];
  readonly active: string | null;
  /** The shells the latest review offered, for the New terminal menu. */
  readonly shells: readonly ProjectShellChoice[];
  /** A refusal that belongs to the panel, not to one tab (the app-wide limit). */
  readonly notice: string | null;
  /** Counts the person's requests to use the terminal (show, a new tab, a tab chosen). */
  readonly focus: number;
  /**
   * What this chat had open when Ghost last closed, offered back until the person resumes
   * or declines it. Nothing starts by itself.
   */
  readonly resume: readonly RememberedTab[] | null;
}

export interface PanelSnapshot {
  readonly chats: ReadonlyMap<string, ChatPanel>;
  readonly tabs: ReadonlyMap<string, PanelTab>;
}

/** One tab's terminal view and process plumbing. The xterm runtime lives in the component layer. */
export interface TerminalRuntime {
  /** Shows the terminal inside `container`; the first call opens the view and starts the shell. */
  attach(container: HTMLElement): void;
  /** Takes the view out of the page. The shell keeps running and its output keeps arriving. */
  detach(): void;
  focus(): void;
  /** Ends the shell (Rust stops its owned processes) and releases the view. */
  dispose(): Promise<void>;
}

export type RuntimeFactory = (options: {
  readonly transport: TerminalTransport;
  readonly label: string;
  readonly onStatus: (status: TerminalState) => void;
}) => TerminalRuntime;

export const CLOSED_PANEL: ChatPanel = {
  open: false,
  height: PANEL_HEIGHT_DEFAULT,
  tabs: [],
  active: null,
  shells: [],
  notice: null,
  focus: 0,
  resume: null,
};

/** Heights land on Graphite's 4px grid, inside the bounds and the room the view has. */
export function clampHeight(height: number, room = PANEL_HEIGHT_MAX): number {
  const max = Math.max(PANEL_HEIGHT_MIN, Math.min(PANEL_HEIGHT_MAX, room));
  const value = Math.max(PANEL_HEIGHT_MIN, Math.min(max, Number.isFinite(height) ? height : 0));
  return Math.floor(value / 4) * 4;
}

/** A shell holds its Rust slot until it exits or fails; finished tabs keep only their output. */
export function holdsShell(status: TabStatus): boolean {
  return status.state === "reviewing" || status.state === "starting" || status.state === "running";
}

/**
 * Whether a tab counts toward the app's limit: a folder check on its way, or a shell that was
 * asked to start. A tab never shown (one a resume made for a chat not on screen) starts nothing
 * until it is, so it holds no slot.
 */
export function countsTowardLimit(tab: PanelTab): boolean {
  return tab.status.state === "reviewing" || (tab.started === true && holdsShell(tab.status));
}

/**
 * What a shell's tab is called: the person's own shell by its program's name ("bash", "fish"), as
 * their terminal app names it, and the others by their label ("Plain Bash", "PowerShell").
 */
export function shellTitle(shell: Pick<ProjectShellChoice, "id" | "label" | "program">): string {
  if (shell.id !== "login") return shell.label;
  const name = shell.program.split(/[\\/]/).pop()?.trim();
  return name ? name : shell.label;
}

/** "PowerShell", then "PowerShell 2" and so on: the lowest number no open tab of this chat uses. */
export function nextTitle(label: string, taken: readonly string[]): string {
  if (!taken.includes(label)) return label;
  for (let number = 2; ; number++) {
    const title = `${label} ${number}`;
    if (!taken.includes(title)) return title;
  }
}

/**
 * The docked terminal panels, one per chat, owned by the chat screen so a chat's shells keep
 * running while another chat is open. Every tab is an explicit user action: a new tab asks Rust for
 * the chat's folder and shells, and the chosen shell opens in that folder when the tab first shows
 * (Rust checks both again then). Nothing runs a command; closing a tab, reloading or quitting
 * stops its shell.
 */
export class TerminalPanels {
  #chats = new Map<string, ChatPanel>();
  #tabs = new Map<string, PanelTab>();
  #runtimes = new Map<string, TerminalRuntime>();
  #listeners = new Set<() => void>();
  #snapshot: PanelSnapshot = { chats: new Map(), tabs: new Map() };
  #sequence = 0;
  /** Chats whose remembered panel this run has looked at, so an offer comes once. */
  #restored = new Set<string>();

  constructor(
    private readonly api: ProjectToolsApi,
    private readonly factory: RuntimeFactory,
    private readonly limit = PANEL_TERMINAL_LIMIT,
    private readonly memory: TerminalMemory = localTerminalMemory,
  ) {}

  subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  getSnapshot = (): PanelSnapshot => this.#snapshot;

  panel(chatId: string): ChatPanel {
    return this.#chats.get(chatId) ?? CLOSED_PANEL;
  }

  runtime(tabId: string): TerminalRuntime | undefined {
    return this.#runtimes.get(tabId);
  }

  /** The reviewed setup uses the normal docked panel, keeping the chat composer available.
   * Process completion belongs to Rust; this tab only renders its output and closes its process. */
  setupTab(chatId: string, transport: TerminalTransport, folder: string): void {
    const id = `setup-${++this.#sequence}`;
    const panel = this.panel(chatId);
    this.#tabs.set(id, {
      id,
      chatId,
      purpose: "setup",
      title: "Setup",
      named: false,
      shell: null,
      folder,
      status: { state: "starting" },
    });
    this.#runtimes.set(
      id,
      this.factory({
        transport,
        label: "Worktree setup terminal",
        onStatus: (status) => {
          if (this.#tabs.has(id)) this.#setTab(id, { status, started: true });
        },
      }),
    );
    this.#setPanel(chatId, {
      ...panel,
      tabs: [...panel.tabs, id],
      // A terminal opened instead of the earlier ones withdraws their offer.
      resume: null,
      active: id,
      open: true,
      notice: null,
      focus: panel.focus + 1,
    });
  }

  /**
   * Run: the command saved in the folder's setup, in a tab of its own in the chat's worktree. A
   * chat runs it once at a time; asking again while it runs shows that tab.
   */
  runTab(chatId: string, transport: TerminalTransport, folder: string, url: string): void {
    const panel = this.panel(chatId);
    const running = this.runningTab(chatId);
    if (running) {
      this.#setPanel(chatId, { ...panel, active: running, open: true, focus: panel.focus + 1 });
      return;
    }
    if (this.#openShells() >= this.limit) {
      this.#setPanel(chatId, {
        ...panel,
        open: true,
        notice: `${this.limit === PANEL_TERMINAL_LIMIT ? "Eight" : this.limit} terminals are open. Close one before starting Run.`,
      });
      return;
    }
    const id = `run-${++this.#sequence}`;
    this.#tabs.set(id, {
      id,
      chatId,
      purpose: "run",
      title: "Run",
      named: false,
      shell: null,
      folder,
      url,
      status: { state: "starting" },
    });
    this.#runtimes.set(
      id,
      this.factory({
        transport,
        label: "Run terminal",
        onStatus: (status) => {
          if (this.#tabs.has(id)) this.#setTab(id, { status, started: true });
        },
      }),
    );
    this.#setPanel(chatId, {
      ...panel,
      tabs: [...panel.tabs, id],
      // A terminal opened instead of the earlier ones withdraws their offer.
      resume: null,
      active: id,
      open: true,
      notice: null,
      focus: panel.focus + 1,
    });
    this.#remember(chatId);
  }

  /**
   * The first time a chat shows in this run: what its panel had open when Ghost last closed,
   * offered back, the panel open as it was. Nothing starts until the person resumes.
   */
  restore(chatId: string): void {
    if (this.#restored.has(chatId)) return;
    this.#restored.add(chatId);
    const panel = this.panel(chatId);
    if (panel.tabs.length > 0) return;
    const remembered = this.memory.read(chatId);
    if (!remembered) return;
    this.#setPanel(chatId, { ...panel, resume: remembered.tabs, open: remembered.open });
  }

  /** Starts the offered terminals again in their order: shells by name, and Run through `onRun`. */
  async resume(chatId: string, onRun?: () => void): Promise<void> {
    const offer = this.panel(chatId).resume;
    if (!offer) return;
    this.#setPanel(chatId, { ...this.panel(chatId), resume: null, open: true });
    for (const tab of offer) {
      if (tab.kind === "run") {
        onRun?.();
        continue;
      }
      const id = await this.newTab(chatId, tab.shell);
      if (id && tab.named) this.rename(id, tab.title);
    }
  }

  /** Declines the offer; the chat's panel is remembered afresh from what opens next. */
  dismissResume(chatId: string): void {
    const panel = this.panel(chatId);
    if (!panel.resume) return;
    this.#setPanel(chatId, { ...panel, resume: null });
    this.#remember(chatId);
  }

  /** The chat's Run tab while its process runs, if there is one. */
  runningTab(chatId: string): string | null {
    return (
      this.panel(chatId).tabs.find((id) => {
        const tab = this.#tabs.get(id);
        return tab?.purpose === "run" && holdsShell(tab.status);
      }) ?? null
    );
  }

  /** Opens setup review without starting a shell. Existing terminal output remains in its tab. */
  showReview(chatId: string): void {
    const panel = this.panel(chatId);
    this.#setPanel(chatId, { ...panel, active: null, open: true, focus: panel.focus + 1 });
  }

  /** Ctrl+`: hides an open panel, or shows it, opening the first terminal when it has none. */
  toggle(chatId: string): void {
    if (this.panel(chatId).open) this.hide(chatId);
    else this.show(chatId);
  }

  show(chatId: string): void {
    const panel = this.panel(chatId);
    this.#setPanel(chatId, {
      ...panel,
      active: panel.active ?? panel.tabs.at(-1) ?? null,
      open: true,
      focus: panel.focus + 1,
    });
    // An offer waiting shows instead of a new shell.
    if (panel.tabs.length === 0 && !panel.resume) void this.newTab(chatId);
    else this.#remember(chatId);
  }

  hide(chatId: string): void {
    const panel = this.panel(chatId);
    if (panel.open) this.#setPanel(chatId, { ...panel, open: false, notice: null });
    this.#remember(chatId);
  }

  /**
   * Reviews the chat's folder and starts the chosen shell (the first one Rust offers by default).
   * Answers the new tab, or `null` when the app's limit refused one.
   */
  async newTab(chatId: string, shell?: ProjectShell): Promise<string | null> {
    const panel = this.panel(chatId);
    if (this.#openShells() >= this.limit) {
      this.#setPanel(chatId, {
        ...panel,
        open: true,
        notice: `${this.limit === PANEL_TERMINAL_LIMIT ? "Eight" : this.limit} terminals are open. Close one before opening another.`,
      });
      return null;
    }
    const id = `terminal-${++this.#sequence}`;
    this.#tabs.set(id, {
      id,
      chatId,
      title: "Terminal",
      named: false,
      shell: null,
      folder: null,
      status: { state: "reviewing" },
    });
    this.#setPanel(chatId, {
      ...panel,
      open: true,
      tabs: [...panel.tabs, id],
      // A terminal opened instead of the earlier ones withdraws their offer.
      resume: null,
      active: id,
      notice: null,
      focus: panel.focus + 1,
    });
    try {
      const review = await this.api.terminalReview(chatId);
      const tab = this.#tabs.get(id);
      if (!tab) return null;
      const choice = review.shells.find((item) => item.id === shell) ?? review.shells[0];
      this.#setPanel(chatId, { ...this.panel(chatId), shells: review.shells });
      if (!choice) {
        this.#setTab(id, {
          folder: review.folder,
          status: { state: "failed", message: "No supported native shell was found." },
        });
        return id;
      }
      const taken = this.panel(chatId)
        .tabs.filter((other) => other !== id)
        .map((other) => this.#tabs.get(other)?.title ?? "");
      const title = nextTitle(shellTitle(choice), taken);
      // The shell opens in one step when the tab first shows (Rust checks the folder and the
      // shell again then), so a tab shown long after this review still opens.
      this.#runtimes.set(
        id,
        this.factory({
          transport: this.api.terminal(chatId, choice.id),
          label: `${title} terminal`,
          onStatus: (status) => {
            if (this.#tabs.has(id)) this.#setTab(id, { status, started: true });
          },
        }),
      );
      this.#setTab(id, {
        title,
        shell: choice,
        folder: review.folder,
        status: { state: "starting" },
      });
      this.#remember(chatId);
    } catch (error) {
      if (this.#tabs.has(id))
        this.#setTab(id, { status: { state: "failed", message: errorMessage(error) } });
    }
    return id;
  }

  /** Shows a tab. A click moves focus into its shell; arrow keys leave it on the tabs. */
  select(chatId: string, tabId: string, focus = true): void {
    const panel = this.panel(chatId);
    if (panel.tabs.includes(tabId))
      this.#setPanel(chatId, { ...panel, active: tabId, focus: panel.focus + (focus ? 1 : 0) });
  }

  /** Closes the tab and stops its shell. The last tab closing hides the panel. */
  async closeTab(tabId: string): Promise<void> {
    const tab = this.#tabs.get(tabId);
    if (!tab) return;
    const panel = this.panel(tab.chatId);
    const index = panel.tabs.indexOf(tabId);
    const tabs = panel.tabs.filter((other) => other !== tabId);
    const active =
      panel.active === tabId ? (tabs[Math.min(index, tabs.length - 1)] ?? null) : panel.active;
    this.#tabs.delete(tabId);
    const runtime = this.#runtimes.get(tabId);
    this.#runtimes.delete(tabId);
    this.#setPanel(tab.chatId, {
      ...panel,
      tabs,
      active,
      open: tabs.length > 0 && panel.open,
      notice: null,
    });
    this.#remember(tab.chatId);
    await runtime?.dispose();
  }

  /** A name of up to 40 characters; an empty name gives the tab its shell's name back. */
  rename(tabId: string, title: string): void {
    const tab = this.#tabs.get(tabId);
    if (!tab) return;
    const name = title.replace(/\s+/g, " ").trim().slice(0, TITLE_LIMIT).trim();
    if (name) {
      this.#setTab(tabId, { title: name, named: true });
      this.#remember(tab.chatId);
      return;
    }
    const taken = this.panel(tab.chatId)
      .tabs.filter((other) => other !== tabId)
      .map((other) => this.#tabs.get(other)?.title ?? "");
    this.#setTab(tabId, {
      title: tab.shell
        ? nextTitle(shellTitle(tab.shell), taken)
        : tab.purpose === "setup"
          ? "Setup"
          : tab.purpose === "run"
            ? "Run"
            : "Terminal",
      named: false,
    });
    this.#remember(tab.chatId);
  }

  setHeight(chatId: string, height: number, room?: number): void {
    const panel = this.panel(chatId);
    const next = clampHeight(height, room);
    if (next !== panel.height) this.#setPanel(chatId, { ...panel, height: next });
  }

  /** Stops every shell. The panels stay usable, so a remount starts clean. */
  async dispose(): Promise<void> {
    const runtimes = [...this.#runtimes.values()];
    this.#runtimes.clear();
    this.#tabs.clear();
    this.#chats.clear();
    this.#publish();
    await Promise.all(runtimes.map((runtime) => runtime.dispose()));
  }

  /**
   * Keeps what the chat's panel has open for the next run: its live shells by shell and name, Run,
   * and whether it shows. Only the person's own actions call this, never a shell's exit, so
   * quitting (which stops every shell) leaves the panel as it was.
   */
  #remember(chatId: string): void {
    const panel = this.panel(chatId);
    // An offer not taken yet keeps the memory it came from.
    if (panel.resume) return;
    const tabs: RememberedTab[] = [];
    for (const id of panel.tabs) {
      const tab = this.#tabs.get(id);
      if (!tab || !holdsShell(tab.status)) continue;
      if (tab.purpose === "run") tabs.push({ kind: "run" });
      else if (!tab.purpose && tab.shell)
        tabs.push({ kind: "shell", shell: tab.shell.id, title: tab.title, named: tab.named });
    }
    this.memory.write(
      chatId,
      tabs.length > 0 ? { tabs, open: panel.open, savedAt: Date.now() } : null,
    );
  }

  /** Shells holding a Rust slot, or about to: what the app's limit counts. */
  #openShells(): number {
    return [...this.#tabs.values()].filter(countsTowardLimit).length;
  }

  #setTab(tabId: string, change: Partial<PanelTab>): void {
    const tab = this.#tabs.get(tabId);
    if (!tab) return;
    this.#tabs.set(tabId, { ...tab, ...change });
    this.#publish();
  }

  #setPanel(chatId: string, panel: ChatPanel): void {
    this.#chats.set(chatId, panel);
    this.#publish();
  }

  #publish(): void {
    this.#snapshot = { chats: new Map(this.#chats), tabs: new Map(this.#tabs) };
    for (const listener of this.#listeners) listener();
  }
}
