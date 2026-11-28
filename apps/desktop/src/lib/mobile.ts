/**
 * The device beside the chat (docs/superpowers/plans/2026-11-21-mobile.md): its commands, its
 * status as a store for React, each chat's pane on this device, and the decoder that turns a
 * device's video into a picture. Rust owns the devices; the window shows one and sends it input.
 */
import { Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  type ChatDevice,
  commands,
  type DeviceInfo,
  type DeviceKey,
  type MobileStatus,
  type TouchPhase,
} from "../bindings";
import { settle } from "./ipc";

export const MOBILE_EVENT = "ghost-mobile";

/** Everything the pane asks of the devices. The fixture implements it without a device. */
export interface MobileApi {
  status(): Promise<MobileStatus>;
  /** Looks again at what is installed, and starts watching for phones. */
  refresh(): Promise<MobileStatus>;
  fetchOffer(): Promise<void>;
  /** The person accepted Google's licence in the offer. */
  fetchStart(): Promise<void>;
  fetchCancel(): Promise<void>;
  attach(chatId: string, deviceId: string | null): Promise<void>;
  detach(chatId: string): Promise<void>;
  stopDevice(deviceId: string): Promise<void>;
  allowAgent(chatId: string, allowed: boolean): Promise<void>;
  /** Streams the chat's device to `onPacket`; answers the stop. */
  watch(chatId: string, onPacket: (packet: Uint8Array) => void): Promise<() => void>;
  touch(chatId: string, phase: TouchPhase, x: number, y: number): Promise<void>;
  scroll(chatId: string, x: number, y: number, horizontal: number, vertical: number): Promise<void>;
  key(chatId: string, key: DeviceKey): Promise<void>;
  text(chatId: string, text: string): Promise<void>;
  rotate(chatId: string): Promise<void>;
  /** Saves a picture of the screen; answers where it is. */
  screenshot(chatId: string): Promise<string>;
  /** Every change to the status; answers the unsubscribe. */
  listen(listener: (status: MobileStatus) => void): () => void;
}

const done = async (call: Promise<unknown>): Promise<void> => {
  await call;
};

export const tauriMobileApi: MobileApi = {
  status: () => settle(commands.mobileStatus()),
  refresh: () => settle(commands.mobileRefresh()),
  fetchOffer: () => done(settle(commands.mobileFetchOffer())),
  fetchStart: () => done(settle(commands.mobileFetchStart())),
  fetchCancel: () => done(settle(commands.mobileFetchCancel())),
  attach: (chatId, deviceId) => done(settle(commands.mobileAttach(chatId, deviceId))),
  detach: (chatId) => done(settle(commands.mobileDetach(chatId))),
  stopDevice: (deviceId) => done(settle(commands.mobileStopDevice(deviceId))),
  allowAgent: (chatId, allowed) => done(settle(commands.mobileAllowAgent(chatId, allowed))),
  watch: async (chatId, onPacket) => {
    // The packets arrive as raw bytes (an ArrayBuffer), whatever the generated type says.
    const channel = new Channel<number[]>((message) => {
      const raw = message as unknown;
      if (raw instanceof ArrayBuffer) onPacket(new Uint8Array(raw));
      else if (Array.isArray(raw)) onPacket(Uint8Array.from(raw as number[]));
    });
    // The stream's own number: stopping it can never end one that replaced it.
    const watch = await settle(commands.mobileWatch(chatId, channel));
    return () => {
      void commands.mobileUnwatch(chatId, watch).catch(() => {});
    };
  },
  touch: (chatId, phase, x, y) => done(settle(commands.mobileTouch(chatId, phase, x, y))),
  scroll: (chatId, x, y, horizontal, vertical) =>
    done(settle(commands.mobileScroll(chatId, x, y, horizontal, vertical))),
  key: (chatId, key) => done(settle(commands.mobileKey(chatId, key))),
  text: (chatId, text) => done(settle(commands.mobileText(chatId, text))),
  rotate: (chatId) => done(settle(commands.mobileRotate(chatId))),
  screenshot: (chatId) => settle(commands.mobileScreenshot(chatId)),
  listen: (listener) => {
    let stop: (() => void) | null = null;
    let stopped = false;
    void listen<MobileStatus>(MOBILE_EVENT, (event) => listener(event.payload)).then(
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

const NOTHING: MobileStatus = {
  enabled: false,
  unavailable: "Devices are not available here.",
  setup: {
    adb: false,
    emulator: false,
    acceleration: null,
    fetch: { state: "idle" },
    ios: { possible: false, ready: false, detail: null, missing: null, tool: { state: "missing" } },
  },
  devices: [],
  chats: [],
  summons: [],
};

/**
 * Calls closer together than this are one stretch of the agent's work: a pane the person closed
 * during it stays closed until the agent has been quiet this long.
 */
export const SUMMON_QUIET_MS = 60_000;

/** The devices as React reads them: one status, replaced whole on every change. */
export class MobileStore {
  readonly api: MobileApi;
  readonly #listeners = new Set<() => void>();
  readonly #now: () => number;
  #status: MobileStatus | null = null;
  #version = 0;
  #stop: (() => void) | null = null;
  /** The latest summon number seen for each chat. */
  readonly #seen = new Map<string, number>();
  /** Chats whose agent called on the device since their pane last answered. */
  readonly #pending = new Set<string>();
  /** When each chat's agent last called, and the chats whose pane the person closed meanwhile. */
  readonly #called = new Map<string, number>();
  readonly #closed = new Set<string>();

  constructor(api: MobileApi, now: () => number = Date.now) {
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

  status(): MobileStatus | null {
    return this.#status;
  }

  /** Whether the pane can be offered: the plugin is on and Android's tools exist for this system. */
  usable(): boolean {
    return this.#status?.enabled === true && this.#status.unavailable === null;
  }

  /** The device a chat holds. */
  held(chatId: string): ChatDevice | null {
    return this.#status?.chats.find((chat) => chat.chat_id === chatId) ?? null;
  }

  devices(): readonly DeviceInfo[] {
    return this.#status?.devices ?? [];
  }

  /** The pane was opened: look again at what is installed. */
  refresh(): void {
    this.#start();
    void this.api.refresh().then(
      (status) => this.#take(status),
      () => {},
    );
  }

  /**
   * Whether the chat's agent called on its device since the pane last answered: the pane should
   * open beside the chat (owner, 24 November 2026: "the agent should be able to open the phone
   * and start working").
   */
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

  #summons(status: MobileStatus, first: boolean): void {
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

  #take(status: MobileStatus): void {
    this.#summons(status, this.#status === null);
    this.#status = status;
    this.#version += 1;
    for (const listener of [...this.#listeners]) listener();
  }
}

export const mobileStore = new MobileStore(tauriMobileApi);

const PANEL_PREFIX = "o7.device-panel.";

export interface PanelStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** Whether each chat's Device pane is open, on this device. Storage failures keep it closed. */
export class DevicePanels {
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

export const devicePanels = new DevicePanels(
  typeof localStorage === "undefined" ? null : localStorage,
);

// The player that draws a device's video, and the mapping from the pane to the picture, are
// shared with the Game pane (lib/screen.ts).
export { avcCodec, ScreenPlayer, type ScreenState, videoPoint } from "./screen";

/** The device key a keyboard key stands for, or `null` for one that is text or not forwarded. */
export function deviceKey(key: string): DeviceKey | null {
  switch (key) {
    case "Enter":
      return "enter";
    case "Backspace":
      return "delete";
    case "Delete":
      return "forward_delete";
    case "Tab":
      return "tab";
    case "Escape":
      return "back";
    case "ArrowUp":
      return "up";
    case "ArrowDown":
      return "down";
    case "ArrowLeft":
      return "left";
    case "ArrowRight":
      return "right";
    default:
      return null;
  }
}

/** Megabytes as people read them: "1.8 GB", "456 MB". */
export function megabytes(mb: number | null): string {
  if (mb === null || !Number.isFinite(mb) || mb <= 0) return "0 MB";
  if (mb >= 1000) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}
