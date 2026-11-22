import { describe, expect, it } from "vitest";
import type { SetupAction, SetupSnapshot } from "../../bindings";
import { DEFAULT_GENERAL } from "../setup";
import {
  FLIGHT_MS,
  flightPose,
  glide,
  heading,
  type Pose,
  place,
  poseTransform,
  turn,
  WING_RATIO,
  WING_SIZE,
} from "./flight";
import { markTourSeen } from "./seen";
import { NOT_TOURED, offerWords, reachable, seenAfter, TOUR_STOPS, tourOffer } from "./stops";

const ids = TOUR_STOPS.map((stop) => stop.id);
const outside = TOUR_STOPS.filter((stop) => stop.place !== "chat").map((stop) => stop.id);
const inside = TOUR_STOPS.filter((stop) => stop.place === "chat").map((stop) => stop.id);

describe("the tour's stops", () => {
  it("names each stop once, in the form the profile saves and the core accepts", () => {
    expect(new Set(ids).size).toBe(ids.length);
    // The core keeps at most 64 names of at most 40 lowercase letters, digits and dashes.
    expect(ids.length).toBeLessThanOrEqual(64);
    for (const id of ids) expect(id).toMatch(/^[a-z0-9-]{1,40}$/);
  });
  it("says what each stop is in a heading and a sentence or two, with no dash for a pause", () => {
    for (const stop of TOUR_STOPS) {
      expect(stop.title.length, stop.id).toBeGreaterThan(2);
      expect(stop.title.length, stop.id).toBeLessThanOrEqual(32);
      expect(stop.body.length, stop.id).toBeGreaterThan(30);
      expect(stop.body.length, stop.id).toBeLessThanOrEqual(190);
      expect(`${stop.title} ${stop.body}`, stop.id).not.toMatch(/[–—]/);
      expect(stop.body.endsWith("."), stop.id).toBe(true);
    }
  });
  it("gives a reason for every control it leaves out, and leaves out no stop", () => {
    for (const [control, why] of Object.entries(NOT_TOURED)) {
      expect(why.length, control).toBeGreaterThan(10);
      expect(ids, control).not.toContain(control);
    }
  });
});

describe("what the tour offers", () => {
  it("offers the whole tour to a profile that never met it", () => {
    const offer = tourOffer([], true);
    expect(offer?.kind).toBe("full");
    expect(offer?.stops.map((stop) => stop.id)).toEqual(ids);
    expect(offerWords(offer ?? { kind: "full", stops: [] }).title).toBe("Want the tour?");
  });
  it("keeps the stops inside a chat for when there is a chat", () => {
    const offer = tourOffer([], false);
    expect(offer?.kind).toBe("full");
    expect(offer?.stops.map((stop) => stop.id)).toEqual(outside);
    expect(reachable(TOUR_STOPS, false).map((stop) => stop.id)).toEqual(outside);
    // Having seen the rest, a first chat brings the stops inside it within reach.
    expect(tourOffer(outside, false)).toBeNull();
    const later = tourOffer(outside, true);
    expect(later?.kind).toBe("more");
    expect(later?.stops.map((stop) => stop.id)).toEqual(inside);
    expect(offerWords(later ?? { kind: "more", stops: [] }).title).toBe("More to see");
  });
  it("offers only a stop added since, by name, and nothing once all are seen", () => {
    const allButOne = ids.filter((id) => id !== "dashboard");
    const offer = tourOffer(allButOne, true);
    expect(offer?.kind).toBe("more");
    expect(offer?.stops.map((stop) => stop.id)).toEqual(["dashboard"]);
    const words = offerWords(offer ?? { kind: "more", stops: [] });
    expect(words.title).toBe("One more thing to see");
    expect(words.body).toContain("Dashboard");
    expect(words.take).toBe("Show me");
    expect(tourOffer(ids, true)).toBeNull();
    // A name this build does not know (a stop a newer build added) changes nothing.
    expect(tourOffer([...ids, "from-the-future"], true)).toBeNull();
    expect(tourOffer(["from-the-future"], true)?.kind).toBe("full");
  });
  it("records the stops reached when a flight finishes, and every stop after a no", () => {
    expect(seenAfter("finished", ["projects", "search"])).toEqual(["projects", "search"]);
    expect(seenAfter("stopped", ["projects"])).toEqual(ids);
    expect(seenAfter("declined", [])).toEqual(ids);
  });
});

describe("saving what the tour showed", () => {
  const snapshot = (revision: number, seen: string[]): SetupSnapshot => ({
    cold_start: false,
    notice: null,
    preferences: {
      revision,
      step: "project",
      motion: "full",
      appearance: "system",
      theme: "claude",
      text_size: "compact",
      completed: true,
      first_project_id: null,
      tour_seen: seen,
      general: { ...DEFAULT_GENERAL },
    },
  });
  it("adds only the stops not yet saved, and nothing when all are", async () => {
    const applied: SetupAction[] = [];
    let saved = snapshot(4, ["projects"]);
    const api = {
      snapshot: async () => saved,
      apply: async (revision: number, action: SetupAction) => {
        expect(revision).toBe(saved.preferences.revision);
        applied.push(action);
        if (action.type === "tour_seen")
          saved = snapshot(revision + 1, [...saved.preferences.tour_seen, ...action.stops]);
        return saved;
      },
    };
    expect(await markTourSeen(api, ["projects", "search"])).toEqual(["projects", "search"]);
    expect(applied).toEqual([{ type: "tour_seen", stops: ["search"] }]);
    expect(await markTourSeen(api, ["search"])).toEqual(["projects", "search"]);
    expect(applied).toHaveLength(1);
  });
  it("reads again and tries once more when another change got there first", async () => {
    let saved = snapshot(1, []);
    let refusals = 1;
    const api = {
      snapshot: async () => saved,
      apply: async (revision: number, action: SetupAction) => {
        if (refusals > 0) {
          refusals -= 1;
          // Another view changed a setting in the same moment.
          saved = snapshot(saved.preferences.revision + 1, saved.preferences.tour_seen);
          throw new Error("Setup changed in another view. Reload it and try again.");
        }
        expect(revision).toBe(saved.preferences.revision);
        if (action.type === "tour_seen") saved = snapshot(revision + 1, action.stops);
        return saved;
      },
    };
    expect(await markTourSeen(api, ["projects"])).toEqual(["projects"]);
    refusals = 2;
    await expect(markTourSeen(api, ["search"])).rejects.toThrow("another view");
  });
});

describe("the wing's flight", () => {
  const window = { width: 1440, height: 900 };
  const card = { width: 320, height: 160 };
  const depth = WING_SIZE * WING_RATIO;
  it("settles just off the control with its nose toward it, the card beyond", () => {
    const row = { left: 8, top: 208, width: 240, height: 40 };
    const right = place(row, "right", card, window);
    expect(right.side).toBe("right");
    expect(right.wing.angle).toBe(-90);
    expect(right.wing.x).toBeCloseTo(248 + 8 + depth / 2);
    expect(right.wing.y).toBe(228);
    expect(right.card.x).toBeCloseTo(248 + 16 + depth);
    const button = { left: 1300, top: 14, width: 28, height: 28 };
    const below = place(button, "below", card, window);
    expect(below.wing.angle).toBe(0);
    expect(below.wing.x).toBe(1314);
    expect(below.wing.y).toBeCloseTo(42 + 8 + depth / 2);
    // The card stays inside the window, whatever the control's place.
    expect(below.card.x + card.width).toBeLessThanOrEqual(window.width - 12);
    expect(place(row, "above", card, window).wing.angle).toBe(180);
  });
  it("takes the other side when the card would not fit on the side asked for", () => {
    const atRightEdge = { left: 1380, top: 400, width: 40, height: 40 };
    const placed = place(atRightEdge, "right", card, window);
    expect(placed.side).toBe("left");
    expect(placed.wing.angle).toBe(90);
    expect(placed.card.x + card.width).toBeLessThan(atRightEdge.left);
    const atFoot = { left: 400, top: 860, width: 300, height: 30 };
    expect(place(atFoot, "below", card, window).side).toBe("above");
    // With no room either way it keeps the side it was given.
    const tall = { left: 100, top: 20, width: 100, height: 860 };
    expect(place(tall, "below", card, window).side).toBe("below");
  });
  it("keeps the card on screen beside a control at the window's foot", () => {
    const settings = { left: 216, top: 856, width: 28, height: 28 };
    const placed = place(settings, "right", card, window);
    expect(placed.card.y + card.height).toBeLessThanOrEqual(window.height - 12);
    expect(placed.card.y).toBeGreaterThanOrEqual(12);
  });
  it("starts and ends each flight exactly where it was told to", () => {
    const from: Pose = { x: 34, y: 870, angle: 0, bank: 0, size: 30 };
    const to: Pose = { x: 900, y: 120, angle: -90, bank: 0, size: WING_SIZE };
    expect(flightPose(from, to, 0)).toEqual(from);
    const end = flightPose(from, to, 1);
    expect(end.x).toBeCloseTo(to.x);
    expect(end.y).toBeCloseTo(to.y);
    expect(end.size).toBe(to.size);
    expect(Math.abs(end.bank)).toBeLessThan(1e-9);
    // Its heading ends on the resting one, whichever way round it turned.
    expect((((end.angle - to.angle) % 360) + 360) % 360).toBeCloseTo(0);
  });
  it("flies nose first along an arc and rolls through the middle", () => {
    const from: Pose = { x: 100, y: 800, angle: 0, bank: 0, size: 30 };
    const to: Pose = { x: 900, y: 800, angle: 0, bank: 0, size: 30 };
    const middle = flightPose(from, to, 0.5);
    // Travelling right, the arc bows upward and the nose points right.
    expect(middle.y).toBeLessThan(800);
    expect(middle.angle).toBeCloseTo(90, 0);
    expect(middle.bank).toBeGreaterThan(0.2);
    expect(flightPose(from, to, 0.1).bank).toBeLessThan(middle.bank);
    // Out of range is held at the ends.
    expect(flightPose(from, to, -1)).toEqual(from);
    expect(flightPose(from, to, 2).x).toBeCloseTo(900);
  });
  it("slides straight between two places a few pixels apart", () => {
    const from: Pose = { x: 100, y: 100, angle: -90, bank: 0, size: 44 };
    const to: Pose = { x: 100, y: 112, angle: -90, bank: 0, size: 44 };
    const middle = flightPose(from, to, 0.5);
    expect(middle).toEqual({ x: 100, y: 106, angle: -90, bank: 0, size: 44 });
  });
  it("turns the short way round and reads a screen's directions", () => {
    expect(turn(170, -170, 0.5)).toBeCloseTo(180);
    expect(turn(0, 90, 1)).toBe(90);
    expect(turn(-90, 180, 1)).toBeCloseTo(-180);
    expect(heading(0, -1)).toBeCloseTo(0);
    expect(heading(1, 0)).toBeCloseTo(90);
    expect(heading(0, 1)).toBeCloseTo(180);
    expect(heading(-1, 0)).toBeCloseTo(-90);
  });
  it("glides from rest to rest, quickly away and slowly in", () => {
    expect(glide(0)).toBe(0);
    expect(glide(1)).toBe(1);
    expect(glide(-3)).toBe(0);
    expect(glide(3)).toBe(1);
    let last = 0;
    for (let step = 1; step <= 20; step += 1) {
      const now = glide(step / 20);
      expect(now).toBeGreaterThanOrEqual(last);
      last = now;
    }
    expect(glide(0.3)).toBeGreaterThan(0.6);
    expect(FLIGHT_MS).toBeGreaterThan(300);
  });
  it("draws a pose about the wing's own centre", () => {
    expect(poseTransform({ x: 100, y: 50, angle: -90, bank: 0.2, size: 44 })).toBe(
      `translate3d(78.00px, ${(50 - (44 * WING_RATIO) / 2).toFixed(2)}px, 0) rotate(-90.00deg) scaleX(0.800)`,
    );
  });
});
