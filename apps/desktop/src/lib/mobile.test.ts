import { afterEach, describe, expect, it, vi } from "vitest";
import { emulator, FakeMobile, held, mobileStatus, phone } from "../dev/mobileFixture";
import {
  avcCodec,
  DevicePanels,
  deviceKey,
  MobileStore,
  megabytes,
  ScreenPlayer,
  type ScreenState,
  videoPoint,
} from "./mobile";

const flush = () => new Promise((done) => setTimeout(done, 0));

describe("the devices' store", () => {
  it("reads one status and replaces it whole on every change", async () => {
    const fake = new FakeMobile(mobileStatus({ devices: [emulator()] }));
    const store = new MobileStore(fake);
    expect(store.status()).toBeNull();
    expect(store.usable()).toBe(false);
    let changes = 0;
    const stop = store.subscribe(() => {
      changes += 1;
    });
    await flush();
    expect(changes).toBe(1);
    expect(store.usable()).toBe(true);
    expect(store.devices().map((device) => device.title)).toEqual(["Phone 1"]);
    expect(store.held("chat")).toBeNull();
    fake.change({ chats: [held("chat", { agent: "Tapping d3" })] });
    expect(changes).toBe(2);
    expect(store.held("chat")?.agent).toBe("Tapping d3");
    expect(store.held("another")).toBeNull();
    stop();
    fake.change({ chats: [] });
    expect(changes).toBe(2);
  });

  it("is not offered while Mobile is off or Android's tools do not exist here", async () => {
    for (const status of [
      mobileStatus({ enabled: false }),
      mobileStatus({ unavailable: "Android's tools are not published for this kind of computer." }),
    ]) {
      const store = new MobileStore(new FakeMobile(status));
      store.subscribe(() => {});
      await flush();
      expect(store.usable()).toBe(false);
    }
  });

  it("offers nothing when the app cannot be asked", async () => {
    const fake = new FakeMobile();
    fake.status = () => Promise.reject(new Error("no app"));
    const store = new MobileStore(fake);
    store.subscribe(() => {});
    await flush();
    expect(store.usable()).toBe(false);
    expect(store.status()?.unavailable).toBe("Devices are not available here.");
  });

  it("looks again when the pane opens", async () => {
    const fake = new FakeMobile(mobileStatus({ devices: [phone()] }));
    const store = new MobileStore(fake);
    store.refresh();
    await flush();
    expect(fake.calls).toEqual(["refresh"]);
    expect(store.devices()).toHaveLength(1);
  });
});

describe("each chat's pane", () => {
  it("is closed until opened, per chat, and survives a storage that fails", () => {
    const kept = new Map<string, string>();
    const panels = new DevicePanels({
      getItem: (key) => kept.get(key) ?? null,
      setItem: (key, value) => void kept.set(key, value),
    });
    expect(panels.open("a")).toBe(false);
    let changes = 0;
    panels.subscribe(() => {
      changes += 1;
    });
    panels.set("a", true);
    expect(panels.open("a")).toBe(true);
    expect(panels.open("b")).toBe(false);
    expect(changes).toBe(1);
    expect([...kept.keys()]).toEqual(["o7.device-panel.a"]);
    panels.set("a", false);
    expect(panels.open("a")).toBe(false);

    const broken = new DevicePanels({
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("full");
      },
    });
    broken.set("a", true);
    expect(broken.open("a")).toBe(false);
    expect(new DevicePanels(null).open("a")).toBe(false);
  });
});

describe("the video", () => {
  // The configuration scrcpy sent from a real emulator: a sequence and a picture parameter set.
  const CONFIG = Uint8Array.from([
    0, 0, 0, 1, 0x67, 0x42, 0xc0, 0x29, 0x8d, 0x68, 0x09, 0x00, 0xa1, 0xa4, 0x20, 0x20, 0x20, 0x20,
    0xf0, 0x88, 0x46, 0xa0, 0, 0, 0, 1, 0x68, 0xce, 0x3c, 0x80,
  ]);

  it("names the codec from the stream's own parameter set", () => {
    expect(avcCodec(CONFIG)).toBe("avc1.42c029");
    // A three-byte start code, and a set that is not first.
    expect(
      avcCodec(Uint8Array.from([0, 0, 1, 0x68, 1, 2, 3, 0, 0, 1, 0x27, 0x64, 0x00, 0x1f, 9])),
    ).toBe("avc1.64001f");
    expect(avcCodec(Uint8Array.from([0, 0, 0, 1, 0x68, 0xce]))).toBeNull();
    expect(avcCodec(new Uint8Array())).toBeNull();
    expect(avcCodec(Uint8Array.from([0, 0, 1, 0x67, 0x42]))).toBeNull();
  });

  it("maps a point of the pane to a point of the video, and nothing outside it", () => {
    const box = { left: 100, top: 50, width: 360, height: 800 };
    const video = { width: 720, height: 1600 };
    expect(videoPoint(100, 50, box, video)).toEqual({ x: 0, y: 0 });
    expect(videoPoint(280, 450, box, video)).toEqual({ x: 360, y: 800 });
    expect(videoPoint(459.9, 849.9, box, video)).toEqual({ x: 719, y: 1599 });
    expect(videoPoint(460, 450, box, video)).toBeNull();
    expect(videoPoint(99, 450, box, video)).toBeNull();
    expect(videoPoint(280, 850, box, video)).toBeNull();
    expect(videoPoint(280, 450, { ...box, width: 0 }, video)).toBeNull();
  });

  it("forwards the keys a phone has and leaves the rest as text", () => {
    expect(deviceKey("Enter")).toBe("enter");
    expect(deviceKey("Backspace")).toBe("delete");
    expect(deviceKey("Delete")).toBe("forward_delete");
    expect(deviceKey("Escape")).toBe("back");
    expect(deviceKey("ArrowDown")).toBe("down");
    expect(deviceKey("a")).toBeNull();
    expect(deviceKey("F5")).toBeNull();
    expect(deviceKey("Shift")).toBeNull();
  });

  it("says sizes the way people do", () => {
    expect(megabytes(7.8)).toBe("8 MB");
    expect(megabytes(435.5)).toBe("436 MB");
    expect(megabytes(1807.6)).toBe("1.8 GB");
    expect(megabytes(2250.9)).toBe("2.2 GB");
    expect(megabytes(0)).toBe("0 MB");
    expect(megabytes(null)).toBe("0 MB");
    expect(megabytes(Number.NaN)).toBe("0 MB");
  });

  describe("the player", () => {
    interface Chunk {
      type: string;
      timestamp: number;
      data: Uint8Array;
    }
    let decoders: FakeDecoder[] = [];
    class FakeDecoder {
      state = "unconfigured";
      configured: unknown = null;
      readonly chunks: Chunk[] = [];
      readonly init: { output: (frame: unknown) => void; error: (error: Error) => void };
      constructor(init: { output: (frame: unknown) => void; error: (error: Error) => void }) {
        this.init = init;
        decoders.push(this);
      }
      configure(config: unknown) {
        this.configured = config;
        this.state = "configured";
      }
      decode(chunk: Chunk) {
        this.chunks.push(chunk);
      }
      close() {
        this.state = "closed";
      }
    }
    const canvas = () =>
      ({
        width: 0,
        height: 0,
        getContext: () => ({ drawImage: () => {} }),
      }) as unknown as HTMLCanvasElement;
    const frame = (kind: number, pts: number, data: number[]) => {
      const packet = new Uint8Array(9 + data.length);
      packet[0] = kind;
      new DataView(packet.buffer).setBigUint64(1, BigInt(pts));
      packet.set(data, 9);
      return packet;
    };
    const install = () => {
      decoders = [];
      vi.stubGlobal("VideoDecoder", FakeDecoder);
      vi.stubGlobal(
        "EncodedVideoChunk",
        class {
          type: string;
          timestamp: number;
          data: Uint8Array;
          constructor(init: Chunk) {
            this.type = init.type;
            this.timestamp = init.timestamp;
            this.data = init.data;
          }
        },
      );
    };
    afterEach(() => vi.unstubAllGlobals());

    it("says so where the web view cannot decode video", () => {
      vi.stubGlobal("VideoDecoder", undefined);
      const states: ScreenState[] = [];
      const player = new ScreenPlayer(canvas(), (state) => states.push(state));
      player.push(Uint8Array.from([1, 2, 208, 6, 64]));
      expect(states).toEqual([{ kind: "unsupported" }]);
      expect(player.size()).toBeNull();
    });

    it("starts at a key frame that carries the configuration, then feeds what follows", () => {
      install();
      const states: ScreenState[] = [];
      const element = canvas();
      const player = new ScreenPlayer(element, (state) => states.push(state));
      expect(states).toEqual([{ kind: "waiting" }]);
      player.push(Uint8Array.from([1, 2, 208, 6, 64]));
      expect(player.size()).toEqual({ width: 720, height: 1600 });
      // A frame before any configuration cannot be drawn.
      player.push(frame(4, 10, [9, 9]));
      expect(decoders).toHaveLength(0);
      player.push(Uint8Array.from([2, ...CONFIG]));
      // Nor one that builds on a picture this viewer never had.
      player.push(frame(4, 20, [8, 8]));
      expect(decoders).toHaveLength(0);
      player.push(frame(3, 30, [1, 2, 3]));
      expect(decoders).toHaveLength(1);
      const decoder = decoders[0];
      expect(decoder?.configured).toEqual({ codec: "avc1.42c029", optimizeForLatency: true });
      expect(decoder?.chunks).toHaveLength(1);
      expect(decoder?.chunks[0]?.type).toBe("key");
      expect(decoder?.chunks[0]?.timestamp).toBe(30);
      expect([...(decoder?.chunks[0]?.data ?? [])]).toEqual([...CONFIG, 1, 2, 3]);
      player.push(frame(4, 40, [7]));
      expect(decoder?.chunks[1]).toMatchObject({ type: "delta", timestamp: 40 });
      expect([...(decoder?.chunks[1]?.data ?? [])]).toEqual([7]);

      // A decoded picture sizes the canvas and marks the screen live.
      decoder?.init.output({ displayWidth: 720, displayHeight: 1600, close: () => {} });
      expect(element.width).toBe(720);
      expect(element.height).toBe(1600);
      expect(states.at(-1)).toEqual({ kind: "live", width: 720, height: 1600 });

      // A new configuration (the device turned) starts a new decoder at its key frame.
      player.push(Uint8Array.from([2, ...CONFIG]));
      player.push(frame(4, 50, [6]));
      expect(decoder?.chunks).toHaveLength(2);
      player.push(frame(3, 60, [5]));
      expect(decoders).toHaveLength(2);
      expect(decoder?.state).toBe("closed");

      // After an error nothing is fed until the next key frame.
      decoders[1]?.init.error(new Error("bad frame"));
      player.push(frame(4, 70, [4]));
      expect(decoders[1]?.chunks).toHaveLength(1);

      player.push(Uint8Array.from([5]));
      expect(states.at(-1)).toEqual({ kind: "ended" });
      player.close();
      player.push(frame(3, 80, [3]));
      expect(decoders).toHaveLength(2);
    });

    it("ignores packets that are too short or of no known kind", () => {
      install();
      const player = new ScreenPlayer(canvas(), () => {});
      for (const packet of [[], [1, 2], [3, 0, 0], [9, 1, 2, 3, 4, 5, 6, 7, 8, 9]])
        player.push(Uint8Array.from(packet));
      expect(player.size()).toBeNull();
      expect(decoders).toHaveLength(0);
    });
  });
});
