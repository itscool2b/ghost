import { describe, expect, it } from "vitest";
import type { GamePicked, GameRecent, GameTunable, GameTunables } from "../bindings";
import { PAD_REST, padEvents, padRelease } from "./game";
import {
  areaOf,
  keepText,
  pickedLabel,
  pointText,
  recentText,
  shownValue,
  tunableRange,
  turned,
} from "./gameTell";

const picked = (change: Partial<GamePicked> = {}): GamePicked => ({
  path: "/root/Main/Player/Sprite",
  name: "Sprite",
  class: "Sprite2D",
  text: "",
  scene: "res://player.tscn",
  in_scene: "Sprite",
  scene_line: 9,
  script: "res://player.gd",
  script_on: "/root/Main/Player",
  area: [360.4, 480, 48, 72],
  ...change,
});

const game = { name: "Moon Hop", width: 1152, height: 648 };

describe("what pointing says", () => {
  it("gives the note, then the node, where it is defined and the script that drives it", () => {
    expect(pointText("  It sinks into the floor ", picked(), game, true)).toBe(
      [
        "It sinks into the floor",
        "",
        "Pointed at in the game (Moon Hop), frozen as it was:",
        "- Sprite (Sprite2D) at /root/Main/Player/Sprite",
        "   Defined in res://player.tscn:9 as Sprite",
        "   Script: res://player.gd (on /root/Main/Player)",
        "   48×72 at (360, 480) in a 1152×648 picture",
        "The attached image shows it outlined in the game.",
      ].join("\n"),
    );
  });

  it("says what a node shows, and leaves out what it does not have", () => {
    const text = pointText(
      "",
      picked({
        name: "Hud",
        class: "Label",
        path: "/root/Main/Hud",
        text: "Score 3",
        scene: "res://main.tscn",
        in_scene: ".",
        scene_line: null,
        script: "res://main.gd",
        script_on: "/root/Main/Hud",
      }),
      { ...game, name: null },
      false,
    );
    expect(text).toBe(
      [
        "Pointed at in the game, frozen as it was:",
        "- Hud (Label) at /root/Main/Hud",
        '   Shows: "Score 3"',
        "   Defined in res://main.tscn",
        "   Script: res://main.gd",
        "   48×72 at (360, 480) in a 1152×648 picture",
      ].join("\n"),
    );
  });

  it("says so when a node was made while the game ran and has no script", () => {
    const text = pointText(
      "x",
      picked({ scene: "", in_scene: "", scene_line: null, script: "", script_on: "" }),
      game,
      false,
    );
    expect(text).toContain("Made while the game ran: it is in no scene file.");
    expect(text).not.toContain("Script:");
  });

  it("names a node once when its name is its kind", () => {
    expect(pickedLabel(picked({ name: "Sprite2D" }))).toBe("Sprite2D");
    expect(pickedLabel(picked())).toBe("Sprite (Sprite2D)");
  });

  it("reads a place that was not a number as nothing", () => {
    expect(areaOf(picked({ area: [null, 2, null, 4] }))).toEqual([0, 2, 0, 4]);
  });
});

const recent = (change: Partial<GameRecent> = {}): GameRecent => ({
  replay: "r3",
  unrepeatable: null,
  picture: [],
  pictures: 12,
  seconds: 15,
  run_seconds: 48.52,
  inputs: 212,
  ...change,
});

describe("what the last seconds say", () => {
  it("names the pictures and how the agent plays the run again", () => {
    expect(recentText("The jump did nothing", recent(), "Moon Hop", true)).toBe(
      [
        "The jump did nothing",
        "",
        "The last 15 seconds of my play in Moon Hop are in the attached filmstrip: 12 pictures, left to right and top to bottom, the newest last. The run is 48.5 seconds long.",
        'Play that run again with game_replay and replay "r3": the same seed and the same 212 inputs at the same steps, frozen where mine ended. until_s stops it earlier.',
      ].join("\n"),
    );
  });

  it("goes without a note, without a picture, and for a run a second long", () => {
    const text = recentText("", recent({ seconds: 1, run_seconds: 1, pictures: 1 }), null, false);
    expect(
      text.startsWith("This is about the last 1 second of my play. The run is 1 second long."),
    ).toBe(true);
  });

  it("says why a run cannot be played again instead of naming one", () => {
    const text = recentText(
      "",
      recent({ replay: null, unrepeatable: "This run was given more input than Ghost keeps." }),
      "Moon Hop",
      true,
    );
    expect(text).toContain("This run was given more input than Ghost keeps.");
    expect(text).not.toContain("game_replay");
  });
});

const tunable = (change: Partial<GameTunable>): GameTunable => ({
  name: "speed",
  kind: "float",
  value: 200,
  initial: 200,
  min: null,
  max: null,
  step: null,
  ...change,
});

describe("tuned values", () => {
  it("takes a range from the script when it gives one", () => {
    expect(tunableRange(tunable({ min: 0, max: 320, step: 4 }))).toEqual({
      min: 0,
      max: 320,
      step: 4,
    });
    expect(tunableRange(tunable({ kind: "int", min: 1, max: 9 }))).toEqual({
      min: 1,
      max: 9,
      step: 1,
    });
    // A range with no step gets one that gives the slider about two hundred places.
    expect(tunableRange(tunable({ min: 0, max: 1 }))).toEqual({ min: 0, max: 1, step: 0.005 });
  });

  it("makes a range from the value itself when the script gives none", () => {
    expect(tunableRange(tunable({}))).toEqual({ min: 0, max: 400, step: 2 });
    expect(tunableRange(tunable({ value: -9.8, initial: -9.8 }))).toEqual({
      min: -19.6,
      max: 19.6,
      step: 0.1,
    });
    expect(tunableRange(tunable({ value: 0, initial: 0 }))).toEqual({
      min: 0,
      max: 1,
      step: 0.005,
    });
    expect(tunableRange(tunable({ kind: "int", value: 3, initial: 3 }))).toEqual({
      min: 0,
      max: 10,
      step: 1,
    });
    // Turned down to nothing, it still reaches what it started at and beyond.
    expect(tunableRange(tunable({ value: 0, initial: 200 }))).toEqual({
      min: 0,
      max: 400,
      step: 2,
    });
    expect(tunableRange(tunable({ kind: "bool", value: 1 }))).toEqual({ min: 0, max: 1, step: 1 });
  });

  it("writes values as a person would", () => {
    expect(shownValue("bool", 1)).toBe("true");
    expect(shownValue("bool", 0)).toBe("false");
    expect(shownValue("int", 3.2)).toBe("3");
    expect(shownValue("float", 0.1 + 0.2)).toBe("0.3");
    expect(shownValue("float", null)).toBe("0");
  });

  it("hands over only what was turned, with what it was", () => {
    const first = [tunable({}), tunable({ name: "double_jump", kind: "bool", value: 0 })];
    const now = [tunable({}), tunable({ name: "double_jump", kind: "bool", value: 1 })];
    const changes = turned(now, first);
    expect(changes).toEqual([{ name: "double_jump", kind: "bool", value: 1, was: 0 }]);
    const tunables: GameTunables = {
      path: "/root/Main",
      name: "Main",
      class: "Node2D",
      script: "res://main.gd",
      values: now,
    };
    expect(keepText(tunables, changes, "Moon Hop")).toBe(
      [
        "Keep these values I tuned while Moon Hop ran. They are set in the running game only: write each where it lives, the script's own start value or the scene that overrides it.",
        "On Main (Node2D) at /root/Main, script res://main.gd:",
        "- double_jump = true (was false)",
      ].join("\n"),
    );
    expect(turned(first, first)).toEqual([]);
  });
});

const pad = (
  pressed: readonly number[] = [],
  axes: readonly number[] = [0, 0, 0, 0],
  values: Readonly<Record<number, number>> = {},
) => ({
  buttons: Array.from({ length: 17 }, (_, index) => ({
    pressed: pressed.includes(index),
    value: values[index] ?? (pressed.includes(index) ? 1 : 0),
  })),
  axes,
});

describe("a controller", () => {
  it("sends its buttons in Godot's numbers, once each way", () => {
    const down = padEvents(PAD_REST, pad([0, 4, 9, 12]));
    expect(down.events).toEqual([
      { t: "pad", button: 0, down: true },
      { t: "pad", button: 9, down: true },
      { t: "pad", button: 6, down: true },
      { t: "pad", button: 11, down: true },
    ]);
    // Held: nothing more is sent.
    expect(padEvents(down.state, pad([0, 4, 9, 12])).events).toEqual([]);
    expect(padEvents(down.state, pad([4, 9, 12])).events).toEqual([
      { t: "pad", button: 0, down: false },
    ]);
  });

  it("sends a stick past its dead zone in twentieths, and a trigger as an axis", () => {
    const moved = padEvents(
      PAD_REST,
      pad([], [0.05, -0.714, 1.4, Number.NaN], { 6: 0.52, 7: 0.02 }),
    );
    expect(moved.events).toEqual([
      { t: "axis", axis: 1, value: -0.7 },
      { t: "axis", axis: 2, value: 1 },
      { t: "axis", axis: 4, value: 0.5 },
    ]);
    // A tremor inside the same twentieth is not sent again.
    expect(padEvents(moved.state, pad([], [0.05, -0.72, 1, 0], { 6: 0.51 })).events).toEqual([]);
    // Back at rest.
    expect(padEvents(moved.state, pad()).events).toEqual([
      { t: "axis", axis: 1, value: 0 },
      { t: "axis", axis: 2, value: 0 },
      { t: "axis", axis: 4, value: 0 },
    ]);
  });

  it("lets go of everything it held", () => {
    const held = padEvents(PAD_REST, pad([1], [0.5, 0, 0, 0])).state;
    expect(padRelease(held)).toEqual([
      { t: "pad", button: 1, down: false },
      { t: "axis", axis: 0, value: 0 },
    ]);
    expect(padRelease(PAD_REST)).toEqual([]);
  });

  it("reads a controller with fewer buttons than the standard one", () => {
    expect(
      padEvents(PAD_REST, { buttons: [{ pressed: true, value: 1 }], axes: [] }).events,
    ).toEqual([{ t: "pad", button: 0, down: true }]);
  });
});
