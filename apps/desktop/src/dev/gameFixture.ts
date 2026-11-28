/**
 * Games without an engine, for the previews and the unit tests (docs/game.md): a status the page
 * can change, every call the pane made, and frames a test can push as the game's picture.
 */
import type {
  ChatGame,
  GameInput,
  GameLog,
  GameProject,
  GameStatus,
  GameToolsChange,
  GodotStatus,
} from "../bindings";
import type { GameApi } from "../lib/game";

export function godotStatus(change: Partial<GodotStatus> = {}): GodotStatus {
  return {
    pinned: "4.7.2",
    size: 86_013_866,
    fetched: true,
    found: [],
    looked: true,
    fetch: { state: "idle" },
    ...change,
  };
}

export function project(change: Partial<GameProject> = {}): GameProject {
  return {
    engine: "godot",
    name: "Moon Hop",
    version: "4.7",
    framework: null,
    unsupported: null,
    ...change,
  };
}

export function chatGame(chatId: string, change: Partial<ChatGame> = {}): ChatGame {
  return {
    chat_id: chatId,
    project: project(),
    state: "idle",
    frozen: false,
    speed: 1,
    fps: 0,
    scene: "",
    width: 0,
    height: 0,
    engine_version: "",
    errors: 0,
    last_error: null,
    ended: null,
    agent: null,
    sound: false,
    pid: null,
    godot_missing: null,
    windowed: false,
    tools: true,
    ...change,
  };
}

/** A game that runs: 1152 by 648, at sixty frames a second. */
export function running(chatId: string, change: Partial<ChatGame> = {}): ChatGame {
  return chatGame(chatId, {
    state: "running",
    fps: 60,
    scene: "res://main.tscn",
    width: 1152,
    height: 648,
    engine_version: "4.7.2-stable (official)",
    pid: 4242,
    ...change,
  });
}

export function gameStatus(change: Partial<GameStatus> = {}): GameStatus {
  return { enabled: true, godot: godotStatus(), chats: [], summons: [], ...change };
}

export const LOG: GameLog = {
  errors: [
    {
      kind: "script",
      text: "Invalid call. Nonexistent function 'hop' in base 'CharacterBody2D (player.gd)'.",
      file: "res://player.gd",
      line: 12,
      function: "_physics_process",
      stack: ["res://player.gd:12 in _physics_process"],
      count: 3,
    },
    {
      kind: "warning",
      text: "The signal 'landed' is declared but never emitted.",
      file: "res://player.gd",
      line: 4,
      function: "",
      stack: [],
      count: 1,
    },
  ],
  lines: ["Godot Engine v4.7.2.stable.official", "fixture ready", "score 1"],
  dropped: 0,
};

/** The stand-in for Rust's games. */
export class FakeGame implements GameApi {
  /** Every call the pane made, as `name` or `name:argument`. */
  readonly calls: string[] = [];
  /** Every input event the pane sent, in order. */
  readonly inputs: GameInput[] = [];
  #status: GameStatus;
  readonly #listeners = new Set<(status: GameStatus) => void>();
  /** The pane's frame listeners, by chat: a test may push packets to one. */
  readonly watching = new Map<string, (packet: Uint8Array) => void>();
  /** Every chat that is looked at holds a game (the tour's fixture, where any chat may open). */
  everyChat = false;
  /** What `turnOn` answers. */
  toolsChange: GameToolsChange = "restarting";
  log_: GameLog = LOG;

  constructor(status: GameStatus = gameStatus()) {
    this.#status = status;
  }

  current(): GameStatus {
    return this.#status;
  }

  /** Replaces the status, as an event from the app would. */
  set(status: GameStatus): void {
    this.#status = status;
    for (const listener of [...this.#listeners]) listener(status);
  }

  change(change: Partial<GameStatus>): void {
    this.set({ ...this.#status, ...change });
  }

  /** Changes one chat's game. */
  game(chatId: string, change: Partial<ChatGame>): void {
    this.change({
      chats: this.#status.chats.map((chat) =>
        chat.chat_id === chatId ? { ...chat, ...change } : chat,
      ),
    });
  }

  /** An agent's tool asked for the chat's pane. */
  summon(chatId: string): void {
    const seq = Math.max(0, ...this.#status.summons.map((summon) => summon.seq)) + 1;
    this.change({
      summons: [
        ...this.#status.summons.filter((summon) => summon.chat_id !== chatId),
        { chat_id: chatId, seq },
      ],
    });
  }

  /**
   * Pushes one frame to the chat's pane: a picture of `width` by `height` drawn here (a ground
   * line and a player at `playerX`), as the JPEG packet a real game sends. Browser only.
   */
  async frame(chatId: string, width = 960, height = 540, playerX = 200): Promise<void> {
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return;
    context.fillStyle = "#14161c";
    context.fillRect(0, 0, width, height);
    context.fillStyle = "#3a4152";
    context.fillRect(0, height - 80, width, 80);
    context.fillStyle = "#e8eaf0";
    context.fillRect(playerX, height - 140, 40, 60);
    const blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob(resolve, "image/jpeg", 0.8),
    );
    if (!blob) return;
    const jpeg = new Uint8Array(await blob.arrayBuffer());
    const packet = new Uint8Array(jpeg.length + 1);
    packet[0] = 6;
    packet.set(jpeg, 1);
    this.watching.get(chatId)?.(packet);
  }

  async status(): Promise<GameStatus> {
    return this.#status;
  }
  async look(chatId: string): Promise<GameStatus> {
    this.calls.push(`look:${chatId}`);
    if (this.everyChat && !this.#of(chatId)) {
      this.#status = { ...this.#status, chats: [...this.#status.chats, chatGame(chatId)] };
    }
    return this.#status;
  }
  async refresh(chatId: string): Promise<GameStatus> {
    this.calls.push(`refresh:${chatId}`);
    return this.#status;
  }
  async fetchStart(): Promise<void> {
    this.calls.push("fetchStart");
    this.change({
      godot: {
        ...this.#status.godot,
        fetch: { state: "running", received: 21_500_000, total: 86_013_866 },
      },
    });
  }
  async fetchCancel(): Promise<void> {
    this.calls.push("fetchCancel");
    this.change({ godot: { ...this.#status.godot, fetch: { state: "idle" } } });
  }
  async run(chatId: string, scene: string | null): Promise<void> {
    this.calls.push(`run:${scene ?? "main"}`);
    this.game(chatId, running(chatId, { project: this.#of(chatId)?.project ?? project() }));
  }
  async stop(chatId: string): Promise<void> {
    this.calls.push("stop");
    this.game(chatId, { state: "ended", ended: "Stopped.", fps: 0, pid: null, frozen: false });
  }
  async time(chatId: string, frozen: boolean | null, speed: number | null): Promise<void> {
    this.calls.push(`time:${frozen ?? "same"}:${speed ?? "same"}`);
    const game = this.#of(chatId);
    if (!game) return;
    this.game(chatId, { frozen: frozen ?? game.frozen, speed: speed ?? game.speed });
  }
  async step(): Promise<void> {
    this.calls.push("step");
  }
  async sound(chatId: string, on: boolean): Promise<void> {
    this.calls.push(`sound:${on}`);
    this.game(chatId, { sound: on });
  }
  async input(_chatId: string, events: readonly GameInput[]): Promise<void> {
    this.inputs.push(...events);
  }
  async watch(chatId: string, onPacket: (packet: Uint8Array) => void): Promise<() => void> {
    this.calls.push("watch");
    this.watching.set(chatId, onPacket);
    return () => {
      this.calls.push("unwatch");
      if (this.watching.get(chatId) === onPacket) this.watching.delete(chatId);
    };
  }
  async log(): Promise<GameLog> {
    this.calls.push("log");
    return this.log_;
  }
  async window(chatId: string, windowed: boolean): Promise<void> {
    this.calls.push(`window:${windowed}`);
    this.game(chatId, { windowed });
  }
  async openEditor(): Promise<void> {
    this.calls.push("openEditor");
  }
  async picture(): Promise<string> {
    this.calls.push("picture");
    return "C:\\Users\\you\\Pictures\\ghost-game-1764200000.jpg";
  }
  async turnOn(chatId: string, on: boolean): Promise<GameToolsChange> {
    this.calls.push(`turnOn:${on}`);
    if (this.#of(chatId)) this.game(chatId, { tools: on });
    return this.toolsChange;
  }
  async turnedOn(): Promise<boolean> {
    return false;
  }
  listen(listener: (status: GameStatus) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #of(chatId: string): ChatGame | null {
    return this.#status.chats.find((chat) => chat.chat_id === chatId) ?? null;
  }
}
