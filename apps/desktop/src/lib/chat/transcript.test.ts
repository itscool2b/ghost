import { readFileSync, writeFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { GhostEvent } from "../../bindings";
import { compactHistory, ev, numbered } from "./testing";
import {
  applyEntries,
  compactToolUpdates,
  type EventEntry,
  emptyTranscript,
  hasProcess,
  lastErrorKey,
  reduce,
  renderCache,
  renderTranscript,
  steeringState,
  stopReasonLabel,
  type ToolItem,
  type Transcript,
  toolDisplayStatus,
} from "./transcript";

const FIXTURE = new URL("./fixtures/live-stream.json", import.meta.url);
const GOLDEN = new URL("./fixtures/live-stream.transcript.json", import.meta.url);

/** The fixture: a live stream as the session actor sends it, each event with its driver's tag. */
function liveStream(): EventEntry[] {
  const tagged = JSON.parse(readFileSync(FIXTURE, "utf8")) as {
    turn: string | null;
    event: GhostEvent;
  }[];
  return numbered(tagged.map(({ turn, event }) => [turn, event] as const));
}

/** Live frames carry no turn tags: only the stored rows do. */
function asFrames(entries: readonly EventEntry[]): EventEntry[] {
  return entries.map((entry) => ({ ...entry, turnId: null }));
}

function build(entries: readonly EventEntry[], cut: string | null = null): Transcript {
  return applyEntries(emptyTranscript(cut), entries);
}

function tool(model: Transcript, id: string): ToolItem {
  const item = model.items.get(`tool:${id}`);
  if (item?.kind !== "tool") throw new Error(`no tool ${id}`);
  return item;
}

function keysOf(model: Transcript): string[][] {
  return model.segments.map((segment) => [...segment.itemKeys]);
}

/** The model's thoughts, shown or not: kept and compacted like every other item. */
function thoughtsOf(model: Transcript) {
  return [...model.items.values()].filter((item) => item.kind === "thought");
}

/** The kinds the screen shows, by segment. */
function shownKinds(model: Transcript, live = true): string[][] {
  return renderTranscript(model, live).map((segment) => segment.items.map((item) => item.kind));
}

describe("the transcript reducer", () => {
  it("keeps steering delivery on its original message across deltas and later turns", () => {
    const pending: GhostEvent = {
      type: "steering_status",
      message_id: "steer",
      state: "pending",
      detail: null,
    };
    const initial = build(
      numbered([
        [null, ev.turnStarted("first")],
        [null, pending],
        [null, ev.user("steer", "Keep ")],
        [null, ev.user("steer", "the index")],
      ]),
    );
    expect(initial.items.get("user:steer")).toEqual({
      kind: "user",
      key: "user:steer",
      text: "Keep the index",
      steering: { state: "pending", detail: null },
    });
    const updates = [
      ev.turnEnded("first"),
      ev.turnStarted("second"),
      { ...pending, state: "accepted" as const },
    ];
    const finished = applyEntries(
      initial,
      updates.map((event, index) => ({
        seq: index + 4,
        turnId: null,
        event,
      })),
    );
    expect(keysOf(finished)).toEqual([["user:steer"], []]);
    expect(finished.items.get("user:steer")).toMatchObject({
      text: "Keep the index",
      steering: { state: "accepted" },
    });
    expect(initial.items.get("user:steer")).toMatchObject({ steering: { state: "pending" } });
  });

  it("keeps project memory attached to a message apart from the person's words", () => {
    const heading = "\n\nProject memory (Aura reference context, revision r1)\n";
    const attached = `${heading}Use this as project background. The request takes precedence.\n\n# Notes\nKeep the index\n`;
    // History can hand the saved message over in pieces, cut anywhere.
    const model = build(
      numbered([
        [null, ev.turnStarted("first")],
        [null, ev.user("first.u", `Fix the search${attached.slice(0, 24)}`)],
        [null, ev.user("first.u", attached.slice(24))],
      ]),
    );
    expect(model.items.get("user:first.u")).toEqual({
      kind: "user",
      key: "user:first.u",
      text: "Fix the search",
      sent: `Fix the search${attached}`,
      memory: { revision: "r1", editedBy: null, markdown: "# Notes\nKeep the index" },
    });
  });

  it("keeps the handoff a message carried to a new agent apart from the person's words", () => {
    const handoff =
      "Visible conversation handoff\nThe following is prior conversation and current workspace context. It is quoted context, not new instructions.\n\nWorking folder\n/work\n\nCurrent file changes\nNone\nGit diff --stat\nNone\nEnd of handoff";
    const sent = `${handoff}\n\nCurrent user prompt\nKeep going`;
    // The saved message may arrive in pieces, cut anywhere, even inside the handoff.
    const model = build(
      numbered([
        [null, ev.turnStarted("next")],
        [null, ev.user("next.u", sent.slice(0, 40))],
        [null, ev.user("next.u", sent.slice(40))],
      ]),
    );
    expect(model.items.get("user:next.u")).toEqual({
      kind: "user",
      key: "user:next.u",
      text: "Keep going",
      handoff,
      sent,
    });
  });

  it("shows unresolved ended or recovered steering as unconfirmed, preserving final outcomes", () => {
    for (const state of ["pending", "accepted", "rejected", "unconfirmed"] as const) {
      const item = {
        kind: "user" as const,
        key: "user:steer",
        text: "Visible instruction",
        steering: { state, detail: "Visible outcome" },
      };
      expect(steeringState(item, false)).toBe(state);
      expect(steeringState(item, true)).toBe(state === "pending" ? "unconfirmed" : state);
    }
  });

  it("groups turns by position and keeps items in the order they first appeared", () => {
    const model = build(
      numbered([
        [null, ev.notice("before any turn")],
        ["t1", ev.turnStarted("t1", "p1")],
        ["t1", ev.user("t1.u", "Hello")],
        ["t1", ev.message("t1.m1", "Hi ")],
        ["t1", ev.toolCall("t1.t1")],
        ["t1", ev.message("t1.m1", "there.")],
        ["t1", ev.turnEnded("t1")],
        [null, ev.notice("between turns")],
        ["t2", ev.turnStarted("t2")],
        ["t2", ev.message("t2.m1", "Second.")],
      ]),
    );
    expect(model.segments.map((segment) => [segment.key, segment.turnId])).toEqual([
      ["outside:0", null],
      ["turn:t1", "t1"],
      ["outside:7", null],
      ["turn:t2", "t2"],
    ]);
    expect(keysOf(model)).toEqual([
      ["notice:0"],
      ["user:t1.u", "message:t1.m1", "tool:t1.t1"],
      ["notice:7"],
      ["message:t2.m1"],
    ]);
    const message = model.items.get("message:t1.m1");
    expect(message?.kind === "message" && message.text).toBe("Hi there.");
    expect(model.segments[1]?.end?.stopReason).toBe("end_turn");
    expect(model.segments[3]?.end).toBeNull();
  });

  it("renders an untagged late event in the turn whose span it landed in", () => {
    // A background permission request the agent sent after turn 1's reply, read while turn 2 ran.
    const model = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.turnEnded("t1")],
        ["t2", ev.turnStarted("t2")],
        [null, ev.permission("bg.p1")],
        ["t2", ev.turnEnded("t2")],
      ]),
    );
    expect(keysOf(model)).toEqual([[], ["permission:bg.p1"]]);
  });

  it("shows a thought with words or a second's time, and keeps a wordless blink in the model only", () => {
    const model = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.user("t1.u", "Fix it")],
        ["t1", ev.thought("th1", "")],
        ["t1", ev.thoughtDone("th1", 0)],
        ["t1", ev.toolCall("a", { kind: "search" })],
        ["t1", ev.thought("th2", "Now read the match.")],
        ["t1", ev.thoughtDone("th2", 1200)],
        ["t1", ev.toolCall("b", { kind: "read" })],
        ["t1", ev.message("m", "Fixed.")],
        ["t1", ev.turnEnded("t1")],
      ]),
    );
    expect(keysOf(model)[0]).toContain("thought:th1");
    expect(thoughtsOf(model).map((item) => item.key)).toEqual(["thought:th1", "thought:th2"]);
    expect(shownKinds(model)).toEqual([["user", "tool", "thought", "tool", "message"]]);
    // A wordless thought that took a second or more says how long it took.
    const slow = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.thought("th", "")],
        ["t1", ev.thoughtDone("th", 3200)],
        ["t1", ev.message("m", "Done.")],
        ["t1", ev.turnEnded("t1")],
      ]),
    );
    expect(shownKinds(slow)).toEqual([["thought", "message"]]);
  });

  it("thinks a thought only while it streams in a running turn of a live session", () => {
    const streaming = numbered([
      ["t1", ev.turnStarted("t1")],
      ["t1", ev.thought("th", "")],
    ]);
    const thought = (model: Transcript, live: boolean) =>
      renderTranscript(model, live)[0]?.items.find((item) => item.kind === "thought");
    // Even before its first word, a thought being thought shows as Thinking.
    expect(thought(build(streaming), true)).toMatchObject({ thinking: true });
    // With no process it is not being thought, and without words it has no row.
    expect(thought(build(streaming), false)).toBeUndefined();
    const ended = build([...streaming, ...numbered([["t1", ev.turnEnded("t1")]], 2)]);
    expect(thought(ended, true)).toBeUndefined();
  });

  it("renders a turn again as its reasoning streams, and reuses it while nothing shown changes", () => {
    const render = renderCache();
    let model = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.toolCall("a")],
      ]),
    );
    const before = render(model, true)[0];
    expect(render(model, true)[0]).toBe(before);
    model = reduce(model, 3, null, ev.thought("th", "Hmm"));
    const thinking = render(model, true)[0];
    expect(thinking).not.toBe(before);
    model = reduce(model, 4, null, ev.thought("th", ", then"));
    expect(render(model, true)[0]).not.toBe(thinking);
    expect(render(model, true)[0]?.items.at(-1)).toMatchObject({
      kind: "thought",
      text: "Hmm, then",
      thinking: true,
    });
  });

  it("is pure: the model it was given stays as it was", () => {
    const before = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.message("m", "a")],
      ]),
    );
    const segments = before.segments;
    const items = before.items;
    const after = reduce(before, 2, null, ev.message("m", "b"));
    expect(before.segments).toBe(segments);
    expect(before.items).toBe(items);
    const kept = before.items.get("message:m");
    expect(kept?.kind === "message" && kept.text).toBe("a");
    const grown = after.items.get("message:m");
    expect(grown?.kind === "message" && grown.text).toBe("ab");
    expect(applyEntries(after, [])).toBe(after);
  });

  it("tool_update_content_semantics: null keeps the content, a list replaces it, [] clears it", () => {
    let model = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        [
          "t1",
          ev.toolCall("t1.t1", {
            content: [{ type: "text", text: "first", truncated: null }],
            locations: [{ path: "a.rs", line: 3 }],
          }),
        ],
      ]),
    );
    model = reduce(model, 2, null, ev.toolUpdate("t1.t1", { status: "completed" }));
    expect(tool(model, "t1.t1").content).toEqual([
      { type: "text", text: "first", truncated: null },
    ]);
    expect(tool(model, "t1.t1").locations).toEqual([{ path: "a.rs", line: 3 }]);
    expect(tool(model, "t1.t1").status).toBe("completed");

    const replaced = [{ type: "terminal" as const, output: "ok", exit_code: 0, truncated: null }];
    model = reduce(model, 3, null, ev.toolUpdate("t1.t1", { content: replaced }));
    expect(tool(model, "t1.t1").content).toEqual(replaced);

    model = reduce(model, 4, null, ev.toolUpdate("t1.t1", { content: [], title: "Renamed" }));
    expect(tool(model, "t1.t1").content).toEqual([]);
    expect(tool(model, "t1.t1").title).toBe("Renamed");
    expect(tool(model, "t1.t1").locations).toEqual([{ path: "a.rs", line: 3 }]);
  });

  it("shows an update for a tool never announced as a tool with ACP's defaults", () => {
    const model = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.toolUpdate("x")],
      ]),
    );
    // The title is the one compaction gives the call it synthesizes for such a tool.
    expect(tool(model, "x")).toMatchObject({ toolKind: "other", title: "Tool", status: "pending" });
  });

  it("applies an update to an earlier turn's tool in place", () => {
    const model = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.toolCall("t1.t1", { status: "in_progress" })],
        ["t1", ev.turnEnded("t1")],
        ["t2", ev.turnStarted("t2")],
        ["t2", ev.toolUpdate("t1.t1", { status: "completed" })],
      ]),
    );
    expect(keysOf(model)).toEqual([["tool:t1.t1"], []]);
    expect(tool(model, "t1.t1").status).toBe("completed");
  });

  it("keeps a turn's last plan and usage, and only a turn's usage", () => {
    const model = build(
      numbered([
        [null, ev.usage(0.5)],
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.plan("a", "b")],
        ["t1", ev.usage(0.1)],
        ["t1", ev.plan("a", "b", "c")],
        ["t1", ev.usage(0.2)],
        ["t1", ev.turnEnded("t1")],
      ]),
    );
    expect(model.segments).toHaveLength(1);
    expect(model.segments[0]?.plan?.map((entry) => entry.content)).toEqual(["a", "b", "c"]);
    expect(model.segments[0]?.usage?.cost?.amount).toBe(0.2);
  });

  it("closes cards with their resolution; open ones are requests without one", () => {
    const model = build(
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.permission("a")],
        ["t1", ev.permission("b")],
        ["t1", ev.question("q")],
        ["t1", ev.permissionResolved("a", "allow", "auto")],
        ["t1", ev.questionResolved("q", null, "agent")],
        [null, ev.permissionResolved("unknown", null)],
      ]),
    );
    const a = model.items.get("permission:a");
    const b = model.items.get("permission:b");
    const q = model.items.get("question:q");
    expect(a?.kind === "permission" && a.resolution).toEqual({
      decision: { type: "selected", option_id: "allow" },
      by: "auto",
    });
    expect(b?.kind === "permission" && b.resolution).toBeNull();
    expect(q?.kind === "question" && q.resolution).toEqual({ answer: null, by: "agent" });
    expect(model.items.has("permission:unknown")).toBe(false);
  });

  it("ignores live header state, which history never holds", () => {
    const model = build(
      numbered([
        [null, ev.session({ status: "running" })],
        [null, ev.lifecycle("p", "queued", "hi")],
        [null, ev.limits()],
      ]),
    );
    expect(model.segments).toEqual([]);
  });

  it("partial_turn_page: a page that cut a turn starts inside it, and the earlier page continues it", () => {
    const whole = numbered([
      [null, ev.notice("start")],
      ["t1", ev.turnStarted("t1")],
      ["t1", ev.user("t1.u", "Long turn")],
      ["t1", ev.message("t1.m1", "part one, ")],
      ["t1", ev.message("t1.m1", "part two")],
      ["t1", ev.toolCall("t1.t1")],
      ["t1", ev.turnEnded("t1")],
    ]);
    // The newest page holds only the turn's last rows.
    const newest = build(whole.slice(4), "t1");
    expect(newest.segments).toHaveLength(1);
    expect(newest.segments[0]).toMatchObject({ key: "turn:t1", turnId: "t1", partial: true });
    expect(keysOf(newest)).toEqual([["message:t1.m1", "tool:t1.t1"]]);
    expect(newest.segments[0]?.end?.stopReason).toBe("end_turn");

    // "Load earlier" brings the rows before: the same turn continues, whole again.
    const both = build(whole);
    expect(both.segments.map((segment) => [segment.key, segment.partial])).toEqual([
      ["outside:0", false],
      ["turn:t1", false],
    ]);
    const message = both.items.get("message:t1.m1");
    expect(message?.kind === "message" && message.text).toBe("part one, part two");
  });

  it("shows an end whose start is not loaded as an ended partial turn", () => {
    const model = build(numbered([["t9", ev.turnEnded("t9", "cancelled")]], 40));
    expect(model.segments[0]).toMatchObject({ key: "turn:t9", partial: true });
    expect(model.segments[0]?.end?.stopReason).toBe("cancelled");
  });

  it("marks where unsaved rows are missing", () => {
    const model = applyEntries(emptyTranscript(), [
      ...numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.message("m", "saved")],
      ]),
      { seq: 5, gap: true },
      { seq: 9, turnId: null, event: ev.message("m2", "live") },
    ]);
    expect(keysOf(model)).toEqual([["message:m", "gap:5", "message:m2"]]);
  });

  it("finds the newest error card", () => {
    const model = build(
      numbered([
        [null, ev.error("spawn_failed", "first")],
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.error("agent_error", "second")],
        ["t1", ev.turnEnded("t1", "error")],
      ]),
    );
    expect(lastErrorKey(model)).toBe("error:2");
    expect(lastErrorKey(emptyTranscript())).toBeNull();
  });
});

describe("what a tool's status mark shows", () => {
  const running: ToolItem = {
    kind: "tool",
    key: "tool:t",
    id: "t",
    toolKind: "execute",
    title: "Run",
    status: "in_progress",
    locations: [],
    rawInput: null,
    toolName: null,
    content: [],
    truncated: null,
  };
  const end = (stopReason: "end_turn" | "cancelled" | "interrupted" | "error" | "refusal") => ({
    stopReason,
    usage: null,
    durationMs: null,
    recovered: false,
  });

  it("stops a tool whose turn ended cancelled, interrupted or failed", () => {
    for (const reason of ["cancelled", "interrupted", "error"] as const) {
      expect(toolDisplayStatus(running, end(reason), true)).toBe("stopped");
    }
  });

  it("keeps the reported status after a normal end, since a background task may finish it", () => {
    expect(toolDisplayStatus(running, end("end_turn"), true)).toBe("in_progress");
    expect(toolDisplayStatus(running, end("refusal"), true)).toBe("in_progress");
    expect(toolDisplayStatus(running, null, true)).toBe("in_progress");
  });

  it("stops every unfinished tool when the session has no process", () => {
    expect(toolDisplayStatus(running, null, false)).toBe("stopped");
    expect(toolDisplayStatus({ ...running, status: "pending" }, end("end_turn"), false)).toBe(
      "stopped",
    );
    expect(toolDisplayStatus({ ...running, status: "completed" }, null, false)).toBe("completed");
    expect(toolDisplayStatus({ ...running, status: "failed" }, end("error"), true)).toBe("failed");
  });

  it("knows which statuses have a process", () => {
    expect(
      ["starting", "ready", "running", "cancelling", "stopping"].every((s) =>
        hasProcess(s as never),
      ),
    ).toBe(true);
    expect(["stopped", "failed", "auth_required"].some((s) => hasProcess(s as never))).toBe(false);
  });
});

describe("stop reason labels", () => {
  it("says why each turn ended", () => {
    const label = (
      stopReason: Parameters<typeof stopReasonLabel>[0]["stopReason"],
      recovered = false,
    ) => stopReasonLabel({ stopReason, usage: null, durationMs: null, recovered });
    expect(label("end_turn")).toBe("Done");
    expect(label("max_tokens")).toBe("Stopped at the output limit");
    expect(label("max_turn_requests")).toBe("Stopped at the step limit");
    expect(label("refusal")).toBe("The agent declined");
    expect(label("cancelled")).toBe("Stopped");
    expect(label("interrupted")).toBe("Interrupted");
    expect(label("interrupted", true)).toBe("Interrupted when Ghost closed");
    expect(label("error")).toBe("Failed");
    expect(label("other")).toBe("Ended");
  });
});

describe("compacted_equals_live", () => {
  it("renders the golden stream the same live and from compacted history", () => {
    const live = liveStream();
    const history = compactHistory(live);
    expect(history.length).toBeLessThan(live.length);

    const fromLive = renderTranscript(build(asFrames(live)), true);
    const fromHistory = renderTranscript(build(history), true);
    expect(fromHistory).toEqual(fromLive);
    // With no process, every unfinished tool shows stopped, in both.
    expect(renderTranscript(build(history), false)).toEqual(
      renderTranscript(build(asFrames(live)), false),
    );
    // The stream's thoughts show as rows, and compaction keeps each one whole.
    expect(thoughtsOf(build(asFrames(live))).length).toBeGreaterThan(0);
    expect(thoughtsOf(build(history))).toEqual(thoughtsOf(build(asFrames(live))));
    expect(shownKinds(build(history)).flat()).toContain("thought");

    if (process.env.GHOST_UPDATE_GOLDEN === "1") {
      writeFileSync(GOLDEN, `${JSON.stringify(fromLive, null, 2)}\n`);
    }
    const golden = JSON.parse(readFileSync(GOLDEN, "utf8")) as unknown;
    expect(JSON.parse(JSON.stringify(fromLive))).toEqual(golden);
  });

  it("keeps the cross-turn and late updates of the golden stream", () => {
    const model = build(compactHistory(liveStream()));
    // A background update in turn 2 renamed turn 1's read and cleared its content.
    expect(tool(model, "t1.t1")).toMatchObject({ title: "Read src/lib.rs again", content: [] });
    // The untagged late update after turn 1 finished its test run.
    expect(tool(model, "t1.t3").status).toBe("completed");
    const message = model.items.get("message:t1.m1");
    expect(message?.kind === "message" && message.text).toBe(
      "The test failed because of a typo. **Fixed.** (one more thing, sent late)",
    );
    // Turn 2 was cancelled with its search still running: stopped.
    const rendered = renderTranscript(model, true);
    const search = rendered[3]?.items.find((item) => item.key === "tool:t2.t1");
    expect(search?.kind === "tool" && search.display).toBe("stopped");
    // The background request landed in turn 2's span and is still open.
    expect(rendered[3]?.items.map((item) => item.key)).toContain("permission:bg.p1");
  });

  it("renders generated streams the same live and compacted", () => {
    for (let seed = 1; seed <= 300; seed += 1) {
      const live = generatedStream(seed);
      for (const maxPiece of [1024 * 1024, 7]) {
        const fromLive = renderTranscript(build(asFrames(live)), true);
        const fromHistory = renderTranscript(build(compactHistory(live, maxPiece)), true);
        expect(fromHistory, `seed ${seed}, pieces of ${maxPiece}`).toEqual(fromLive);
        expect(
          thoughtsOf(build(compactHistory(live, maxPiece))),
          `seed ${seed} thoughts, pieces of ${maxPiece}`,
        ).toEqual(thoughtsOf(build(asFrames(live))));
      }
    }
  });
});

/** A small deterministic generator (mulberry32). */
function random(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * A live stream of a few turns with what drivers and the session actor send: split deltas,
 * thoughts and their completions, tools with updates (to this turn's tools, to earlier turns'
 * tools, and untagged late ones), plans (tagged and late), usage, cards with and without answers,
 * notices and errors in turns and between them, and live header state.
 *
 * Usage outside a turn is never stored, and frames carry no turn tags, so an untagged usage
 * report can render the same live and from history only where no turn is open: the generator
 * sends it between turns. (A late one landing in the next turn's span would show as that turn's
 * cost live until the turn's own report replaces it.)
 */
function generatedStream(seed: number): EventEntry[] {
  const next = random(seed);
  const pick = <T>(choices: readonly T[]): T => choices[Math.floor(next() * choices.length)] as T;
  const tagged: [string | null, GhostEvent][] = [];
  const tools: string[] = [];
  const messages: string[] = [];
  const turns = 1 + Math.floor(next() * 4);
  for (let turn = 1; turn <= turns; turn += 1) {
    const id = `t${turn}`;
    tagged.push([null, ev.session({ status: "running", revision: turn })]);
    tagged.push([id, ev.turnStarted(id, `p${turn}`)]);
    tagged.push([id, ev.user(`${id}.u`, `prompt ${turn}`)]);
    const steps = Math.floor(next() * 12);
    let toolCount = 0;
    let messageCount = 0;
    let thoughtCount = 0;
    for (let step = 0; step < steps; step += 1) {
      const roll = next();
      if (roll < 0.22) {
        messageCount += 1;
        const message = `${id}.m${messageCount}`;
        messages.push(message);
        for (let part = Math.floor(next() * 4); part >= 0; part -= 1) {
          tagged.push([id, ev.message(message, pick(["a", "bc", "def ", "ghij\n", "é"]))]);
        }
        if (next() < 0.5) tagged.push([id, ev.messageDone(message)]);
      } else if (roll < 0.3) {
        thoughtCount += 1;
        const thought = `${id}.th${thoughtCount}`;
        for (let part = Math.floor(next() * 3); part >= 0; part -= 1) {
          tagged.push([id, ev.thought(thought, pick(["hm", "so", " then", "ü"]))]);
        }
        if (next() < 0.6) tagged.push([id, ev.thoughtDone(thought, pick([null, 1200, 30]))]);
      } else if (roll < 0.33) {
        tagged.push([id, ev.error("agent_error", `failed at ${step}`, pick([null, "detail"]))]);
      } else if (roll < 0.35) {
        // A plan the agent sent after an earlier turn's reply: untagged, so stored as it is.
        tagged.push([null, ev.plan(`late ${step}`)]);
      } else if (roll < 0.55) {
        toolCount += 1;
        const tool = `${id}.t${toolCount}`;
        tools.push(tool);
        if (next() < 0.8)
          tagged.push([id, ev.toolCall(tool, { status: pick(["pending", "in_progress"]) })]);
        tagged.push([
          id,
          ev.toolUpdate(tool, {
            status: pick([null, "in_progress", "completed", "failed"]),
            content: pick([null, [], [{ type: "text", text: `out ${step}`, truncated: null }]]),
            title: pick([null, `title ${step}`]),
          }),
        ]);
      } else if (roll < 0.65 && tools.length > 0) {
        // An update to any tool so far. One of this turn's is tagged with it (the driver reads
        // it while the prompt runs); an earlier turn's arrives tagged with this turn (a background
        // task) or untagged (read after its own turn's reply).
        const target = pick(tools);
        tagged.push([
          target.startsWith(`${id}.`) ? id : pick([id, null]),
          ev.toolUpdate(target, {
            status: pick([null, "completed"]),
            content: pick([null, [], [{ type: "text", text: `late ${step}`, truncated: null }]]),
          }),
        ]);
      } else if (roll < 0.72) {
        tagged.push([
          id,
          ev.plan(
            ...Array.from({ length: 1 + Math.floor(next() * 3) }, (_, i) => `step ${step}.${i}`),
          ),
        ]);
      } else if (roll < 0.78) {
        tagged.push([id, ev.usage(step / 100)]);
      } else if (roll < 0.84) {
        const request = `${id}.r${step}`;
        tagged.push([pick([id, null]), ev.permission(request)]);
        if (next() < 0.6) tagged.push([id, ev.permissionResolved(request, pick(["allow", null]))]);
      } else if (roll < 0.88) {
        const request = `${id}.q${step}`;
        tagged.push([id, ev.question(request)]);
        if (next() < 0.6)
          tagged.push([id, ev.questionResolved(request, { type: "select", index: 0 })]);
      } else if (roll < 0.92 && messages.some((m) => !m.startsWith(`${id}.`))) {
        // Late agent output for an earlier turn's message, read after that turn's reply.
        const earlier = messages.filter((m) => !m.startsWith(`${id}.`));
        tagged.push([null, ev.message(pick(earlier), " late")]);
      } else if (roll < 0.96) {
        tagged.push([pick([id, null]), ev.notice(`note ${step}`)]);
      } else {
        tagged.push([null, ev.lifecycle(`p${turn + 1}`, "queued", "next")]);
      }
    }
    tagged.push([id, ev.turnEnded(id, pick(["end_turn", "cancelled", "error", "interrupted"]))]);
    if (next() < 0.5) tagged.push([null, ev.notice(`after ${turn}`)]);
    if (next() < 0.3) tagged.push([null, ev.usage(turn)]);
    if (next() < 0.2) tagged.push([null, ev.error("agent_exited", "The agent stopped.")]);
  }
  return numbered(tagged);
}

describe("folding a tool's updates", () => {
  const content = (text: string) => [{ type: "text" as const, text, truncated: null }];
  const raw = numbered([
    ["t1", ev.turnStarted("t1")],
    ["t1", ev.user("t1.u", "Run it")],
    // Updates of a tool never announced in this history: the first one places it.
    ["t1", ev.toolUpdate("early", { title: "Early", content: content("a") })],
    ["t1", ev.toolCall("run", { kind: "execute", title: "Run tests" })],
    ["t1", ev.toolUpdate("run", { content: content("1") })],
    ["t1", ev.message("t1.m1", "Running ")],
    ["t1", ev.toolUpdate("early", { status: "completed" })],
    ["t1", ev.toolUpdate("run", { content: content("1 2"), truncated: null })],
    ["t1", ev.toolUpdate("run", { title: "Run all tests" })],
    // A second call with the same id replaces the item: updates after it fold separately.
    ["t1", ev.toolCall("run", { kind: "execute", title: "Again" })],
    ["t1", ev.toolUpdate("run", { content: content("3") })],
    ["t1", ev.toolUpdate("run", { status: "failed" })],
    ["t1", ev.message("t1.m1", "done")],
    ["t1", ev.turnEnded("t1")],
  ]);

  it("leaves the transcript as it was, with one update per tool and call", () => {
    const folded = compactToolUpdates(raw);
    const updates = folded.filter((e) => "event" in e && e.event.type === "tool_update");
    expect(updates).toHaveLength(3);
    expect(folded).toHaveLength(raw.length - 4);
    const whole = applyEntries(emptyTranscript(), raw);
    expect(applyEntries(emptyTranscript(), folded)).toEqual(whole);
  });

  it("folds tens of thousands of streamed updates into one", () => {
    const stream = numbered([
      ["t1", ev.turnStarted("t1")],
      ["t1", ev.toolCall("sh", { kind: "execute" })],
      ...Array.from(
        { length: 50_000 },
        (_, i) => ["t1", ev.toolUpdate("sh", { content: content(`line ${i}`) })] as const,
      ),
      ["t1", ev.turnEnded("t1")],
    ]);
    const folded = compactToolUpdates(stream);
    expect(folded).toHaveLength(4);
    expect(applyEntries(emptyTranscript(), folded)).toEqual(
      applyEntries(emptyTranscript(), stream),
    );
  });
});
