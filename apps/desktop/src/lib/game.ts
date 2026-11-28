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
  type EngineProbe,
  type GameAsset,
  type GameAssetData,
  type GameAssets,
  type GameConnected,
  type GameEngine,
  type GameInput,
  type GameLog,
  type GamePicked,
  type GameRecent,
  type GameStatus,
  type GameToolsChange,
  type GameTunables,
  type Starter,
} from "../bindings";
import { freshSince } from "./gameAssets";
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
  /** What is under a point of the picture, in the game's own pixels; nothing there is null. */
  pick(chatId: string, x: number, y: number): Promise<GamePicked | null>;
  /** The values of a node a person may turn while the game runs. */
  tunables(chatId: string, node: string): Promise<GameTunables>;
  /** Turns one of them, in the running game only. */
  setValue(
    chatId: string,
    node: string,
    property: string,
    kind: string,
    value: number,
  ): Promise<void>;
  /** The last seconds of the run as pictures, and the run kept for the agent to play again. */
  recent(chatId: string, seconds: number): Promise<GameRecent>;
  /** The pictures, sounds and models of the chat's folder, the newest first. */
  assets(chatId: string): Promise<GameAssets>;
  /** One of them, by the path it was listed under. */
  asset(chatId: string, path: string): Promise<GameAssetData>;
  /** Streams the chat's game to `onPacket`; answers the stop. */
  watch(chatId: string, onPacket: (packet: Uint8Array) => void): Promise<() => void>;
  log(chatId: string): Promise<GameLog>;
  /** A window of the game's own, or back out of sight. A running game starts again. */
  window(chatId: string, windowed: boolean): Promise<void>;
  openEditor(chatId: string): Promise<void>;
  /** Saves a picture of the game; answers where it is. */
  picture(chatId: string): Promise<string>;
  /** Writes one of Ghost's starter projects into the chat's folder, and turns Game on for it. */
  startProject(chatId: string, starter: Starter): Promise<GameToolsChange>;
  /** Whether the agent tools of the chat's engine (Unity, Unreal, Roblox) are within reach. */
  probe(chatId: string): Promise<EngineProbe | null>;
  /** Adds the engine's own MCP server to the chat's project. */
  connect(chatId: string): Promise<GameConnected>;
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
  pick: (chatId, x, y) => settle(commands.gamePick(chatId, x, y)),
  tunables: (chatId, node) => settle(commands.gameTunables(chatId, node)),
  setValue: (chatId, node, property, kind, value) =>
    done(settle(commands.gameSetValue(chatId, node, property, kind, value))),
  recent: (chatId, seconds) => settle(commands.gameRecent(chatId, seconds)),
  assets: (chatId) => settle(commands.gameAssets(chatId)),
  asset: (chatId, path) => settle(commands.gameAsset(chatId, path)),
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
  startProject: (chatId, starter) => settle(commands.gameStartProject(chatId, starter)),
  probe: (chatId) => settle(commands.gameProbe(chatId)),
  connect: (chatId) => settle(commands.gameConnect(chatId)),
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

const NO_ASSETS: readonly GameAsset[] = [];

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
  /** What each game chat's folder held when it was last listed: each asset's path and age. */
  readonly #listed = new Map<string, ReadonlyMap<string, number>>();
  /** The assets that appeared or changed since, for the chat to show until they are seen. */
  readonly #fresh = new Map<string, readonly GameAsset[]>();
  /** An asset the chat asked the pane to open. */
  readonly #wanted = new Map<string, string>();

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
        (status) => {
          this.#take(status);
          this.#list(chatId);
        },
        () => {},
      )
      .finally(() => this.#looking.delete(chatId));
  }

  /**
   * Lists a game chat's assets again. The first list is what was there; from then on, what is
   * new or changed since the list before is kept for the chat to show: the art a turn made.
   */
  #list(chatId: string): void {
    if (!this.of(chatId)?.project) return;
    void this.api.assets(chatId).then(
      (list) => {
        const before = this.#listed.get(chatId);
        this.#listed.set(chatId, new Map(list.assets.map((asset) => [asset.path, asset.modified])));
        if (!before) return;
        const fresh = freshSince(before, list.assets);
        if (fresh.length === 0) return;
        const paths = new Set(fresh.map((asset) => asset.path));
        const waiting = (this.#fresh.get(chatId) ?? []).filter((asset) => !paths.has(asset.path));
        this.#fresh.set(chatId, [...fresh, ...waiting].slice(0, 24));
        this.#changed();
      },
      () => {},
    );
  }

  /** The assets a turn left in the chat's folder that the person has not looked at yet. */
  freshAssets(chatId: string): readonly GameAsset[] {
    return this.#fresh.get(chatId) ?? NO_ASSETS;
  }

  /** The person dismissed them. */
  seenAssets(chatId: string): void {
    if (this.#fresh.delete(chatId)) this.#changed();
  }

  /** The chat asks the pane to open one asset. */
  wantAsset(chatId: string, path: string): void {
    this.#wanted.set(chatId, path);
    this.#fresh.delete(chatId);
    this.#changed();
  }

  wantedAsset(chatId: string): string | null {
    return this.#wanted.get(chatId) ?? null;
  }

  /** The pane opened it. */
  shownAsset(chatId: string): void {
    if (this.#wanted.delete(chatId)) this.#changed();
  }

  #changed(): void {
    this.#version += 1;
    for (const listener of [...this.#listeners]) listener();
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

/** The projects Ghost can start, as the pane lists them. */
export const STARTERS: readonly {
  readonly id: Starter;
  readonly name: string;
  readonly what: string;
}[] = [
  { id: "godot2d", name: "Godot, 2D", what: "A player that runs and jumps on a floor." },
  { id: "godot3d", name: "Godot, 3D", what: "A player on a floor, with a camera and a sun." },
  { id: "web2d", name: "Web, 2D", what: "A plain canvas page with nothing to install." },
  { id: "web3d", name: "Web, 3D", what: "Three.js, with Vite to serve it." },
];

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

/**
 * A controller, as the game hears it. The pane reads the standard gamepad the browser gives it
 * and sends what changed, in Godot's own numbers: its triggers are axes there, and its buttons
 * are in another order.
 */
const PAD_BUTTONS: readonly (number | null)[] = [
  0, // bottom face button
  1, // right
  2, // left
  3, // top
  9, // left shoulder
  10, // right shoulder
  null, // the triggers are axes 4 and 5
  null,
  4, // back
  6, // start
  7, // left stick
  8, // right stick
  11, // up
  12, // down
  13, // left
  14, // right
  5, // guide
];
const TRIGGERS: readonly (readonly [number, number])[] = [
  [6, 4],
  [7, 5],
];

/** What the game was last told of a controller: buttons by Godot's number, then six axes. */
export interface PadState {
  readonly buttons: readonly boolean[];
  readonly axes: readonly number[];
}

export const PAD_REST: PadState = {
  buttons: Array.from({ length: 15 }, () => false),
  axes: [0, 0, 0, 0, 0, 0],
};

/**
 * A stick as the game hears it: nothing inside the dead zone, and in twentieths, so a resting
 * hand's tremor is not sent sixty times a second.
 */
function stick(value: number, dead: number): number {
  if (!Number.isFinite(value) || Math.abs(value) < dead) return 0;
  return Math.round(Math.max(-1, Math.min(1, value)) * 20) / 20;
}

/** What changed on a controller since `before`, as input for the game, and what it is told now. */
export function padEvents(
  before: PadState,
  pad: {
    readonly buttons: readonly { readonly pressed: boolean; readonly value: number }[];
    readonly axes: readonly number[];
  },
): { readonly events: GameInput[]; readonly state: PadState } {
  const events: GameInput[] = [];
  const buttons = [...before.buttons];
  PAD_BUTTONS.forEach((button, index) => {
    if (button === null) return;
    const down = pad.buttons[index]?.pressed === true;
    if (down === (buttons[button] ?? false)) return;
    buttons[button] = down;
    events.push({ t: "pad", button, down });
  });
  const axes = [...before.axes];
  const tell = (axis: number, value: number) => {
    if (value === (axes[axis] ?? 0)) return;
    axes[axis] = value;
    events.push({ t: "axis", axis, value });
  };
  for (let axis = 0; axis < 4; axis += 1) tell(axis, stick(pad.axes[axis] ?? 0, 0.12));
  for (const [button, axis] of TRIGGERS) tell(axis, stick(pad.buttons[button]?.value ?? 0, 0.05));
  return { events, state: { buttons, axes } };
}

/** Everything a controller holds let go: the pane lost the keyboard, or the controller left. */
export function padRelease(state: PadState): GameInput[] {
  const events: GameInput[] = [];
  state.buttons.forEach((down, button) => {
    if (down) events.push({ t: "pad", button, down: false });
  });
  state.axes.forEach((value, axis) => {
    if (value !== 0) events.push({ t: "axis", axis, value: 0 });
  });
  return events;
}
