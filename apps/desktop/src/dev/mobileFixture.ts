/**
 * An in-memory stand-in for the devices (docs/superpowers/plans/2026-11-21-mobile.md), for the
 * fixture and the unit tests: it holds one status, changes it the way the app would, and records
 * what the pane asked for. There is no device behind it, so no video.
 */
import type { ChatDevice, DeviceInfo, DeviceKey, MobileStatus, TouchPhase } from "../bindings";
import type { MobileApi } from "../lib/mobile";

const LICENSE = `Terms and Conditions

This is the Android Software Development Kit License Agreement

1. Introduction

1.1 The Android Software Development Kit (referred to in the License Agreement as the "SDK" and specifically including the Android system files, packaged APIs, and Google APIs add-ons) is licensed to you subject to the terms of the License Agreement. The License Agreement forms a legally binding contract between you and Google in relation to your use of the SDK.

2. Accepting the License Agreement

2.1 In order to use the SDK, you must first agree to the License Agreement. You may not use the SDK if you do not accept the License Agreement.

${"3. More terms follow in the real licence, which is several pages long.\n\n".repeat(12)}`;

/** iPhone simulators as Windows and Linux see them: not possible here. */
export const NO_IPHONE: MobileStatus["setup"]["ios"] = {
  possible: false,
  ready: false,
  detail: null,
  missing: "iPhone simulators run only on a Mac.",
  tool: { state: "missing" },
};

/** iPhone simulators on a Mac with Xcode, AXe not fetched yet. */
export const MAC_IPHONE: MobileStatus["setup"]["ios"] = {
  possible: true,
  ready: true,
  detail: "Xcode 26.1, iPhone 17 Pro, iOS 26.1",
  missing: null,
  tool: { state: "missing" },
};

/** A status with Android set up and nothing running, changed by `change`. */
export function mobileStatus(change: Partial<MobileStatus> = {}): MobileStatus {
  return {
    enabled: true,
    unavailable: null,
    setup: {
      adb: true,
      emulator: true,
      acceleration: {
        usable: true,
        detail: "WHPX(10.0.26100) is installed and usable.",
        fix: null,
      },
      fetch: { state: "idle" },
      ios: NO_IPHONE,
    },
    devices: [],
    chats: [],
    summons: [],
    ...change,
  };
}

/** The same on a Mac: Android set up, and Xcode found. */
export function macStatus(change: Partial<MobileStatus> = {}): MobileStatus {
  const status = mobileStatus(change);
  return { ...status, setup: { ...status.setup, ios: MAC_IPHONE, ...change.setup } };
}

export const simulator = (change: Partial<DeviceInfo> = {}): DeviceInfo => ({
  id: "o7_iphone_1",
  kind: "simulator",
  title: "iPhone 1",
  state: "stopped",
  error: null,
  chat_id: null,
  pid: null,
  ...change,
});

export const emulator = (change: Partial<DeviceInfo> = {}): DeviceInfo => ({
  id: "o7_phone_1",
  kind: "emulator",
  title: "Phone 1",
  state: "stopped",
  error: null,
  chat_id: null,
  pid: null,
  ...change,
});

export const phone = (change: Partial<DeviceInfo> = {}): DeviceInfo => ({
  id: "adb:R58M12ABCDE",
  kind: "phone",
  title: "Pixel 8",
  state: "ready",
  error: null,
  chat_id: null,
  pid: null,
  ...change,
});

export const held = (chatId: string, change: Partial<ChatDevice> = {}): ChatDevice => ({
  chat_id: chatId,
  device_id: "o7_phone_1",
  kind: "emulator",
  title: "Phone 1",
  starting: false,
  live: true,
  agent_allowed: true,
  agent: null,
  error: null,
  ...change,
});

/** The offer the pane shows before anything is downloaded. */
export const OFFER: MobileStatus["setup"]["fetch"] = {
  state: "offer",
  items: [
    { name: "Android SDK Platform-Tools", size_mb: 7.8 },
    { name: "Android Emulator", size_mb: 435.5 },
    { name: "Google APIs Intel x86_64 Atom System Image", size_mb: 1807.6 },
  ],
  total_mb: 2250.9,
  license: LICENSE,
};

export class FakeMobile implements MobileApi {
  /** Every call the pane made, as `name` or `name:argument`. */
  readonly calls: string[] = [];
  #status: MobileStatus;
  readonly #listeners = new Set<(status: MobileStatus) => void>();
  /** The pane's video listeners, by chat: a test may push packets to one. */
  readonly watching = new Map<string, (packet: Uint8Array) => void>();

  constructor(status: MobileStatus = mobileStatus()) {
    this.#status = status;
  }

  current(): MobileStatus {
    return this.#status;
  }

  /** Replaces the status, as an event from the app would. */
  set(status: MobileStatus): void {
    this.#status = status;
    for (const listener of [...this.#listeners]) listener(status);
  }

  change(change: Partial<MobileStatus>): void {
    this.set({ ...this.#status, ...change });
  }

  #fetch(fetch: MobileStatus["setup"]["fetch"]): void {
    this.change({ setup: { ...this.#status.setup, fetch } });
  }

  async status(): Promise<MobileStatus> {
    return this.#status;
  }
  async refresh(): Promise<MobileStatus> {
    this.calls.push("refresh");
    return this.#status;
  }
  async fetchOffer(): Promise<void> {
    this.calls.push("fetchOffer");
    this.#fetch(OFFER);
  }
  async fetchStart(): Promise<void> {
    this.calls.push("fetchStart");
    this.#fetch({
      state: "running",
      name: "Android Emulator",
      phase: "downloading",
      received_mb: 212.4,
      total_mb: 2250.9,
    });
  }
  async fetchCancel(): Promise<void> {
    this.calls.push("fetchCancel");
    this.#fetch({ state: "idle" });
  }
  async attach(chatId: string, deviceId: string | null): Promise<void> {
    this.calls.push(`attach:${deviceId ?? "any"}`);
    // "ios" asks for any of Ghost's iPhones, as `ghost_devices::manager::ANY_IPHONE` does.
    const kind = deviceId === "ios" ? "simulator" : "emulator";
    const device =
      this.#status.devices.find((entry) => entry.id === deviceId) ??
      this.#status.devices.find((entry) => entry.kind === kind) ??
      (kind === "simulator" ? simulator() : emulator());
    const devices = this.#status.devices.some((entry) => entry.id === device.id)
      ? this.#status.devices
      : [...this.#status.devices, device];
    this.change({
      devices: devices.map((entry) =>
        entry.id === device.id
          ? { ...entry, chat_id: chatId, state: entry.kind === "phone" ? "ready" : "starting" }
          : entry,
      ),
      chats: [
        ...this.#status.chats.filter((entry) => entry.chat_id !== chatId),
        held(chatId, {
          device_id: device.id,
          kind: device.kind,
          title: device.title,
          starting: true,
          live: false,
          agent_allowed: device.kind !== "phone",
        }),
      ],
    });
  }
  async detach(chatId: string): Promise<void> {
    this.calls.push("detach");
    this.change({
      devices: this.#status.devices.map((entry) =>
        entry.chat_id === chatId ? { ...entry, chat_id: null } : entry,
      ),
      chats: this.#status.chats.filter((entry) => entry.chat_id !== chatId),
    });
  }
  async stopDevice(deviceId: string): Promise<void> {
    this.calls.push(`stopDevice:${deviceId}`);
    this.change({
      devices: this.#status.devices.map((entry) =>
        entry.id === deviceId ? { ...entry, state: "stopped", chat_id: null } : entry,
      ),
      chats: this.#status.chats.filter((entry) => entry.device_id !== deviceId),
    });
  }
  async allowAgent(chatId: string, allowed: boolean): Promise<void> {
    this.calls.push(`allowAgent:${allowed}`);
    this.change({
      chats: this.#status.chats.map((entry) =>
        entry.chat_id === chatId ? { ...entry, agent_allowed: allowed } : entry,
      ),
    });
  }
  async watch(chatId: string, onPacket: (packet: Uint8Array) => void): Promise<() => void> {
    this.calls.push("watch");
    this.watching.set(chatId, onPacket);
    return () => {
      this.calls.push("unwatch");
      // Only its own stream, as the app's stop does.
      if (this.watching.get(chatId) === onPacket) this.watching.delete(chatId);
    };
  }
  async touch(_chatId: string, phase: TouchPhase, x: number, y: number): Promise<void> {
    this.calls.push(`touch:${phase}:${x},${y}`);
  }
  async scroll(
    _chatId: string,
    x: number,
    y: number,
    horizontal: number,
    vertical: number,
  ): Promise<void> {
    this.calls.push(`scroll:${x},${y}:${horizontal},${vertical}`);
  }
  async key(_chatId: string, key: DeviceKey): Promise<void> {
    this.calls.push(`key:${key}`);
  }
  async text(_chatId: string, text: string): Promise<void> {
    this.calls.push(`text:${text}`);
  }
  async rotate(): Promise<void> {
    this.calls.push("rotate");
  }
  async screenshot(): Promise<string> {
    this.calls.push("screenshot");
    return "C:\\Users\\sam\\Pictures\\ghost-device-1795300000.png";
  }
  listen(listener: (status: MobileStatus) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }
}
