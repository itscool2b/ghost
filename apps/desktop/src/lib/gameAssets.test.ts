import { describe, expect, it } from "vitest";
import type { GameAsset } from "../bindings";
import { FakeGame, gameStatus, running } from "../dev/gameFixture";
import { GameStore } from "./game";
import {
  AssetBytes,
  assetFolder,
  assetName,
  clock,
  counts,
  frameCount,
  frameRect,
  freshSince,
  guessFrames,
  MAKERS,
  matching,
  peaks,
  sizeText,
  soundLength,
} from "./gameAssets";

const asset = (path: string, kind: GameAsset["kind"] = "picture", modified = 10): GameAsset => ({
  path,
  kind,
  bytes: 100,
  modified,
  too_large: false,
});

describe("an asset's words", () => {
  it("names a file and its folder", () => {
    expect(assetName("art/player/run.png")).toBe("run.png");
    expect(assetFolder("art/player/run.png")).toBe("art/player");
    expect(assetName("icon.svg")).toBe("icon.svg");
    expect(assetFolder("icon.svg")).toBe("");
  });

  it("writes sizes and lengths as a person reads them", () => {
    expect(sizeText(412)).toBe("412 B");
    expect(sizeText(4044)).toBe("4 KB");
    expect(sizeText(15_728_640)).toBe("15.0 MB");
    expect(clock(7)).toBe("0:07");
    expect(clock(83.4)).toBe("1:23");
    expect(soundLength(0.251)).toBe("0.25 s");
    expect(soundLength(83.4)).toBe("1:23");
    expect(soundLength(Number.NaN)).toBe("");
  });
});

describe("a list of assets", () => {
  const list = [
    asset("art/player/run.png"),
    asset("art/tiles/Grass.png"),
    asset("sound/jump.wav", "sound"),
    asset("models/crate.glb", "model"),
  ];

  it("counts each kind and finds by kind and by every word typed", () => {
    expect(counts(list)).toEqual({ picture: 2, sound: 1, model: 1 });
    expect(matching(list, null, "").length).toBe(4);
    expect(matching(list, "picture", "").map((entry) => entry.path)).toEqual([
      "art/player/run.png",
      "art/tiles/Grass.png",
    ]);
    expect(matching(list, null, "  tiles GRASS ").map((entry) => entry.path)).toEqual([
      "art/tiles/Grass.png",
    ]);
    expect(matching(list, "sound", "grass")).toEqual([]);
  });

  it("knows what is new or changed since the list before", () => {
    const before = new Map([
      ["art/player/run.png", 10],
      ["sound/jump.wav", 10],
    ]);
    const now = [
      asset("art/player/run.png", "picture", 12),
      asset("sound/jump.wav", "sound", 10),
      asset("art/enemy.png", "picture", 11),
    ];
    expect(freshSince(before, now).map((entry) => entry.path)).toEqual([
      "art/player/run.png",
      "art/enemy.png",
    ]);
    expect(freshSince(new Map(), [])).toEqual([]);
  });
});

describe("a sprite sheet", () => {
  it("is guessed from its shape: a strip of squares is that many frames", () => {
    expect(guessFrames(128, 32)).toEqual({ across: 4, down: 1 });
    expect(guessFrames(16, 96)).toEqual({ across: 1, down: 6 });
    expect(guessFrames(64, 64)).toEqual({ across: 1, down: 1 });
    expect(guessFrames(320, 180)).toEqual({ across: 1, down: 1 });
    // Too many squares to be frames, and nothing at all.
    expect(guessFrames(6500, 100)).toEqual({ across: 1, down: 1 });
    expect(guessFrames(0, 0)).toEqual({ across: 1, down: 1 });
  });

  it("is cut across and then down, and comes round again", () => {
    expect(frameRect(0, 4, 2, 128, 64)).toEqual({ x: 0, y: 0, width: 32, height: 32 });
    expect(frameRect(5, 4, 2, 128, 64)).toEqual({ x: 32, y: 32, width: 32, height: 32 });
    expect(frameRect(8, 4, 2, 128, 64)).toEqual({ x: 0, y: 0, width: 32, height: 32 });
    expect(frameRect(-1, 4, 2, 128, 64)).toEqual({ x: 96, y: 32, width: 32, height: 32 });
  });

  it("takes a count a person typed within what a sheet holds", () => {
    expect(frameCount("4")).toBe(4);
    expect(frameCount("0")).toBe(1);
    expect(frameCount("900")).toBe(64);
    expect(frameCount("")).toBe(1);
    expect(frameCount("abc")).toBe(1);
    expect(frameCount(2.6)).toBe(3);
  });
});

describe("a sound's shape", () => {
  it("is the quietest and loudest of each slice", () => {
    expect(peaks([0, 0.5, -0.5, 1, -1, 0.25, 0, 0], 4)).toEqual([
      [0, 0.5],
      [-0.5, 1],
      [-1, 0.25],
      [0, 0],
    ]);
    // More slices than samples, no samples, and samples out of range.
    expect(peaks([0.5], 3).length).toBe(3);
    expect(peaks([], 2)).toEqual([
      [0, 0],
      [0, 0],
    ]);
    expect(peaks([4, -4], 1)).toEqual([[-1, 1]]);
  });
});

describe("the makers", () => {
  it("are named with a guide of their own, on their own site", () => {
    expect(MAKERS.map((maker) => maker.name)).toEqual(["Meshy", "PixelLab", "ElevenLabs"]);
    for (const maker of MAKERS) {
      const host = new URL(maker.guide).hostname;
      expect(maker.guide.startsWith("https://")).toBe(true);
      expect(
        host.endsWith(`${maker.name.toLowerCase()}.ai`) || host.endsWith("elevenlabs.io"),
      ).toBe(true);
    }
  });
});

describe("asset bytes", () => {
  const data = { kind: "picture" as const, mime: "image/png", base64: btoa("png") };

  it("reads an asset once however many show it, and again when it has changed", async () => {
    const read: string[] = [];
    const bytes = new AssetBytes(async (path) => {
      read.push(path);
      return data;
    });
    const first = bytes.acquire({ path: "a.png", modified: 1 });
    const second = bytes.acquire({ path: "a.png", modified: 1 });
    expect((await first.promise).bytes).toEqual(new Uint8Array([112, 110, 103]));
    expect((await second.promise).url).toBe((await first.promise).url);
    expect(read).toEqual(["a.png"]);
    const changed = bytes.acquire({ path: "a.png", modified: 2 });
    await changed.promise;
    expect(read).toEqual(["a.png", "a.png"]);
    bytes.clear();
    expect(bytes.size()).toBe(0);
  });

  it("lets go of what nothing shows once more than a few are kept, the longest unused first", async () => {
    const bytes = new AssetBytes(async () => data, 2);
    const held = [1, 2, 3].map((n) => bytes.acquire({ path: `${n}.png`, modified: 1 }));
    await Promise.all(held.map((entry) => entry.promise));
    // All three are shown: none can go.
    expect(bytes.size()).toBe(3);
    held[0]?.release();
    expect(bytes.size()).toBe(2);
    held[1]?.release();
    held[1]?.release();
    // Within what is kept: it stays, for the next card that shows it.
    expect(bytes.size()).toBe(2);
  });

  it("asks again for an asset that could not be read", async () => {
    let fail = true;
    const bytes = new AssetBytes(async () => {
      if (fail) throw new Error("gone");
      return data;
    });
    const first = bytes.acquire({ path: "a.png", modified: 1 });
    await expect(first.promise).rejects.toThrow("gone");
    first.release();
    fail = false;
    await new Promise((done) => setTimeout(done, 0));
    await expect(bytes.acquire({ path: "a.png", modified: 1 }).promise).resolves.toBeTruthy();
  });
});

describe("what a turn leaves", () => {
  const settle = () => new Promise((done) => setTimeout(done, 0));

  it("is said once for a game chat: the first list is what was there", async () => {
    const game = new FakeGame(gameStatus({ chats: [running("c1")] }));
    const store = new GameStore(game);
    store.look("c1");
    await settle();
    await settle();
    expect(store.freshAssets("c1")).toEqual([]);
    game.made("art/enemy.png");
    store.look("c1");
    await settle();
    await settle();
    expect(store.freshAssets("c1").map((entry) => entry.path)).toEqual(["art/enemy.png"]);
    // Opening one from the chat asks the pane for it, and what was new is seen.
    store.wantAsset("c1", "art/enemy.png");
    expect(store.wantedAsset("c1")).toBe("art/enemy.png");
    expect(store.freshAssets("c1")).toEqual([]);
    store.shownAsset("c1");
    expect(store.wantedAsset("c1")).toBe(null);
    // A folder that did not change says nothing.
    store.look("c1");
    await settle();
    await settle();
    expect(store.freshAssets("c1")).toEqual([]);
    // Two turns' worth wait together until they are seen, and dismissing clears them.
    game.made("art/a.png");
    store.look("c1");
    await settle();
    await settle();
    game.made("art/b.png");
    store.look("c1");
    await settle();
    await settle();
    expect(store.freshAssets("c1").map((entry) => entry.path)).toEqual(["art/b.png", "art/a.png"]);
    store.seenAssets("c1");
    expect(store.freshAssets("c1")).toEqual([]);
  });

  it("is never listed for a chat with no game", async () => {
    const game = new FakeGame(gameStatus({ chats: [] }));
    const store = new GameStore(game);
    store.look("c1");
    await settle();
    await settle();
    expect(game.calls.includes("assets")).toBe(false);
  });
});
