/**
 * The game beside the chat (docs/game.md): its commands, its status as a store for React, each
 * chat's pane on this device, and the keys and pointer the pane sends it. Rust owns the games;
 * the window shows one and sends it input.
 */
import { Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  type ChatGame,
  commands,
  type GameEngine,
  type GameInput,
  type GameLog,
  type GameStatus,
  type GameToolsChange,
} from "../bindings";
import { settle } from "./ipc";

export const GAME_EVENT = "ghost-game";

/** Everything the pane asks of the games. The fixture implements it without an engine. */
export interface GameApi {
  status(): Promise<GameStatus>;
  /** What the chat's folder holds now: cheap, and it starts nothing. */
  look(chatId: string): Promise<GameStatus>;
  /** The pane was opened: looks for Godot again, and at the chat's folder. */
  refresh(chatId: string): Promise<GameStatus>;
  /** The person pressed Get Godot. */
  fetchStart(): Promise<void>;
  fetchCancel(): Promise<void>;
  run(chatId: string, scene: string | null): Promise<void>;
  stop(chatId: string): Promise<void>;
  time(chatId: string, frozen: boolean | null, speed: number | null): Promise<void>;
  /** One frame of game time while paused. */
  step(chatId: string): Promise<void>;
  sound(chatId: string, on: boolean): Promise<void>;
  input(chatId: string, events: readonly GameInput[]): Promise<void>;
  /** Streams the chat's game to `onPacket`; answers the stop. */
  watch(chatId: string, onPacket: (packet: Uint8Array) => void): Promise<() => void>;
  log(chatId: string): Promise<GameLog>;
  /** A window of the game's own, or back out of sight. A running game starts again. */
  window(chatId: string, windowed: boolean): Promise<void>;
  openEditor(chatId: string): Promise<void>;
  /** Saves a picture of the game; answers where it is. */
  picture(chatId: string): Promise<string>;
  /** Turns Game on for this chat and gives its agent the tools. */
  turnOn(chatId: string, on: boolean): Promise<GameToolsChange>;
  turnedOn(chatId: string): Promise<boolean>;
  /** Every change to the status; answers the unsubscribe. */
  listen(listener: (status: GameStatus) => void): () => void;
}

const done = async (call: Promise<unknown>): Promise<void> => {
  await call;
};

export const tauriGameApi: GameApi = {
  status: () => settle(commands.gameStatus()),
  look: (chatId) => settle(commands.gameLook(chatId)),
  refresh: (chatId) => settle(commands.gameRefresh(chatId)),
  fetchStart: () => done(settle(commands.gameFetchStart())),
  fetchCancel: () => done(settle(commands.gameFetchCancel())),
  run: (chatId, scene) => done(settle(commands.gameRun(chatId, scene))),
  stop: (chatId) => done(settle(commands.gameStop(chatId))),
  time: (chatId, frozen, speed) => done(settle(commands.gameTime(chatId, frozen, speed))),
  step: (chatId) => done(settle(commands.gameStep(chatId))),
  sound: (chatId, on) => done(settle(commands.gameSound(chatId, on))),
  input: (chatId, events) => done(settle(commands.gameInput(chatId, [...events]))),
  watch: async (chatId, onPacket) => {
    // The packets arrive as raw bytes (an ArrayBuffer), whatever the generated type says.
    const channel = new Channel<number[]>((message) => {
      const raw = message as unknown;
      if (raw instanceof ArrayBuffer) onPacket(new Uint8Array(raw));
      else if (Array.isArray(raw)) onPacket(Uint8Array.from(raw as number[]));
    });
    // The stream's own number: stopping it can never end one that replaced it.
    const watch = await settle(commands.gameWatch(chatId, channel));
    return () => {
      void commands.gameUnwatch(chatId, watch).catch(() => {});
    };
  },
  log: (chatId) => settle(commands.gameLog(chatId)),
  window: (chatId, windowed) => done(settle(commands.gameWindow(chatId, windowed))),
  openEditor: (chatId) => done(settle(commands.gameOpenEditor(chatId))),
  picture: (chatId) => settle(commands.gamePicture(chatId)),
  turnOn: (chatId, on) => settle(commands.gameTurnOn(chatId, on)),
  turnedOn: (chatId) => settle(commands.gameTurnedOn(chatId)),
  listen: (listener) => {
    let stop: (() => void) | null = null;
    let stopped = false;
    void listen<GameStatus>(GAME_EVENT, (event) => listener(event.payload)).then(
      (unlisten) => {
        if (stopped) unlisten();
        else stop = unlisten;
      },
      () => {
        // Outside the app (a fixture): no events.
      },
    );
    return () => {
      stopped = true;
      stop?.();
    };
  },
};

const NOTHING: GameStatus = {
  enabled: false,
  godot: {
    pinned: "",
    size: 0,
    fetched: false,
    found: [],
    looked: false,
    fetch: { state: "idle" },
  },
  chats: [],
  summons: [],
};

/**
 * Calls closer together than this are one stretch of the agent's work: a pane the person closed
 * during it stays closed until the agent has been quiet this long.
 */
export const SUMMON_QUIET_MS = 60_000;

/** The games as React reads them: one status, replaced whole on every change. */
export class GameStore {
  readonly api: GameApi;
  readonly #listeners = new Set<() => void>();
  readonly #now: () => number;
  #status: GameStatus | null = null;
  #version = 0;
  #stop: (() => void) | null = null;
  /** The latest summon number seen for each chat. */
  readonly #seen = new Map<string, number>();
  /** Chats whose agent called on the game since their pane last answered. */
  readonly #pending = new Set<string>();
  /** When each chat's agent last called, and the chats whose pane the person closed meanwhile. */
  readonly #called = new Map<string, number>();
  readonly #closed = new Set<string>();
  /** Chats being looked at now, so a burst of turn ends asks once. */
  readonly #looking = new Set<string>();

  constructor(api: GameApi, now: () => number = Date.now) {
    this.api = api;
    this.#now = now;
  }

  /** Starts listening (once, on first use). */
  #start(): void {
    if (this.#stop) return;
    this.#stop = this.api.listen((status) => this.#take(status));
    void this.api.status().then(
      (status) => {
        // An event that arrived meanwhile is newer.
        if (this.#status === null) this.#take(status);
      },
      () => this.#take(NOTHING),
    );
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#start();
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  readonly getVersion = (): number => this.#version;

  status(): GameStatus | null {
    return this.#status;
  }

  /** Whether Game is on. */
  usable(): boolean {
    return this.#status?.enabled === true;
  }

  /** The chat's game, once its folder has been looked at. */
  of(chatId: string): ChatGame | null {
    return this.#status?.chats.find((chat) => chat.chat_id === chatId) ?? null;
  }

  /**
   * Whether the chat's header offers the Game pane: its folder holds a game, a game of its runs
   * or ran, or its agent has the game tools (the person turned Game on for it).
   */
  offered(chatId: string): boolean {
    if (!this.usable()) return false;
    const game = this.of(chatId);
    return game !== null && (game.project !== null || game.state !== "idle" || game.tools);
  }

  /** A chat is shown, or one of its turns ended: what its folder holds now. */
  look(chatId: string): void {
    this.#start();
    if (this.#looking.has(chatId)) return;
    this.#looking.add(chatId);
    void this.api
      .look(chatId)
      .then(
        (status) => this.#take(status),
        () => {},
      )
      .finally(() => this.#looking.delete(chatId));
  }

  /** The pane was opened: look for Godot again, and at the chat's folder. */
  refresh(chatId: string): void {
    this.#start();
    void this.api.refresh(chatId).then(
      (status) => this.#take(status),
      () => {},
    );
  }

  /** Whether the chat's agent called on its game since the pane last answered. */
  summoned(chatId: string): boolean {
    const called = this.#called.get(chatId);
    // A call from a stretch of work that has ended opens nothing later.
    return (
      this.#pending.has(chatId) && called !== undefined && this.#now() - called < SUMMON_QUIET_MS
    );
  }

  /** The pane answered the chat's summon (it opened, or was open). */
  answer(chatId: string): void {
    this.#pending.delete(chatId);
  }

  /** The person closed the chat's pane: the agent's next calls in this stretch leave it closed. */
  closed(chatId: string): void {
    this.#pending.delete(chatId);
    const called = this.#called.get(chatId);
    if (called !== undefined && this.#now() - called < SUMMON_QUIET_MS) this.#closed.add(chatId);
  }

  #summons(status: GameStatus, first: boolean): void {
    for (const { chat_id: chatId, seq } of status.summons ?? []) {
      if (this.#seen.get(chatId) === seq) continue;
      this.#seen.set(chatId, seq);
      // What was called before this window started is not a reason to open anything.
      if (first) continue;
      const now = this.#now();
      const before = this.#called.get(chatId);
      if (before === undefined || now - before >= SUMMON_QUIET_MS) this.#closed.delete(chatId);
      this.#called.set(chatId, now);
      if (!this.#closed.has(chatId)) this.#pending.add(chatId);
    }
  }

  #take(status: GameStatus): void {
    this.#summons(status, this.#status === null);
    this.#status = status;
    this.#version += 1;
    for (const listener of [...this.#listeners]) listener();
  }
}

export const gameStore = new GameStore(tauriGameApi);

const PANEL_PREFIX = "ghost.game-panel.";

export interface PanelStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Whether each chat's Game pane is open, on this device. Storage failures keep it closed. */
export class GamePanels {
  readonly #storage: PanelStorage | null;
  readonly #listeners = new Set<() => void>();
  #version = 0;

  constructor(storage: PanelStorage | null) {
    this.#storage = storage;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  readonly getVersion = (): number => this.#version;

  open(chatId: string): boolean {
    try {
      return this.#storage?.getItem(PANEL_PREFIX + chatId) === "open";
    } catch {
      return false;
    }
  }

  set(chatId: string, open: boolean): void {
    try {
      this.#storage?.setItem(PANEL_PREFIX + chatId, open ? "open" : "closed");
    } catch {
      /* A full or blocked storage keeps the pane closed next time. */
    }
    this.#version += 1;
    for (const listener of [...this.#listeners]) listener();
  }
}

export const gamePanels = new GamePanels(typeof localStorage === "undefined" ? null : localStorage);

/** An engine's name as people write it. */
export function engineName(engine: GameEngine): string {
  switch (engine) {
    case "godot":
      return "Godot";
    case "web":
      return "Web";
    case "unity":
      return "Unity";
    case "unreal":
      return "Unreal Engine";
    case "roblox":
      return "Roblox";
    case "bevy":
      return "Bevy";
    case "love":
      return "LÖVE";
    case "pygame":
      return "Pygame";
    case "game_maker":
      return "GameMaker";
    case "defold":
      return "Defold";
    case "mono_game":
      return "MonoGame";
  }
}

/** Bytes as people read a download: "86 MB", "1.1 GB". */
export function megabytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 MB";
  const mb = bytes / (1024 * 1024);
  if (mb >= 1000) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}

/** The speeds the pane offers, slowest first. */
export const SPEEDS: readonly number[] = [0.25, 0.5, 1, 2];

/** Keys the pane never sends to the game: they are the window's own, or the person's way out. */
function reserved(event: { code: string; ctrlKey: boolean; metaKey: boolean }): boolean {
  // Ctrl and Command chords are Ghost's shortcuts (closing the pane, the composer, the palette).
  if (event.ctrlKey || event.metaKey) return true;
  // F5, F11 and F12 are the web view's; the rest of the function row is the game's.
  return event.code === "F5" || event.code === "F11" || event.code === "F12";
}

/**
 * The person's keys, as the game hears them: each key goes down once (a held key's repeats are
 * the game's own business) and comes up again, and every key still down is released when the
 * pane loses the keyboard, so a game is never left with a key stuck down.
 */
export class KeyboardSender {
  readonly #send: (events: GameInput[]) => void;
  readonly #down = new Set<string>();

  constructor(send: (events: GameInput[]) => void) {
    this.#send = send;
  }

  /** A key went down. Answers whether the game took it (so the page should not). */
  down(event: {
    code: string;
    key: string;
    repeat: boolean;
    shiftKey: boolean;
    ctrlKey: boolean;
    altKey: boolean;
    metaKey: boolean;
  }): boolean {
    if (event.code === "" || reserved(event)) return false;
    if (event.repeat || this.#down.has(event.code)) return true;
    this.#down.add(event.code);
    this.#send([
      {
        t: "key",
        code: event.code,
        down: true,
        // What the key types, for a game's text fields: one character, never a key's name.
        text: [...event.key].length === 1 ? event.key : "",
        shift: event.shiftKey,
        ctrl: false,
        alt: event.altKey,
      },
    ]);
    return true;
  }

  /** A key came up. Answers whether it was one the game held. */
  up(event: { code: string }): boolean {
    if (!this.#down.delete(event.code)) return false;
    this.#send([{ t: "key", code: event.code, down: false }]);
    return true;
  }

  /** The pane lost the keyboard, or the game stopped: every key still down comes up. */
  releaseAll(): void {
    if (this.#down.size === 0) return;
    const events: GameInput[] = [...this.#down].map((code) => ({ t: "key", code, down: false }));
    this.#down.clear();
    this.#send(events);
  }

  held(): number {
    return this.#down.size;
  }
}

/** The mouse button a pointer event's button number stands for. */
export function mouseButton(button: number): "left" | "middle" | "right" | null {
  return button === 0 ? "left" : button === 1 ? "middle" : button === 2 ? "right" : null;
}
