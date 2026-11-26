import { describe, expect, it } from "vitest";
import type { EventBatch, GhostEvent, ToolContent, ToolUpdate } from "../bindings";
import {
  activityOf,
  applyBatch,
  applyBatches,
  initialStreamState,
  lastMessage,
  MAX_NOTICES,
  type StreamState,
  seqDistance,
} from "./stream";

const SESSION = "019a0000-0000-7000-8000-000000000001";

function frame(seqStart: number, events: GhostEvent[], sessionId = SESSION): EventBatch {
  return { chat_id: sessionId, seq_start: seqStart, events };
}

const delta = (text: string, id = "m1"): GhostEvent => ({
  type: "message_delta",
  message_id: id,
  text,
});
const thought = (text: string, id = "t1"): GhostEvent => ({
  type: "thought_delta",
  thought_id: id,
  text,
});

/** A tool update that changes only the fields in `fields`. */
const update = (id: string, fields: Partial<Omit<ToolUpdate, "tool_call_id">>): GhostEvent => ({
  type: "tool_update",
  tool_call_id: id,
  status: null,
  title: null,
  kind: null,
  locations: null,
  raw_input: null,
  tool_name: null,
  content: null,
  truncated: null,
  ...fields,
});

const permission = (id: string, title: string): GhostEvent => ({
  type: "permission_requested",
  request_id: id,
  tool_call_id: null,
  title,
  kind: null,
  raw_input: null,
  content: [],
  locations: [],
  options: [],
  truncated: null,
});

/** Frames numbered back to back from `start`, like the batcher produces. */
function numbered(groups: GhostEvent[][], start = 0): EventBatch[] {
  let seq = start;
  return groups.map((events) => {
    const batch = frame(seq, events);
    seq += events.length;
    return batch;
  });
}

function textOf(state: StreamState, id = "m1"): string | undefined {
  return state.messages.get(id)?.text;
}

describe("applyBatch", () => {
  it("appends deltas per id and keeps blocks in first-seen order", () => {
    const state = applyBatches(
      initialStreamState(),
      numbered([
        [{ type: "turn_started", turn_id: "turn-1", prompt_id: null }, thought("Let me ")],
        [thought("look."), { type: "thought_completed", thought_id: "t1", duration_ms: 1200 }],
        [
          {
            type: "tool_call",
            tool_call_id: "read",
            kind: "read",
            title: "Read src/main.rs",
            status: "in_progress",
            locations: [{ path: "src/main.rs", line: null }],
            raw_input: null,
            tool_name: null,
            content: [],
            truncated: null,
          },
        ],
        [delta("Hello, "), delta("world")],
        [delta("!"), { type: "message_completed", message_id: "m1" }],
      ]),
    );
    expect(state.items).toEqual([
      { kind: "thought", id: "t1" },
      { kind: "tool", id: "read" },
      { kind: "message", id: "m1" },
    ]);
    expect(state.thoughts.get("t1")).toEqual({
      id: "t1",
      text: "Let me look.",
      completed: true,
      durationMs: 1200,
    });
    expect(state.messages.get("m1")).toEqual({ id: "m1", text: "Hello, world!", completed: true });
    expect(state.events).toBe(9);
    expect(state.frames).toBe(5);
    expect(state.nextSeq).toBe(9);
    expect(state.gaps).toBe(0);
    expect(state.duplicates).toBe(0);
  });

  it("takes coalesced and split deltas exactly as sent", () => {
    // The batcher merges a run of tokens into one delta, and splits a huge one across frames.
    const big = "x".repeat(10_000);
    const state = applyBatches(
      initialStreamState(),
      numbered([[delta("one two three ")], [delta(big.slice(0, 4000))], [delta(big.slice(4000))]]),
    );
    expect(textOf(state)).toBe(`one two three ${big}`);
    expect(state.items).toHaveLength(1);
  });

  it("drops a frame it has already applied", () => {
    const [first, second] = numbered([[delta("a"), delta("b")], [delta("c")]]);
    if (!first || !second) throw new Error("frames");
    const once = applyBatches(initialStreamState(), [first, second]);
    const again = applyBatches(once, [first, second]);
    expect(textOf(again)).toBe("abc");
    expect(again.duplicates).toBe(3);
    expect(again.events).toBe(3);
    expect(again.frames).toBe(2);
    expect(again.nextSeq).toBe(3);
  });

  it("applies only the new tail of an overlapping frame", () => {
    const state = applyBatches(initialStreamState(), [
      frame(0, [delta("a"), delta("b")]),
      // Replays seq 1 ("b") and adds seq 2 and 3.
      frame(1, [delta("b"), delta("c"), delta("d")]),
    ]);
    expect(textOf(state)).toBe("abcd");
    expect(state.duplicates).toBe(1);
    expect(state.events).toBe(4);
    expect(state.nextSeq).toBe(4);
    expect(state.gaps).toBe(0);
  });

  it("counts gaps and the events they skipped, and keeps going", () => {
    const state = applyBatches(initialStreamState(), [
      frame(0, [delta("a")]),
      frame(3, [delta("d")]),
      frame(4, [delta("e")]),
      frame(10, [delta("k")]),
    ]);
    expect(state.gaps).toBe(2);
    expect(state.missed).toBe(2 + 5);
    expect(textOf(state)).toBe("adek");
    expect(state.nextSeq).toBe(11);
  });

  it("treats a first frame that does not start at 0 as a gap", () => {
    const state = applyBatch(initialStreamState(), frame(5, [delta("late")]));
    expect(state.gaps).toBe(1);
    expect(state.missed).toBe(5);
    expect(state.sessionId).toBe(SESSION);
  });

  it("ignores frames from another session", () => {
    const own = applyBatch(initialStreamState(), frame(0, [delta("mine")]));
    const after = applyBatch(own, frame(1, [delta("theirs")], "another-session"));
    expect(textOf(after)).toBe("mine");
    expect(after.foreign).toBe(1);
    expect(after.events).toBe(1);
    expect(after.nextSeq).toBe(1);
  });

  it("follows sequence numbers across the u32 wrap", () => {
    // A resumed session (the batcher's `starting_at`) can sit just below the wrap.
    const start = 2 ** 32 - 2;
    const frames = numbered([[delta("a")], [delta("b"), delta("c")], [delta("d")]], start).map(
      (batch) => ({ ...batch, seq_start: batch.seq_start % 2 ** 32 }),
    );
    expect(frames.map((f) => f.seq_start)).toEqual([2 ** 32 - 2, 2 ** 32 - 1, 1]);
    const state = applyBatches({ ...initialStreamState(), nextSeq: start }, frames);
    expect(state.gaps).toBe(0);
    expect(state.duplicates).toBe(0);
    expect(textOf(state)).toBe("abcd");
    expect(state.nextSeq).toBe(2);
    // A replay of the pre-wrap frame is recognised as old, not as a 4-billion-event gap.
    const replayed = applyBatch(state, frames[0] ?? frame(0, []));
    expect(replayed.duplicates).toBe(1);
    expect(replayed.gaps).toBe(0);
    expect(seqDistance(1, 2 ** 32 - 1)).toBe(2);
    expect(seqDistance(2 ** 32 - 1, 1)).toBe(-2);
  });

  it("ignores empty frames entirely", () => {
    const state = initialStreamState();
    expect(applyBatch(state, frame(0, []))).toBe(state);
  });

  it("leaves the previous state untouched", () => {
    const before = applyBatch(initialStreamState(), frame(0, [delta("a")]));
    const snapshot = { text: textOf(before), items: before.items.length };
    applyBatch(before, frame(1, [delta("b"), delta("x", "m2")]));
    expect({ text: textOf(before), items: before.items.length }).toEqual(snapshot);
  });

  it("records tool updates: null content keeps it, a list replaces it, [] clears it", () => {
    const diff: ToolContent = {
      type: "diff",
      path: "src/main.rs",
      old_text: "a",
      new_text: "b",
      truncated: null,
    };
    const updated = applyBatches(
      initialStreamState(),
      numbered([
        [
          {
            type: "tool_call",
            tool_call_id: "edit",
            kind: "edit",
            title: "Edit src/main.rs",
            status: "in_progress",
            locations: [],
            raw_input: null,
            tool_name: null,
            content: [],
            truncated: null,
          },
        ],
        [update("edit", { content: [diff] })],
        [update("edit", { status: "completed", title: "Edited src/main.rs" })],
      ]),
    );
    const tool = updated.tools.get("edit");
    expect(tool?.status).toBe("completed");
    expect(tool?.title).toBe("Edited src/main.rs");
    expect(tool?.kind).toBe("edit");
    expect(tool?.content).toEqual([diff]);

    const cleared = applyBatch(updated, frame(updated.nextSeq, [update("edit", { content: [] })]));
    expect(cleared.tools.get("edit")?.content).toEqual([]);
  });

  it("shows an update for a tool it never saw announced", () => {
    const state = applyBatch(
      initialStreamState(),
      frame(0, [
        update("ghost", {
          status: "failed",
          content: [{ type: "text", text: "boom", truncated: null }],
        }),
      ]),
    );
    expect(state.items).toEqual([{ kind: "tool", id: "ghost" }]);
    expect(state.tools.get("ghost")).toMatchObject({
      kind: "other",
      title: "Tool",
      status: "failed",
    });
  });

  it("keeps the latest plan, usage, session and pending permissions", () => {
    const state = applyBatches(
      initialStreamState(),
      numbered([
        [
          {
            type: "plan_updated",
            entries: [{ content: "Old", status: "pending", priority: null }],
          },
          {
            type: "plan_updated",
            entries: [{ content: "Read", status: "completed", priority: "high" }],
          },
          {
            type: "usage_updated",
            context_used: 100,
            context_size: 200_000,
            cost: { amount: 0.01, currency: "USD" },
          },
          {
            type: "session_updated",
            status: "running",
            detail: null,
            native_session_id: null,
            agent_name: null,
            agent_version: null,
            title: "Demo",
            model: "demo",
            effort: null,
            mode: null,
            autonomy: "full_auto",
            queue_held: false,
            revision: 0,
            queue_wait: null,
          },
          permission("p1", "Run rm?"),
          permission("p2", "Run ls?"),
          {
            type: "permission_resolved",
            request_id: "p1",
            decision: { type: "cancelled" },
            by: "user",
          },
        ],
      ]),
    );
    expect(state.plan).toEqual([{ content: "Read", status: "completed", priority: "high" }]);
    expect(state.usage).toEqual({
      context_used: 100,
      context_size: 200_000,
      cost: { amount: 0.01, currency: "USD" },
    });
    expect(state.session?.title).toBe("Demo");
    expect(state.permissions.map((p) => p.request_id)).toEqual(["p2"]);
  });

  it("caps notices and records errors as notices", () => {
    const notices: GhostEvent[] = Array.from({ length: MAX_NOTICES + 5 }, (_, i) => ({
      type: "notice",
      level: "info",
      message: `n${i}`,
    }));
    notices.push({
      type: "error",
      kind: "agent_error",
      message: "boom",
      detail: null,
      recoverable: false,
    });
    const state = applyBatch(initialStreamState(), frame(0, notices));
    expect(state.notices).toHaveLength(MAX_NOTICES);
    expect(state.notices.at(-1)).toEqual({ level: "error", message: "boom" });
  });

  it("tracks the turn from start to its stop reason", () => {
    const started = applyBatch(
      initialStreamState(),
      frame(0, [{ type: "turn_started", turn_id: "turn-1", prompt_id: null }]),
    );
    expect(started.turn).toEqual({ id: "turn-1", ended: false, stopReason: null, usage: null });
    const ended = applyBatch(
      started,
      frame(1, [
        {
          type: "turn_ended",
          turn_id: "turn-1",
          stop_reason: "cancelled",
          usage: null,
          duration_ms: null,
          recovered: false,
        },
      ]),
    );
    expect(ended.turn?.ended).toBe(true);
    expect(ended.turn?.stopReason).toBe("cancelled");
  });
});

describe("activityOf and lastMessage", () => {
  it("follows the turn through its blocks", () => {
    let state = initialStreamState();
    expect(activityOf(state)).toBe("idle");
    state = applyBatch(state, frame(0, [{ type: "turn_started", turn_id: "t", prompt_id: null }]));
    expect(activityOf(state)).toBe("thinking");
    state = applyBatch(
      state,
      frame(1, [
        {
          type: "tool_call",
          tool_call_id: "x",
          kind: "execute",
          title: "Ran tests",
          status: "in_progress",
          locations: [],
          raw_input: null,
          tool_name: null,
          content: [],
          truncated: null,
        },
      ]),
    );
    expect(activityOf(state)).toBe("tool");
    state = applyBatch(state, frame(2, [delta("Done")]));
    expect(activityOf(state)).toBe("streaming");
    expect(lastMessage(state)?.text).toBe("Done");
    state = applyBatch(
      state,
      frame(3, [
        {
          type: "turn_ended",
          turn_id: "t",
          stop_reason: "end_turn",
          usage: null,
          duration_ms: null,
          recovered: false,
        },
      ]),
    );
    expect(activityOf(state)).toBe("done");
  });

  it("reports an errored turn", () => {
    const state = applyBatch(
      initialStreamState(),
      frame(0, [
        { type: "turn_started", turn_id: "t", prompt_id: null },
        {
          type: "turn_ended",
          turn_id: "t",
          stop_reason: "error",
          usage: null,
          duration_ms: null,
          recovered: false,
        },
      ]),
    );
    expect(activityOf(state)).toBe("error");
    expect(lastMessage(state)).toBeNull();
  });
});
