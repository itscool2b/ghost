import { AGENT_BRAND_IDS } from "@ghost/ui";
import { describe, expect, it } from "vitest";
import { APPROVAL } from "../../fixture/approval";
import { GOLDEN } from "../../fixture/golden";
import { eventsAt, reduceTranscript } from "../../model/transcript";
import {
  belowPoint,
  bodyRadius,
  eccentricAnomaly,
  IDLE_TIMELINE,
  isFarSide,
  markFrame,
  markTimeline,
  orbitArc,
  orbitPoint,
  PARKED_ANGLE,
  SIGNATURES,
  satelliteSide,
  signatureOf,
  stateAt,
  topAngle,
  travelAngle,
  travelPhase,
  waitingPoint,
} from "./geometry";

const TAU = Math.PI * 2;

function goldenAt(seconds: number) {
  return reduceTranscript(GOLDEN.events.slice(0, eventsAt(GOLDEN.events, seconds * 1000)));
}

describe("signatures", () => {
  it("gives every brand an orbit, and an unknown agent the dashed one", () => {
    for (const id of AGENT_BRAND_IDS) expect(SIGNATURES[id]).toBeDefined();
    expect(signatureOf("someone-new").unknown).toBe(true);
    expect(signatureOf("constructor").unknown).toBe(true);
    expect(signatureOf("claude-code")).toMatchObject({ tilt: 28.5, k: 0.34, period: 2400 });
  });

  it("keeps every designed silhouette distinct", () => {
    const shapes = AGENT_BRAND_IDS.filter((id) => id !== "generic").map((id) => {
      const s = SIGNATURES[id];
      return `${s.kind}:${s.tilt}:${s.a}:${s.k}:${s.shift}:${s.satellites}:${s.direction}`;
    });
    expect(new Set(shapes).size).toBe(shapes.length);
  });

  it("sizes satellites by the canvas, never under 4 or over 10 px", () => {
    expect([14, 16, 20, 24, 56, 220].map(satelliteSide)).toEqual([4, 4, 4, 4, 10, 10]);
    expect(bodyRadius(100)).toBeCloseTo(9, 9);
    // Below 48px the body is a dot, so the orbit reads as a line around a node, not a capsule.
    expect(bodyRadius(16)).toBeCloseTo(0.96, 9);
    // Every size keeps the node inside the narrowest orbit's minor axis.
    for (const size of [14, 16, 20, 24, 56, 100]) {
      expect(bodyRadius(size)).toBeLessThan(0.42 * size * 0.3);
    }
  });
});

describe("orbit geometry", () => {
  it("parks the satellite 40 degrees up the front of the orbit, outside the body", () => {
    for (const id of AGENT_BRAND_IDS) {
      const signature = SIGNATURES[id];
      const point = orbitPoint(signature, PARKED_ANGLE, 100);
      expect(Math.hypot(point.x - 50, point.y - 50)).toBeGreaterThan(bodyRadius(100));
    }
    // Claude Code: (0.42 cos 40, 0.1428 sin 40) rotated 28.5 degrees, y flipped.
    const claude = orbitPoint(SIGNATURES["claude-code"], PARKED_ANGLE, 100);
    const ex = 42 * Math.cos(PARKED_ANGLE);
    const ey = 42 * 0.34 * Math.sin(PARKED_ANGLE);
    const tilt = (28.5 * Math.PI) / 180;
    expect(claude.x).toBeCloseTo(50 + ex * Math.cos(tilt) - ey * Math.sin(tilt), 6);
    expect(claude.y).toBeCloseTo(50 - (ex * Math.sin(tilt) + ey * Math.cos(tilt)), 6);
  });

  it("finds the highest point of each orbit for a waiting satellite", () => {
    for (const id of AGENT_BRAND_IDS) {
      const signature = SIGNATURES[id];
      const top = orbitPoint(signature, topAngle(signature), 100).y;
      for (let index = 0; index < 360; index += 3) {
        const y = orbitPoint(signature, (index * Math.PI) / 180, 100).y;
        expect(top).toBeLessThanOrEqual(y + 1e-9);
      }
    }
  });

  it("draws a ring as a circle and an arc as a path through the orbit", () => {
    const ring = SIGNATURES.cursor;
    for (const angle of [0, 1, 2, 4]) {
      const point = orbitPoint(ring, angle, 100);
      expect(Math.hypot(point.x - 50, point.y - 50)).toBeCloseTo(42, 6);
    }
    expect(isFarSide(ring, 1)).toBe(false);
    expect(isFarSide(SIGNATURES.codex, 1)).toBe(true);
    expect(isFarSide(SIGNATURES.codex, -1)).toBe(false);
    const path = orbitArc(SIGNATURES.codex, 0, Math.PI, 24);
    expect(path.startsWith("M")).toBe(true);
    expect(path.split("L").length).toBeGreaterThan(8);
  });
});

describe("satellite travel", () => {
  it("starts and ends every revolution at the parked point", () => {
    for (const id of AGENT_BRAND_IDS) {
      const signature = SIGNATURES[id];
      expect(travelAngle(signature, 0)).toBeCloseTo(PARKED_ANGLE, 9);
      expect(travelAngle(signature, 1)).toBeCloseTo(PARKED_ANGLE + signature.direction * TAU, 9);
      expect(travelAngle(signature, 3)).toBeCloseTo(
        PARKED_ANGLE + signature.direction * 3 * TAU,
        9,
      );
    }
  });

  it("moves counterclockwise, except OpenCode", () => {
    expect(travelAngle(SIGNATURES.pi, 0.25)).toBeGreaterThan(PARKED_ANGLE);
    expect(travelAngle(SIGNATURES.opencode, 0.25)).toBeLessThan(PARKED_ANGLE);
  });

  it("steps Codex through 12 positions like a terminal cursor", () => {
    const positions = new Set<number>();
    for (let index = 0; index < 120; index += 1) {
      positions.add(Number(travelAngle(SIGNATURES.codex, index / 120).toFixed(6)));
    }
    expect(positions.size).toBe(12);
  });

  it("solves Kepler's equation, and swings Grok Build fast near the body and slow far out", () => {
    for (const mean of [0.1, 1, 2.5, 4, 6]) {
      const e = eccentricAnomaly(mean, 0.6);
      expect(e - 0.6 * Math.sin(e)).toBeCloseTo(mean, 4);
    }
    const grok = SIGNATURES.grok;
    const speedNear = (target: number): number => {
      // Angular speed where the satellite passes `target` (radians, in the orbit's frame).
      let best = { gap: Number.POSITIVE_INFINITY, speed: 0 };
      for (let index = 0; index < 2000; index += 1) {
        const phase = index / 2000;
        const angle = travelAngle(grok, phase);
        const gap = Math.abs(Math.atan2(Math.sin(angle - target), Math.cos(angle - target)));
        if (gap < best.gap) {
          best = { gap, speed: travelAngle(grok, phase + 0.0005) - angle };
        }
      }
      return best.speed;
    };
    // The orbit is shifted toward +x, so the body sits near angle pi: periapsis.
    expect(speedNear(Math.PI)).toBeGreaterThan(speedNear(0) * 3);
  });

  it("speeds Antigravity across the rising half", () => {
    const sig = SIGNATURES.antigravity;
    const rate = (phase: number) => travelAngle(sig, phase + 0.001) - travelAngle(sig, phase);
    // Around phase 0.9 the satellite rises past the rightmost point; around 0.4 it falls.
    expect(rate(0.9)).toBeGreaterThan(rate(0.4));
  });
});

describe("activity over time", () => {
  it("reads the golden turn: thinking, exploring, streaming, testing, done", () => {
    expect(stateAt(markTimeline(goldenAt(3)), 3000).state).toBe("thinking");
    expect(stateAt(markTimeline(goldenAt(9)), 9000).state).toBe("tool");
    expect(stateAt(markTimeline(goldenAt(14)), 14000).state).toBe("streaming");
    expect(stateAt(markTimeline(goldenAt(24)), 24000).state).toBe("tool");
    expect(stateAt(markTimeline(goldenAt(28)), 28500).state).toBe("thinking");
    const end = markTimeline(goldenAt(60));
    expect(stateAt(end, 48300)).toEqual({ state: "done", since: 48300 });
    expect(stateAt(IDLE_TIMELINE, 5).state).toBe("idle");
  });

  it("waits on the open permission request, with one orange ripple", () => {
    const model = reduceTranscript(APPROVAL.events);
    const timeline = markTimeline(model);
    expect(stateAt(timeline, APPROVAL.duration).state).toBe("waiting");
    expect(timeline.ripples.filter((ripple) => ripple.tone === "signal")).toHaveLength(1);
  });

  it("travels only while thinking or streaming, and never backwards", () => {
    const timeline = markTimeline(goldenAt(60));
    const signature = SIGNATURES["claude-code"];
    let previous = 0;
    for (let ms = 0; ms <= 50_000; ms += 250) {
      const phase = travelPhase(timeline, signature, ms);
      expect(phase).toBeGreaterThanOrEqual(previous - 1e-12);
      previous = phase;
    }
    // Stopped while the tests run (23 s to 26.5 s).
    expect(travelPhase(timeline, signature, 26_000)).toBeCloseTo(
      travelPhase(timeline, signature, 23_500),
      9,
    );
    // Thinking runs at two thirds of streaming's pace.
    const thinking =
      travelPhase(timeline, signature, 3000) - travelPhase(timeline, signature, 2000);
    const streaming =
      travelPhase(timeline, signature, 14_000) - travelPhase(timeline, signature, 13_000);
    expect(thinking / streaming).toBeCloseTo(1 / 1.5, 6);
  });
});

describe("the frame", () => {
  it("draws a moving satellite with a trail while streaming, and parks it at rest", () => {
    const signature = SIGNATURES["claude-code"];
    const streaming = markFrame(signature, markTimeline(goldenAt(14)), 14_000, 20);
    expect(streaming.state).toBe("streaming");
    expect(streaming.trail).toBe(true);
    expect(streaming.orbitTone).toBe("text-1");
    expect(markFrame(signature, IDLE_TIMELINE, 0, 20).orbitTone).toBe("text-2");
    const idle = markFrame(signature, IDLE_TIMELINE, 0, 20);
    const parked = orbitPoint(signature, PARKED_ANGLE, 20);
    expect([idle.x, idle.y]).toEqual([parked.x, parked.y]);
    expect(idle.trail).toBe(false);
  });

  it("turns a waiting satellite into an orange diamond at the top of the mark", () => {
    const signature = SIGNATURES.codex;
    const timeline = markTimeline(reduceTranscript(APPROVAL.events));
    const frame = markFrame(signature, timeline, APPROVAL.duration + 400, 24);
    // Codex's flat orbit tops out inside the body, so the diamond rises just above it.
    const rest = waitingPoint(signature, 24);
    expect(rest.onOrbit).toBe(false);
    expect(frame).toMatchObject({
      diamond: true,
      hidden: false,
      satelliteTone: "signal",
      // Only the satellite spends the orange; the orbit stays a quiet line.
      orbitTone: "text-3",
    });
    expect(frame.x).toBeCloseTo(rest.x, 6);
    expect(frame.y).toBeCloseTo(rest.y, 6);
    // Claude Code's inclined orbit tops out clear of the body at 24px: the diamond rides it.
    const claude = waitingPoint(SIGNATURES["claude-code"], 24);
    const apex = orbitPoint(SIGNATURES["claude-code"], topAngle(SIGNATURES["claude-code"]), 24);
    expect(claude).toMatchObject({ onOrbit: true, x: apex.x, y: apex.y });
    // One orange ripple on entry.
    expect(markFrame(signature, timeline, APPROVAL.duration + 100, 24).ripple?.tone).toBe("signal");
    expect(markFrame(signature, timeline, APPROVAL.duration + 700, 24).ripple).toBeNull();
  });

  it("finishes the revolution when done, draws the check, then lets it go", () => {
    const signature = SIGNATURES["claude-code"];
    const timeline = markTimeline(goldenAt(60));
    const settledLater = markFrame(signature, timeline, 48_300 + 600, 16);
    const parked = orbitPoint(signature, PARKED_ANGLE, 16);
    expect(settledLater.x).toBeCloseTo(parked.x, 6);
    expect(settledLater.y).toBeCloseTo(parked.y, 6);
    expect(markFrame(signature, timeline, 48_300 + 300, 16)).toMatchObject({
      checkDraw: 1,
      checkOpacity: 1,
    });
    expect(markFrame(signature, timeline, 48_300 + 2500, 16).checkOpacity).toBe(0);
  });

  it("drops a failed satellite to the bottom of the body, in red, on a dashed orbit", () => {
    const timeline = {
      segments: [{ from: 0, to: null, state: "error" as const }],
      ripples: [],
      streamedChars: 0,
    };
    const frame = markFrame(SIGNATURES.pi, timeline, 1000, 40);
    expect(frame).toMatchObject({ satelliteTone: "red", orbitDashed: true });
    expect(frame.x).toBeCloseTo(belowPoint(40).x, 6);
    expect(frame.y).toBeCloseTo(belowPoint(40).y, 6);
    expect(frame.y).toBeGreaterThan(20 + bodyRadius(40));
  });

  it("under reduced motion, parks active satellites and darkens the orbit instead", () => {
    const signature = SIGNATURES["claude-code"];
    const frame = markFrame(signature, markTimeline(goldenAt(14)), 14_000, 20, true);
    const parked = orbitPoint(signature, PARKED_ANGLE, 20);
    expect(frame).toMatchObject({ orbitTone: "text-1", trail: false, ripple: null });
    expect([frame.x, frame.y]).toEqual([parked.x, parked.y]);
    const waiting = markFrame(
      SIGNATURES.codex,
      markTimeline(reduceTranscript(APPROVAL.events)),
      APPROVAL.duration,
      24,
      true,
    );
    expect(waiting).toMatchObject({ diamond: true, satelliteTone: "signal" });
  });

  it("never hides the needs-you diamond, for any agent at any mark size", () => {
    const still = {
      segments: [{ from: -1e9, to: null, state: "waiting" as const }],
      ripples: [],
      streamedChars: 0,
    };
    for (const signature of Object.values(SIGNATURES)) {
      for (const size of [14, 16, 20, 24, 56]) {
        for (const reduced of [false, true]) {
          const frame = markFrame(signature, still, 0, size, reduced);
          expect(frame.diamond).toBe(true);
          expect(frame.hidden).toBe(false);
          // Its centre sits clear of the body, so the node never covers it.
          const reach = Math.hypot(frame.x - size / 2, frame.y - size / 2);
          expect(reach).toBeGreaterThanOrEqual(bodyRadius(size) + satelliteSide(size) / 2 - 1e-9);
        }
      }
    }
  });

  it("is deterministic: the same inputs always draw the same frame", () => {
    const timeline = markTimeline(goldenAt(24));
    const signature = SIGNATURES["claude-code"];
    expect(markFrame(signature, timeline, 24_000, 20)).toEqual(
      markFrame(signature, timeline, 24_000, 20),
    );
  });
});
