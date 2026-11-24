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

export const MOBILE_EVENT = "o7-mobile";

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
};

/** The devices as React reads them: one status, replaced whole on every change. */
export class MobileStore {
  readonly api: MobileApi;
  readonly #listeners = new Set<() => void>();
  #status: MobileStatus | null = null;
  #version = 0;
  #stop: (() => void) | null = null;

  constructor(api: MobileApi) {
    this.api = api;
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

  #take(status: MobileStatus): void {
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

/** The kinds of packet a device's video is sent as (`o7_devices::manager::Viewer`). */
const PACKET_SIZE = 1;
const PACKET_CONFIG = 2;
const PACKET_KEY = 3;
const PACKET_FRAME = 4;
const PACKET_ENDED = 5;
/** A whole picture as a JPEG: an iPhone simulator's screen. */
const PACKET_PICTURE = 6;

/**
 * The WebCodecs codec string for an H.264 configuration in Annex B form: `avc1.` and the three
 * bytes after the sequence parameter set's header (profile, constraints, level). `null` when
 * the bytes hold no such set.
 */
export function avcCodec(config: Uint8Array): string | null {
  for (let at = 0; at + 4 < config.length; at += 1) {
    const three = config[at] === 0 && config[at + 1] === 0 && config[at + 2] === 1;
    if (!three) continue;
    const header = config[at + 3];
    // A sequence parameter set is unit type 7.
    if (header !== undefined && (header & 0x1f) === 7 && at + 6 < config.length) {
      const hex = (value: number | undefined) => (value ?? 0).toString(16).padStart(2, "0");
      return `avc1.${hex(config[at + 4])}${hex(config[at + 5])}${hex(config[at + 6])}`;
    }
  }
  return null;
}

/** What the screen is doing, for the pane to say. */
export type ScreenState =
  | { readonly kind: "waiting" }
  | { readonly kind: "live"; readonly width: number; readonly height: number }
  | { readonly kind: "ended" }
  /** This system's webview cannot decode the video. */
  | { readonly kind: "unsupported" };

/**
 * Draws a device's video on a canvas. Packets go in as they arrive; frames come out on the canvas
 * as soon as they are decoded. A configuration packet starts the decoder again, and the key frame
 * after it carries the configuration in front of it, as Annex B decoding needs. An iPhone's screen
 * arrives as whole pictures instead, which every web view can draw.
 */
export class ScreenPlayer {
  readonly #canvas: HTMLCanvasElement;
  readonly #onState: (state: ScreenState) => void;
  #decoder: VideoDecoder | null = null;
  #config: Uint8Array | null = null;
  #configured = false;
  #size: { width: number; height: number } | null = null;
  #closed = false;
  /** A picture is being drawn; the newest one that came meanwhile waits here. */
  #drawing = false;
  #waiting: Uint8Array | null = null;

  constructor(canvas: HTMLCanvasElement, onState: (state: ScreenState) => void) {
    this.#canvas = canvas;
    this.#onState = onState;
    onState({ kind: "waiting" });
  }

  /** The video's size now, in its own pixels: what a touch's coordinates are measured in. */
  size(): { width: number; height: number } | null {
    return this.#size;
  }

  push(packet: Uint8Array): void {
    if (this.#closed || packet.length === 0) return;
    const kind = packet[0];
    const view = new DataView(packet.buffer, packet.byteOffset, packet.byteLength);
    if (kind === PACKET_SIZE && packet.length >= 5) {
      this.#size = { width: view.getUint16(1), height: view.getUint16(3) };
      return;
    }
    if (kind === PACKET_ENDED) {
      this.#onState({ kind: "ended" });
      return;
    }
    if (kind === PACKET_PICTURE) {
      this.#picture(packet.subarray(1));
      return;
    }
    // The rest is H.264, which needs the web view's video decoder.
    if (typeof VideoDecoder === "undefined") {
      if (kind === PACKET_CONFIG || kind === PACKET_KEY) this.#onState({ kind: "unsupported" });
      return;
    }
    if (kind === PACKET_CONFIG) {
      this.#config = packet.slice(1);
      this.#configured = false;
      return;
    }
    if ((kind !== PACKET_KEY && kind !== PACKET_FRAME) || packet.length < 9) return;
    const key = kind === PACKET_KEY;
    const timestamp = Number(view.getBigUint64(1));
    let data = packet.subarray(9);
    if (key && !this.#configured) {
      if (!this.#config || !this.#start(this.#config)) return;
      // The first key frame after a configuration carries it.
      const joined = new Uint8Array(this.#config.length + data.length);
      joined.set(this.#config, 0);
      joined.set(data, this.#config.length);
      data = joined;
      this.#configured = true;
    }
    if (!this.#configured || !this.#decoder || this.#decoder.state !== "configured") return;
    try {
      this.#decoder.decode(new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp, data }));
    } catch {
      // A frame the decoder refuses: wait for the next key frame.
      this.#configured = false;
    }
  }

  #start(config: Uint8Array): boolean {
    const codec = avcCodec(config);
    if (!codec) return false;
    this.#decoder?.close();
    const decoder = new VideoDecoder({
      output: (frame) => this.#draw(frame),
      error: () => {
        // The stream starts again from its next configuration and key frame.
        this.#configured = false;
      },
    });
    try {
      decoder.configure({ codec, optimizeForLatency: true });
    } catch {
      this.#onState({ kind: "unsupported" });
      return false;
    }
    this.#decoder = decoder;
    return true;
  }

  #draw(frame: VideoFrame): void {
    if (this.#closed) {
      frame.close();
      return;
    }
    const { displayWidth: width, displayHeight: height } = frame;
    if (this.#canvas.width !== width || this.#canvas.height !== height) {
      this.#canvas.width = width;
      this.#canvas.height = height;
    }
    this.#canvas.getContext("2d")?.drawImage(frame, 0, 0, width, height);
    frame.close();
    this.#onState({ kind: "live", width, height });
  }

  /** Draws one JPEG. Pictures can come faster than a slow machine draws them: only the newest
   * one waits, and the rest are dropped. */
  #picture(jpeg: Uint8Array): void {
    if (this.#drawing) {
      this.#waiting = jpeg;
      return;
    }
    this.#drawing = true;
    createImageBitmap(new Blob([jpeg.slice()], { type: "image/jpeg" }))
      .then(
        (bitmap) => {
          if (this.#closed) {
            bitmap.close();
            return;
          }
          const { width, height } = bitmap;
          if (this.#canvas.width !== width || this.#canvas.height !== height) {
            this.#canvas.width = width;
            this.#canvas.height = height;
          }
          this.#canvas.getContext("2d")?.drawImage(bitmap, 0, 0, width, height);
          bitmap.close();
          this.#size = { width, height };
          this.#onState({ kind: "live", width, height });
        },
        () => {
          // A picture that would not decode: the next one replaces it.
        },
      )
      .finally(() => {
        this.#drawing = false;
        const next = this.#waiting;
        this.#waiting = null;
        if (next && !this.#closed) this.#picture(next);
      });
  }

  close(): void {
    this.#closed = true;
    if (this.#decoder && this.#decoder.state !== "closed") this.#decoder.close();
    this.#decoder = null;
  }
}

/**
 * A point of the pane's canvas as a point of the video: the canvas shows the whole video, scaled
 * to fit, so the two differ only by that scale. `null` outside the picture.
 */
export function videoPoint(
  clientX: number,
  clientY: number,
  box: { left: number; top: number; width: number; height: number },
  video: { width: number; height: number },
): { x: number; y: number } | null {
  if (box.width <= 0 || box.height <= 0) return null;
  const x = ((clientX - box.left) / box.width) * video.width;
  const y = ((clientY - box.top) / box.height) * video.height;
  if (x < 0 || y < 0 || x >= video.width || y >= video.height) return null;
  return { x: Math.floor(x), y: Math.floor(y) };
}

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
