import { describe, expect, it } from "vitest";
import { APPROVAL } from "../../fixture/approval";
import { GOLDEN } from "../../fixture/golden";
import type { ChatRecording } from "../../fixture/types";
import { eventsAt, reduceTranscript } from "../../model/transcript";
import { type HeldStatus, holdStatus, type LiveStatus, liveStatus, VERB_HOLD_MS } from "./status";

/** The live status of a recording's last turn at `seconds` on its timeline. */
function statusAt(recording: ChatRecording, seconds: number): LiveStatus {
  const now = seconds * 1000;
  const model = reduceTranscript(recording.events.slice(0, eventsAt(recording.events, now)));
  const turn = model.turns[model.turns.length - 1];
  if (!turn) throw new Error("no turn yet");
  return liveStatus(turn, model.activity, model.plan, now);
}

describe("the status line over the golden turn", () => {
  it("names what runs and what is read", () => {
    expect(statusAt(GOLDEN, 5)).toMatchObject({ verb: "Thinking", target: null });
    expect(statusAt(GOLDEN, 8.5)).toMatchObject({ verb: "Reading", target: "pacing.ts" });
    expect(statusAt(GOLDEN, 9.5)).toMatchObject({ verb: "Searching", mono: true });
    expect(statusAt(GOLDEN, 20)).toMatchObject({ verb: "Editing", target: "pacing.ts" });
    expect(statusAt(GOLDEN, 24)).toMatchObject({
      verb: "Running",
      target: "pnpm --filter @ghost/ui test pacing",
      mono: true,
    });
    expect(statusAt(GOLDEN, 35)).toMatchObject({ verb: "Writing" });
  });

  it("keeps the tool's verb through the short thoughts between calls", () => {
    // 8.75 to 8.9 s: the first read is done and the next one has not started.
    expect(statusAt(GOLDEN, 8.8)).toMatchObject({ verb: "Reading", target: "pacing.ts" });
    // 9.9 s the search ends; a second and a half later the agent is plainly thinking.
    expect(statusAt(GOLDEN, 10.5)).toMatchObject({ verb: "Searching" });
    expect(statusAt(GOLDEN, 11.5)).toMatchObject({ verb: "Thinking" });
  });

  it("says when the agent is updating its plan", () => {
    expect(statusAt(GOLDEN, 19.3)).toMatchObject({ verb: "Planning" });
  });

  it("names the command a permission holds", () => {
    const end = APPROVAL.duration / 1000;
    expect(statusAt(APPROVAL, end)).toMatchObject({
      verb: "Needs your approval",
      target: "cargo test -p aura-drivers",
      tone: "wait",
    });
  });
});

describe("holding a verb on the line", () => {
  const reading: LiveStatus = { verb: "Reading", target: "a.ts", mono: false, tone: "work" };
  const searching: LiveStatus = { verb: "Searching", target: "now", mono: true, tone: "work" };
  const waiting: LiveStatus = {
    verb: "Needs your approval",
    target: null,
    mono: false,
    tone: "wait",
  };

  it("keeps a verb up for a second before the next one shows", () => {
    const first = holdStatus(null, reading, 1000);
    expect(holdStatus(first, searching, 1000 + VERB_HOLD_MS - 1)).toBe(first);
    expect(holdStatus(first, searching, 1000 + VERB_HOLD_MS)).toEqual({
      status: searching,
      since: 1000 + VERB_HOLD_MS,
    });
  });

  it("updates the target under the same verb at once, without restarting the clock", () => {
    const first = holdStatus(null, reading, 1000);
    const next = holdStatus(first, { ...reading, target: "b.ts" }, 1200);
    expect(next).toEqual({ status: { ...reading, target: "b.ts" }, since: 1000 });
  });

  it("holds nothing while the playhead is still, so an answer you give shows at once", () => {
    const first = holdStatus(null, waiting, 9000);
    expect(holdStatus(first, reading, 9000, 0)).toEqual({ status: reading, since: 9000 });
  });

  it("shows waiting on you at once, and starts over after a replay", () => {
    const first: HeldStatus = holdStatus(null, reading, 5000);
    expect(holdStatus(first, waiting, 5100).status).toBe(waiting);
    expect(holdStatus(first, searching, 100)).toEqual({ status: searching, since: 100 });
  });
});
