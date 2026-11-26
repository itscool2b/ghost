import type { ChatTerminalStarted, TerminalChunk } from "../../bindings";
import {
  chatTerminalEnded,
  chatTerminalReserve,
  chatTerminalStart,
  terminalClose,
  terminalRead,
  terminalResize,
  terminalWrite,
} from "../../lib/ipc";
import { TerminalSession, type TerminalSize, type TerminalState } from "../../lib/terminal";
import { TerminalActivity, type TerminalUse } from "../../lib/terminalActivity";
import { createXterm, loadXterm, type XtermView } from "../xterm";
import { shellKeys } from "./terminalRuntime";

/** What a terminal chat's view shows: the terminal's state and what its launch left out. */
export interface TerminalChatSnapshot {
  readonly state: TerminalState | { readonly state: "idle" };
  /** Shared servers its agent's terminal could not take, with why. */
  readonly omitted: readonly string[];
  /**
   * A link clicked in the terminal, shown in full before it opens: terminal output is any
   * program's, so its link text can say one address and lead to another.
   */
  readonly link: string | null;
}

/** The IPC a terminal chat uses; tests replace it. */
export interface TerminalChatTransport {
  reserve(): Promise<string>;
  start(terminalId: string, chatId: string, size: TerminalSize): Promise<ChatTerminalStarted>;
  read(terminalId: string): Promise<TerminalChunk>;
  write(terminalId: string, bytes: number[]): Promise<void>;
  resize(terminalId: string, size: TerminalSize): Promise<void>;
  close(terminalId: string): Promise<void>;
  /** The chat gets its session back. */
  ended(terminalId: string): Promise<void>;
}

export const chatTerminalTransport: TerminalChatTransport = {
  // Terminal chats have a pool of their own: a chat left running never refuses the next one.
  reserve: chatTerminalReserve,
  start: (id, chatId, size) => chatTerminalStart(id, chatId, size.rows, size.cols),
  read: terminalRead,
  write: terminalWrite,
  resize: (id, size) => terminalResize(id, size.rows, size.cols),
  close: terminalClose,
  ended: chatTerminalEnded,
};

/** The agent's interface counts as ready once its output has been quiet this long. */
export const SETTLED_MS = 900;
/** The first input waits at most this long for the interface to settle, then goes anyway. */
export const FIRST_INPUT_WAIT_MS = 20_000;

/** How long a terminal chat's agent gets to exit by itself before its processes are closed. */
export const EXIT_GRACE_MS = 2500;

/**
 * One chat's terminal: its view and its agent's process, kept while the person looks at another
 * chat (the view detaches; the agent keeps working) and closed only when asked. Closing asks the
 * agent to exit first, as its own interface exits (Ctrl+C twice for most agents; the keys come
 * from Rust with the launch), so it finishes its session's record, then closes its processes if it
 * has not exited after `EXIT_GRACE_MS`.
 */
export class TerminalChatRuntime {
  readonly host: HTMLDivElement;
  readonly #chatId: string;
  readonly #label: string;
  readonly #transport: TerminalChatTransport;
  readonly #listeners = new Set<() => void>();
  #snapshot: TerminalChatSnapshot = { state: { state: "idle" }, omitted: [], link: null };
  #view: XtermView | null = null;
  #session: TerminalSession | null = null;
  #opening: Promise<void> | null = null;
  #observer: ResizeObserver | null = null;
  #cleanups: (() => void)[] = [];
  #disposed = false;
  #wantsFocus = false;
  #exited: (() => void) | null = null;
  /** What asks this agent's interface to exit, one key at a time. */
  #exitInput = "";
  #lastOutput = 0;
  #activity = new TerminalActivity();

  constructor(chatId: string, label: string, transport: TerminalChatTransport) {
    this.#chatId = chatId;
    this.#label = label;
    this.#transport = transport;
    this.host = document.createElement("div");
    this.host.className = "terminal-chat-host";
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  readonly getSnapshot = (): TerminalChatSnapshot => this.#snapshot;

  #set(next: Partial<TerminalChatSnapshot>): void {
    this.#snapshot = { ...this.#snapshot, ...next };
    if (this.#snapshot.state.state === "finished" || this.#snapshot.state.state === "failed") {
      this.#exited?.();
      this.#exited = null;
    }
    for (const listener of [...this.#listeners]) listener();
  }

  /** Shows the terminal in `container`, opening it (and starting the agent) the first time. */
  attach(container: HTMLElement): void {
    if (this.#disposed) return;
    if (this.host.parentElement !== container) container.appendChild(this.host);
    if (!this.#opening) {
      this.#opening = this.#open().catch(() => {
        if (!this.#disposed)
          this.#set({ state: { state: "failed", message: "The terminal view could not load." } });
      });
    } else if (this.#view) {
      this.#view.fit();
      this.#view.term.refresh(0, this.#view.term.rows - 1);
    }
  }

  /** Puts away the link shown for confirmation. */
  dismissLink(): void {
    if (this.#snapshot.link !== null) this.#set({ link: null });
  }

  detach(): void {
    this.host.remove();
  }

  focus(): void {
    if (this.#view) this.#view.term.focus();
    else this.#wantsFocus = true;
  }

  /**
   * What the terminal is doing: its agent starting or drawing output (working), its agent quiet
   * at its prompt (open), or nothing running.
   */
  use(now = Date.now()): TerminalUse {
    const { state } = this.#snapshot.state;
    if (state === "starting") return "working";
    if (state !== "running") return null;
    return this.#activity.working(now) ? "working" : "open";
  }

  async #open(): Promise<void> {
    const modules = await loadXterm();
    if (this.#disposed) return;
    // xterm measures its cells as it opens: a view that left the page opens on its next attach.
    if (!this.host.isConnected) {
      this.#opening = null;
      return;
    }
    const view = createXterm(modules, this.host, {
      inputLabel: `${this.#label} terminal input`,
      keys: (event) => shellKeys(event, () => view.term.hasSelection()),
      variant: "chat",
      onLink: (url) => this.#set({ link: url }),
    });
    this.#view = view;
    const term = view.term;
    const transport = this.#transport;
    const session = new TerminalSession(
      {
        reserve: () => transport.reserve(),
        start: async (id, chatId, size) => {
          const started = await transport.start(id, chatId, size);
          this.#set({ omitted: started.omitted });
          if (started.exit_input) this.#exitInput = started.exit_input;
          if (started.first_input) void this.#typeWhenSettled(started.first_input);
          return started.terminal_id;
        },
        read: (id) => transport.read(id),
        write: (id, bytes) => transport.write(id, bytes),
        resize: (id, size) => transport.resize(id, size),
        // Closing always gives the chat its session back, whether the agent exited or not.
        close: (id) => transport.close(id).finally(() => transport.ended(id)),
      },
      (bytes) => {
        this.#lastOutput = Date.now();
        this.#activity.output(this.#lastOutput);
        return new Promise<void>((resolve) => term.write(bytes, resolve));
      },
      (state) => {
        if (this.#disposed) return;
        term.options.disableStdin = state.state !== "running";
        this.#set({ state });
      },
    );
    this.#session = session;
    const encoder = new TextEncoder();
    // What the person types or resizes makes the agent redraw; that redraw is not its work.
    const poke = () => this.#activity.poke(Date.now());
    const subscriptions = [
      term.onData((text) => {
        poke();
        session.write(encoder.encode(text));
      }),
      term.onBinary((text) => {
        poke();
        session.write(Uint8Array.from(text, (char) => char.charCodeAt(0) & 255));
      }),
      term.onResize(({ rows, cols }) => {
        poke();
        void session.resize({ rows, cols });
      }),
    ];
    this.#cleanups.push(() => {
      for (const subscription of subscriptions) subscription.dispose();
    });
    this.#observer = new ResizeObserver(() => {
      if (this.host.isConnected) view.fit();
    });
    this.#observer.observe(this.host);
    view.fit();
    if (this.#wantsFocus && this.host.isConnected) term.focus();
    this.#wantsFocus = false;
    void session.start(this.#chatId, { rows: term.rows, cols: term.cols });
  }

  /**
   * Pastes `text` into the agent's input once its interface has drawn and gone quiet, as one
   * bracketed paste so its line breaks stay part of the message. The person sends it.
   */
  async #typeWhenSettled(text: string): Promise<void> {
    const started = Date.now();
    for (;;) {
      await new Promise((resolve) => setTimeout(resolve, 150));
      if (this.#disposed || !this.#session) return;
      const quiet = this.#lastOutput > 0 && Date.now() - this.#lastOutput >= SETTLED_MS;
      if (quiet || Date.now() - started >= FIRST_INPUT_WAIT_MS) break;
    }
    if (this.#snapshot.state.state !== "running") return;
    // Rust already dropped every control character (ghost_drivers::terminal::typed_text), so the
    // text cannot end the paste early and act as keys. It is pasted, never sent: the person reads
    // it and presses Enter, so no prompt on screen (a folder-trust question, say) is answered
    // for them.
    const encoder = new TextEncoder();
    this.#activity.poke(Date.now());
    this.#session?.write(encoder.encode(`\u001b[200~${text}\u001b[201~`));
  }

  /** Starts the agent again after it exited or failed, in a fresh view. */
  async restart(): Promise<void> {
    if (this.#disposed) return;
    const container = this.host.parentElement;
    await this.#teardown();
    this.#snapshot = { state: { state: "idle" }, omitted: [], link: null };
    this.#activity = new TerminalActivity();
    this.#opening = null;
    if (container) this.attach(container);
  }

  /**
   * Ends the terminal: asks the agent to exit, waits `EXIT_GRACE_MS` for it, then closes its
   * processes. The chat gets its session back either way.
   */
  async close(grace = EXIT_GRACE_MS): Promise<void> {
    if (this.#disposed) return;
    this.#disposed = true;
    const session = this.#session;
    if (session && this.#snapshot.state.state === "running") {
      const exited = new Promise<void>((resolve) => {
        this.#exited = resolve;
      });
      const keys = [...this.#exitInput];
      for (const [index, key] of keys.entries()) {
        if (index > 0) await new Promise((resolve) => setTimeout(resolve, 150));
        session.write(new TextEncoder().encode(key));
      }
      await Promise.race([exited, new Promise((resolve) => setTimeout(resolve, grace))]);
    }
    await this.#teardown();
    for (const listener of [...this.#listeners]) listener();
  }

  async #teardown(): Promise<void> {
    this.#observer?.disconnect();
    this.#observer = null;
    for (const cleanup of this.#cleanups.splice(0)) cleanup();
    const closing = this.#session?.close();
    this.#session = null;
    this.#view?.dispose();
    this.#view = null;
    this.host.replaceChildren();
    await closing;
  }
}

/** The window's terminal chats, by chat: each lives until it is closed, across navigation. */
export class TerminalChats {
  readonly #runtimes = new Map<string, TerminalChatRuntime>();
  readonly #transport: TerminalChatTransport;

  constructor(transport: TerminalChatTransport = chatTerminalTransport) {
    this.#transport = transport;
  }

  /** The chat's terminal, made on first use. */
  get(chatId: string, label: string): TerminalChatRuntime {
    let runtime = this.#runtimes.get(chatId);
    if (!runtime) {
      runtime = new TerminalChatRuntime(chatId, label, this.#transport);
      this.#runtimes.set(chatId, runtime);
    }
    return runtime;
  }

  has(chatId: string): boolean {
    return this.#runtimes.has(chatId);
  }

  /** What the chat's terminal is doing now (see `TerminalChatRuntime.use`). */
  readonly use = (chatId: string, now = Date.now()): TerminalUse =>
    this.#runtimes.get(chatId)?.use(now) ?? null;

  /** Ends the chat's terminal (switching to Chat, another agent, or deleting the chat). */
  async close(chatId: string): Promise<void> {
    const runtime = this.#runtimes.get(chatId);
    this.#runtimes.delete(chatId);
    await runtime?.close();
  }
}

export const terminalChats = new TerminalChats();
