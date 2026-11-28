import { describe, expect, it } from "vitest";
import type { GameInput } from "../bindings";
import { chatGame, FakeGame, gameStatus, project, running } from "../dev/gameFixture";
import {
  engineName,
  GamePanels,
  GameStore,
  KeyboardSender,
  megabytes,
  mouseButton,
  SUMMON_QUIET_MS,
} from "./game";

const key = (code: string, change: Partial<Parameters<KeyboardSender["down"]>[0]> = {}) => ({
  code,
  key: code.startsWith("Key") ? code.slice(3).toLowerCase() : code,
  repeat: false,
  shiftKey: false,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  ...change,
});

/** Lets the store's first status and any event after it arrive. */
const settled = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe("KeyboardSender", () => {
  it("sends a key down once and up once, by its code", () => {
    const sent: GameInput[] = [];
    const keys = new KeyboardSender((events) => sent.push(...events));
    expect(keys.down(key("KeyD"))).toBe(true);
    // The system's own repeats of a held key, and a second down without an up, send nothing.
    expect(keys.down(key("KeyD", { repeat: true }))).toBe(true);
    expect(keys.down(key("KeyD"))).toBe(true);
    expect(keys.up({ code: "KeyD" })).toBe(true);
    expect(keys.up({ code: "KeyD" })).toBe(false);
    expect(sent).toEqual([
      { t: "key", code: "KeyD", down: true, text: "d", shift: false, ctrl: false, alt: false },
      { t: "key", code: "KeyD", down: false },
    ]);
  });

  it("sends what a key types as text, and never a key's name", () => {
    const sent: GameInput[] = [];
    const keys = new KeyboardSender((events) => sent.push(...events));
    keys.down(key("Space", { key: " " }));
    keys.down(key("ArrowLeft", { key: "ArrowLeft" }));
    keys.down(key("KeyE", { key: "É", shiftKey: true }));
    expect(
      sent.map((event) => (event.t === "key" ? [event.code, event.text, event.shift] : null)),
    ).toEqual([
      ["Space", " ", false],
      ["ArrowLeft", "", false],
      ["KeyE", "É", true],
    ]);
  });

  it("keeps Ghost's chords and the web view's keys out of the game", () => {
    const sent: GameInput[] = [];
    const keys = new KeyboardSender((events) => sent.push(...events));
    expect(keys.down(key("KeyG", { ctrlKey: true, shiftKey: true }))).toBe(false);
    expect(keys.down(key("KeyK", { metaKey: true }))).toBe(false);
    expect(keys.down(key("F5"))).toBe(false);
    expect(keys.down(key("F12"))).toBe(false);
    expect(keys.down(key(""))).toBe(false);
    // The rest of the function row is the game's.
    expect(keys.down(key("F1"))).toBe(true);
    expect(sent).toHaveLength(1);
  });

  it("releases every key still down when the pane loses the keyboard", () => {
    const sent: GameInput[] = [];
    const keys = new KeyboardSender((events) => sent.push(...events));
    keys.down(key("ArrowRight"));
    keys.down(key("Space", { key: " " }));
    keys.up({ code: "ArrowRight" });
    keys.down(key("KeyW"));
    sent.length = 0;
    keys.releaseAll();
    expect(sent).toEqual([
      { t: "key", code: "Space", down: false },
      { t: "key", code: "KeyW", down: false },
    ]);
    expect(keys.held()).toBe(0);
    // Nothing is down: nothing is sent, and a late key-up of one of them is not the game's.
    keys.releaseAll();
    expect(keys.up({ code: "Space" })).toBe(false);
    expect(sent).toHaveLength(2);
  });
});

describe("GameStore", () => {
  it("offers the pane's button only where there is a game, and never while Game is off", async () => {
    const fake = new FakeGame(
      gameStatus({
        chats: [
          chatGame("godot"),
          chatGame("empty", { project: null, tools: false }),
          chatGame("turned-on", { project: null, tools: true }),
          running("playing", { project: null, tools: false }),
          chatGame("unity", { project: project({ engine: "unity" }), tools: false }),
        ],
      }),
    );
    const store = new GameStore(fake);
    const stop = store.subscribe(() => {});
    await settled();
    expect(store.usable()).toBe(true);
    expect(store.offered("godot")).toBe(true);
    expect(store.offered("empty")).toBe(false);
    expect(store.offered("turned-on")).toBe(true);
    expect(store.offered("playing")).toBe(true);
    expect(store.offered("unity")).toBe(true);
    expect(store.offered("never-looked-at")).toBe(false);
    fake.change({ enabled: false });
    expect(store.usable()).toBe(false);
    expect(store.offered("godot")).toBe(false);
    stop();
  });

  it("looks at a chat once however many turns end at the same moment", async () => {
    const fake = new FakeGame();
    const store = new GameStore(fake);
    store.look("chat");
    store.look("chat");
    store.look("other");
    await settled();
    expect(fake.calls).toEqual(["look:chat", "look:other"]);
    store.look("chat");
    await settled();
    expect(fake.calls.filter((call) => call === "look:chat")).toHaveLength(2);
  });

  it("opens the pane for an agent's call, and leaves it closed where the person closed it", async () => {
    let now = 1_000_000;
    // A call made before this window started is in the first status: it opens nothing.
    const fake = new FakeGame(gameStatus({ summons: [{ chat_id: "old", seq: 4 }] }));
    const store = new GameStore(fake, () => now);
    const stop = store.subscribe(() => {});
    await settled();
    expect(store.summoned("old")).toBe(false);

    fake.summon("chat");
    expect(store.summoned("chat")).toBe(true);
    store.answer("chat");
    expect(store.summoned("chat")).toBe(false);

    // The person closes the pane while the agent is at it: its next calls leave it closed.
    now += 5_000;
    fake.summon("chat");
    expect(store.summoned("chat")).toBe(true);
    store.closed("chat");
    now += 5_000;
    fake.summon("chat");
    expect(store.summoned("chat")).toBe(false);

    // After a quiet minute a new stretch of work opens it again.
    now += SUMMON_QUIET_MS + 1;
    fake.summon("chat");
    expect(store.summoned("chat")).toBe(true);
    // A summon nobody answered goes stale with the stretch it came from.
    now += SUMMON_QUIET_MS + 1;
    expect(store.summoned("chat")).toBe(false);
    stop();
  });

  it("reads as off when the app cannot be reached", async () => {
    const fake = new FakeGame();
    fake.status = async () => {
      throw new Error("no app");
    };
    const store = new GameStore(fake);
    const stop = store.subscribe(() => {});
    await settled();
    expect(store.usable()).toBe(false);
    expect(store.status()?.chats).toEqual([]);
    stop();
  });
});

describe("GamePanels", () => {
  it("remembers each chat's pane, and stays closed when storage fails", () => {
    const kept = new Map<string, string>();
    const panels = new GamePanels({
      getItem: (name) => kept.get(name) ?? null,
      setItem: (name, value) => void kept.set(name, value),
    });
    let changes = 0;
    panels.subscribe(() => {
      changes += 1;
    });
    expect(panels.open("a")).toBe(false);
    panels.set("a", true);
    expect(panels.open("a")).toBe(true);
    expect(panels.open("b")).toBe(false);
    expect(changes).toBe(1);
    expect([...kept.keys()]).toEqual(["ghost.game-panel.a"]);

    const broken = new GamePanels({
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("full");
      },
    });
    broken.set("a", true);
    expect(broken.open("a")).toBe(false);
    expect(new GamePanels(null).open("a")).toBe(false);
  });
});

describe("words", () => {
  it("names engines as people write them", () => {
    expect(engineName("godot")).toBe("Godot");
    expect(engineName("unreal")).toBe("Unreal Engine");
    expect(engineName("game_maker")).toBe("GameMaker");
    expect(engineName("mono_game")).toBe("MonoGame");
  });

  it("writes a download's size", () => {
    expect(megabytes(86_013_866)).toBe("82 MB");
    expect(megabytes(1_200_000_000)).toBe("1.1 GB");
    expect(megabytes(0)).toBe("0 MB");
    expect(megabytes(Number.NaN)).toBe("0 MB");
  });

  it("knows a pointer's three buttons and no others", () => {
    expect([0, 1, 2, 3, -1].map(mouseButton)).toEqual(["left", "middle", "right", null, null]);
  });
});
