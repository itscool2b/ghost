/**
 * Games without an engine, for the previews and the unit tests (docs/game.md): a status the page
 * can change, every call the pane made, and frames a test can push as the game's picture.
 */
import type {
  ChatGame,
  EngineProbe,
  GameAsset,
  GameAssetData,
  GameAssets,
  GameConnected,
  GameExport,
  GameInput,
  GameLog,
  GamePicked,
  GameProject,
  GameRecent,
  GameStatus,
  GameToolsChange,
  GameTunables,
  GodotStatus,
  Starter,
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
    joypads: 0,
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
function base64(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text);
}

/** A quarter of a second of a falling tone: a real WAV, mono, 8 kHz, 16 bits. */
function wav(): Uint8Array {
  const [rate, count] = [8000, 2000];
  const bytes = new Uint8Array(44 + count * 2);
  const view = new DataView(bytes.buffer);
  const text = (at: number, word: string) => {
    for (let index = 0; index < word.length; index += 1) bytes[at + index] = word.charCodeAt(index);
  };
  text(0, "RIFF");
  view.setUint32(4, 36 + count * 2, true);
  text(8, "WAVEfmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, rate, true);
  view.setUint32(28, rate * 2, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  text(36, "data");
  view.setUint32(40, count * 2, true);
  for (let index = 0; index < count; index += 1) {
    const fade = 1 - index / count;
    const value = Math.sin((index / rate) * 2 * Math.PI * (660 - index / 8)) * fade;
    view.setInt16(44 + index * 2, Math.round(value * 30000), true);
  }
  return bytes;
}

/** The smallest model there is: one triangle in a .glb. */
function glb(): Uint8Array {
  const json = JSON.stringify({
    asset: { version: "2.0" },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ mesh: 0, name: "Crate" }],
    meshes: [{ primitives: [{ attributes: { POSITION: 0 } }] }],
    buffers: [{ byteLength: 36 }],
    bufferViews: [{ buffer: 0, byteOffset: 0, byteLength: 36, target: 34962 }],
    accessors: [
      {
        bufferView: 0,
        componentType: 5126,
        count: 3,
        type: "VEC3",
        min: [0, 0, 0],
        max: [1, 1, 0],
      },
    ],
  });
  const padded = json + " ".repeat((4 - (json.length % 4)) % 4);
  const total = 12 + 8 + padded.length + 8 + 36;
  const bytes = new Uint8Array(total);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, 0x46546c67, true);
  view.setUint32(4, 2, true);
  view.setUint32(8, total, true);
  view.setUint32(12, padded.length, true);
  view.setUint32(16, 0x4e4f534a, true);
  for (let index = 0; index < padded.length; index += 1)
    bytes[20 + index] = padded.charCodeAt(index);
  const at = 20 + padded.length;
  view.setUint32(at, 36, true);
  view.setUint32(at + 4, 0x004e4942, true);
  [0, 0, 0, 1, 0, 0, 0, 1, 0].forEach((value, index) => {
    view.setFloat32(at + 8 + index * 4, value, true);
  });
  return bytes;
}

/** A picture drawn here, as a PNG. Browser only. */
async function png(
  width: number,
  height: number,
  draw: (context: CanvasRenderingContext2D) => void,
): Promise<Uint8Array> {
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext("2d");
  if (context) draw(context);
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  return blob ? new Uint8Array(await blob.arrayBuffer()) : new Uint8Array();
}

const asset = (
  path: string,
  kind: GameAsset["kind"],
  bytes: number,
  age: number,
  tooLarge = false,
): GameAsset => ({ path, kind, bytes, modified: 1_764_000_000 - age, too_large: tooLarge });

export class FakeGame implements GameApi {
  /** Every call the pane made, as `name` or `name:argument`. */
  readonly calls: string[] = [];
  /** Every input event the pane sent, in order. */
  readonly inputs: GameInput[] = [];
  /** Every message the pane handed the chat (the preview's message box writes them here). */
  readonly sent: { text: string; attachments: string[] }[] = [];
  /** Every picture it attached: whether it was a PNG, and its size. */
  readonly pastes: { name: string | null; bytes: number; png: boolean }[] = [];
  /** What the pane put in the message box, as the box last held it. */
  drafted = "";
  /** Looks at the chat's folder again, as the end of a turn does (the preview sets it). */
  look_: () => void = () => {};
  #status: GameStatus;
  readonly #listeners = new Set<(status: GameStatus) => void>();
  /** The pane's frame listeners, by chat: a test may push packets to one. */
  readonly watching = new Map<string, (packet: Uint8Array) => void>();
  /** Every chat that is looked at holds a game (the tour's fixture, where any chat may open). */
  everyChat = false;
  /** What a look for another engine's tools finds. */
  probed: EngineProbe | null = null;
  /** Whether the engine's server was added to the project already. */
  #connected = false;
  /** What `turnOn` answers. */
  toolsChange: GameToolsChange = "restarting";
  /** Where the fixture's last frame drew its player, in the picture's pixels. */
  #player = { x: 200, y: 400, width: 40, height: 60, of: { width: 960, height: 540 } };
  /** The values the fixture's scene exports, as the pane turned them. */
  tunables_: GameTunables = {
    path: "/root/Main",
    name: "Main",
    class: "Node2D",
    script: "res://main.gd",
    values: [
      { name: "speed", kind: "float", value: 200, initial: 200, min: null, max: null, step: null },
      { name: "jump_height", kind: "float", value: 96, initial: 96, min: 0, max: 320, step: 4 },
      { name: "lives", kind: "int", value: 3, initial: 3, min: 1, max: 9, step: 1 },
      { name: "double_jump", kind: "bool", value: 0, initial: 0, min: null, max: null, step: null },
    ],
  };
  /** The folder's art, the newest first. */
  assets_: GameAsset[] = [
    asset("art/player/run.png", "picture", 412, 60),
    asset("sound/jump.wav", "sound", 4044, 120),
    asset("models/crate.glb", "model", 680, 180),
    asset("art/title.png", "picture", 3120, 240),
    asset("art/tiles/grass.png", "picture", 96, 300),
    asset("models/tree.gltf", "model", 2210, 360),
    asset("art/backdrop.png", "picture", 15_728_640, 420, true),
  ];
  /** What the last seconds answer; a test may make the run one that cannot be played again. */
  unrepeatable: string | null = null;
  #replays = 0;
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
    this.#player = { x: playerX, y: height - 140, width: 40, height: 60, of: { width, height } };
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
  /**
   * What is under a point: the player where the last frame drew it, the ground under the
   * picture's last 80 lines, and nothing in the sky. The point is in the game's own pixels,
   * which the frame may be a smaller picture of.
   */
  async pick(chatId: string, x: number, y: number): Promise<GamePicked | null> {
    this.calls.push(`pick:${Math.round(x)},${Math.round(y)}`);
    const game = this.#status.chats.find((entry) => entry.chat_id === chatId);
    const drawn = this.#player;
    const scale = game && game.width > 0 ? game.width / drawn.of.width : 1;
    const height = drawn.of.height * scale;
    const player = {
      x: drawn.x * scale,
      y: drawn.y * scale,
      width: drawn.width * scale,
      height: drawn.height * scale,
    };
    const inside =
      x >= player.x &&
      x <= player.x + player.width &&
      y >= player.y &&
      y <= player.y + player.height;
    if (inside)
      return {
        path: "/root/Main/Player/Sprite2D",
        name: "Sprite2D",
        class: "Sprite2D",
        text: "",
        scene: "res://player.tscn",
        in_scene: "Sprite2D",
        scene_line: 9,
        script: "res://player.gd",
        script_on: "/root/Main/Player",
        area: [player.x, player.y, player.width, player.height],
      };
    if (y >= height - 80 * scale)
      return {
        path: "/root/Main/Ground",
        name: "Ground",
        class: "StaticBody2D",
        text: "",
        scene: "res://main.tscn",
        in_scene: "Ground",
        scene_line: 14,
        script: "res://main.gd",
        script_on: "/root/Main",
        area: [0, height - 80 * scale, drawn.of.width * scale, 80 * scale],
      };
    return null;
  }
  async tunables(_chatId: string, node: string): Promise<GameTunables> {
    this.calls.push(`tunables:${node}`);
    if (node === "/root/Main/Player")
      return {
        path: node,
        name: "Player",
        class: "CharacterBody2D",
        script: "res://player.gd",
        values: [],
      };
    return this.tunables_;
  }
  async setValue(
    _chatId: string,
    node: string,
    property: string,
    _kind: string,
    value: number,
  ): Promise<void> {
    this.calls.push(`set:${node}:${property}=${value}`);
    this.tunables_ = {
      ...this.tunables_,
      values: this.tunables_.values.map((entry) =>
        entry.name === property ? { ...entry, value } : entry,
      ),
    };
  }
  /** Twelve small pictures of a run, drawn here, and the run's name for the agent. */
  async recent(_chatId: string, seconds: number): Promise<GameRecent> {
    this.calls.push(`recent:${seconds}`);
    const canvas = document.createElement("canvas");
    const [cell, gap] = [{ width: 320, height: 180 }, 4];
    canvas.width = 4 * cell.width + 3 * gap;
    canvas.height = 3 * cell.height + 2 * gap;
    const context = canvas.getContext("2d");
    if (context) {
      context.fillStyle = "#121212";
      context.fillRect(0, 0, canvas.width, canvas.height);
      for (let index = 0; index < 12; index += 1) {
        const left = (index % 4) * (cell.width + gap);
        const top = Math.floor(index / 4) * (cell.height + gap);
        context.fillStyle = "#14161c";
        context.fillRect(left, top, cell.width, cell.height);
        context.fillStyle = "#3a4152";
        context.fillRect(left, top + cell.height - 27, cell.width, 27);
        context.fillStyle = "#e8eaf0";
        context.fillRect(left + 20 + index * 22, top + cell.height - 47, 13, 20);
      }
    }
    const blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob(resolve, "image/jpeg", 0.8),
    );
    const picture = blob ? [...new Uint8Array(await blob.arrayBuffer())] : [];
    if (this.unrepeatable === null) this.#replays += 1;
    return {
      replay: this.unrepeatable === null ? `r${this.#replays}` : null,
      unrepeatable: this.unrepeatable,
      picture,
      pictures: 12,
      seconds: Math.min(seconds, 15),
      run_seconds: 48.5,
      inputs: 212,
    };
  }
  /** What an export answers; a test may make it fail, or make it large. */
  exported_: GameExport = {
    folder: "C:\\Users\\me\\Ghost\\game\\exports\\moon-hop",
    zip: "C:\\Users\\me\\Downloads\\moon-hop-web.zip",
    files: 9,
    bytes: 9_961_472,
    what: "Godot's web export",
  };
  exportFails: string | null = null;
  async exportWeb(): Promise<GameExport> {
    this.calls.push("export");
    // Long enough for the pane to say it is working.
    await new Promise((done) => setTimeout(done, 150));
    if (this.exportFails !== null) throw new Error(this.exportFails);
    return this.exported_;
  }
  async exportShow(): Promise<void> {
    this.calls.push("exportShow");
  }
  /** A turn made a picture: it is the folder's newest. */
  made(path: string): void {
    const newest = Math.max(...this.assets_.map((entry) => entry.modified));
    this.assets_ = [
      { path, kind: "picture", bytes: 300, modified: newest + 5, too_large: false },
      ...this.assets_.filter((entry) => entry.path !== path),
    ];
  }
  async assets(): Promise<GameAssets> {
    this.calls.push("assets");
    return { assets: [...this.assets_], cut: false };
  }
  async asset(_chatId: string, path: string): Promise<GameAssetData> {
    this.calls.push(`asset:${path}`);
    if (path.endsWith(".wav")) return { kind: "sound", mime: "audio/wav", base64: base64(wav()) };
    if (path.endsWith(".glb"))
      return { kind: "model", mime: "model/gltf-binary", base64: base64(glb()) };
    if (path.endsWith(".gltf")) return { kind: "model", mime: "model/gltf+json", base64: "e30=" };
    // A sheet of four frames, each a square with a figure a step further on; other pictures
    // are one frame of the same.
    const sheet = path.endsWith("run.png");
    const [cell, frames] = sheet ? [32, 4] : path.endsWith("grass.png") ? [16, 1] : [180, 1];
    const width = sheet ? cell * frames : path.endsWith("title.png") ? 320 : cell;
    const bytes = await png(width, cell, (context) => {
      for (let frame = 0; frame < frames; frame += 1) {
        context.fillStyle = frame % 2 === 0 ? "#22303f" : "#2a3b4d";
        context.fillRect(frame * cell, 0, sheet ? cell : width, cell);
        context.fillStyle = "#f2c14e";
        context.fillRect(
          frame * cell + cell * 0.2 + frame * 3,
          cell * 0.25,
          cell * 0.3,
          cell * 0.5,
        );
      }
    });
    return { kind: "picture", mime: "image/png", base64: base64(bytes) };
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
  async startProject(chatId: string, starter: Starter): Promise<GameToolsChange> {
    this.calls.push(`startProject:${starter}`);
    const web = starter === "web2d" || starter === "web3d";
    this.game(chatId, {
      tools: true,
      project: project({
        engine: web ? "web" : "godot",
        name: web ? "moon-hop" : "Moon Hop",
        version: web ? null : "4.7",
        framework: starter === "web3d" ? "three" : starter === "web2d" ? "canvas" : null,
      }),
    });
    return this.toolsChange;
  }
  async probe(chatId: string): Promise<EngineProbe | null> {
    this.calls.push("probe");
    const engine = this.#of(chatId)?.project?.engine;
    return this.probed ?? (engine ? { engine, ready: false, detail: "" } : null);
  }
  async connect(): Promise<GameConnected> {
    this.calls.push("connect");
    const result = this.#connected ? "already" : "added";
    this.#connected = true;
    return result;
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
