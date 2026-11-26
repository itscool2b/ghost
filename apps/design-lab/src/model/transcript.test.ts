import { describe, expect, it } from "vitest";
import { APPROVAL, APPROVAL_REQUEST_ID } from "../fixture/approval";
import { GOLDEN, GOLDEN_ANSWER, GOLDEN_PROMPT, GOLDEN_TURN_ID } from "../fixture/golden";
import type { LabEvent, TimedEvent, ToolCall } from "../fixture/types";
import { parseMarkdown } from "./markdown";
import { EMPTY_TRANSCRIPT, eventsAt, reduceTranscript } from "./transcript";
import type {
  MessageItem,
  PermissionItem,
  ThoughtItem,
  ToolRunItem,
  TranscriptModel,
  Turn,
  TurnItem,
} from "./types";

/** Events one second apart, starting at 1 s. */
function timeline(...events: LabEvent[]): TimedEvent[] {
  return events.map((event, index) => ({ at: (index + 1) * 1000, event }));
}

const user = (id: string, text = "Do the thing"): LabEvent => ({
  type: "user_message",
  message_id: id,
  text,
  agent: "claude-code",
});
const started = (turnId: string): LabEvent => ({ type: "turn_started", turn_id: turnId });
const ended = (
  turnId: string,
  stop: "end_turn" | "cancelled" | "error" = "end_turn",
): LabEvent => ({
  type: "turn_ended",
  turn_id: turnId,
  stop_reason: stop,
  usage: { input_tokens: 10, output_tokens: 5, cached_input_tokens: 100, reasoning_tokens: null },
});
const say = (id: string, text: string): LabEvent => ({
  type: "message_delta",
  message_id: id,
  text,
});
const think = (id: string, text: string): LabEvent => ({
  type: "thought_delta",
  thought_id: id,
  text,
});
const tool = (
  id: string,
  kind: ToolCall["kind"],
  target: string,
  status: ToolCall["status"] = "in_progress",
): LabEvent => ({
  type: "tool_call",
  tool_call_id: id,
  kind,
  title: `${kind} ${target}`,
  status,
  locations: kind === "execute" || kind === "search" ? [] : [{ path: target, line: null }],
  raw_input:
    kind === "execute" ? { command: target } : kind === "search" ? { pattern: target } : null,
});
/** Completes a call, optionally with content (diffs, text, terminal output) as the agent sends it. */
const done = (id: string, content: readonly object[] = []): LabEvent =>
  ({
    type: "tool_update",
    tool_call_id: id,
    status: "completed",
    title: null,
    content,
  }) as LabEvent;

function only<T>(items: readonly T[]): T {
  expect(items).toHaveLength(1);
  return items[0] as T;
}

const turnOf = (model: TranscriptModel): Turn => only(model.turns);
const itemsOf = <K extends TurnItem["kind"]>(model: TranscriptModel, kind: K) =>
  turnOf(model).items.filter((item): item is Extract<TurnItem, { kind: K }> => item.kind === kind);

const at = (seconds: number) =>
  reduceTranscript(GOLDEN.events.slice(0, eventsAt(GOLDEN.events, seconds * 1000)));

describe("reduceTranscript basics", () => {
  it("starts empty and idle", () => {
    expect(EMPTY_TRANSCRIPT).toMatchObject({
      turns: [],
      plan: null,
      pendingPermissions: [],
      activity: "idle",
      aura: "calm",
      eventCount: 0,
      lastEventAt: null,
    });
  });

  it("opens a turn on the user's message and names it when the agent starts", () => {
    const waiting = reduceTranscript(timeline(user("u1", "Hello")));
    expect(turnOf(waiting)).toMatchObject({
      user: { id: "u1", text: "Hello", at: 1000 },
      agent: "claude-code",
      status: "running",
      startedAt: 1000,
    });
    expect(waiting.activity).toBe("thinking");

    const named = reduceTranscript(timeline(user("u1"), started("t1")));
    expect(turnOf(named)).toMatchObject({ id: "t1", startedAt: 2000 });
  });

  it("opens a turn for agent output that arrives without one", () => {
    const own = reduceTranscript(timeline(started("t1"), say("m1", "hi")));
    expect(turnOf(own)).toMatchObject({ id: "t1", user: null });
    const orphan = reduceTranscript(timeline(say("m1", "hi")));
    expect(turnOf(orphan)).toMatchObject({ id: "implicit-1", user: null, status: "running" });
  });

  it("accumulates deltas and tracks streaming", () => {
    const model = reduceTranscript(
      timeline(user("u1"), started("t1"), say("m1", "Hel"), say("m1", "lo")),
    );
    const message = only(itemsOf(model, "message"));
    expect(message).toMatchObject({
      text: "Hello",
      streaming: true,
      startedAt: 3000,
      endedAt: null,
    });
    expect(model.activity).toBe("streaming");
    expect(model.aura).toBe("active");

    const completed = reduceTranscript(
      timeline(user("u1"), started("t1"), say("m1", "Hello"), {
        type: "message_completed",
        message_id: "m1",
      }),
    );
    expect(only(itemsOf(completed, "message"))).toMatchObject({ streaming: false, endedAt: 4000 });
    expect(completed.activity).toBe("thinking");
  });

  it("finishes open text when the next block starts, or when the turn ends", () => {
    const model = reduceTranscript(
      timeline(user("u1"), started("t1"), think("th1", "hmm"), say("m1", "Answer"), ended("t1")),
    );
    const [thought, message] = turnOf(model).items as [ThoughtItem, MessageItem];
    expect(thought).toMatchObject({ streaming: false, endedAt: 4000, durationMs: 1000 });
    expect(message).toMatchObject({ streaming: false, endedAt: 5000 });
  });

  it("prefers the thought duration the agent reports", () => {
    const model = reduceTranscript(
      timeline(user("u1"), started("t1"), think("th1", "hmm"), {
        type: "thought_completed",
        thought_id: "th1",
        duration_ms: 12_000,
      }),
    );
    expect(only(itemsOf(model, "thought"))).toMatchObject({ durationMs: 12_000, streaming: false });
    const live = reduceTranscript(timeline(user("u1"), started("t1"), think("th1", "hmm")));
    expect(only(itemsOf(live, "thought"))).toMatchObject({ durationMs: null, streaming: true });
    expect(live.activity).toBe("thinking");
  });

  it("ends a turn with its outcome, footer and usage", () => {
    const model = reduceTranscript(
      timeline(user("u1"), started("t1"), say("m1", "ok"), ended("t1")),
    );
    expect(turnOf(model)).toMatchObject({
      status: "completed",
      stopReason: "end_turn",
      endedAt: 4000,
    });
    expect(turnOf(model).footer).toMatchObject({
      durationMs: 2000,
      stopReason: "end_turn",
      usage: { input_tokens: 10, output_tokens: 5, cached_input_tokens: 100 },
      files: [],
    });
    expect(model.activity).toBe("done");
    expect(
      reduceTranscript(timeline(user("u1"), started("t1"), ended("t1", "cancelled"))).activity,
    ).toBe("idle");
    const failed = reduceTranscript(timeline(user("u1"), started("t1"), ended("t1", "error")));
    expect(turnOf(failed).status).toBe("failed");
    expect(failed.activity).toBe("error");
  });

  it("starts a new turn for each user message", () => {
    const model = reduceTranscript(
      timeline(
        user("u1"),
        started("t1"),
        say("m1", "one"),
        ended("t1"),
        user("u2"),
        started("t2"),
        say("m2", "two"),
      ),
    );
    expect(model.turns.map((turn) => [turn.id, turn.status])).toEqual([
      ["t1", "completed"],
      ["t2", "running"],
    ]);
    expect(model.activity).toBe("streaming");
  });

  it("ignores events it does not know", () => {
    const strange = { type: "teleport", to: "mars" } as unknown as LabEvent;
    const model = reduceTranscript(timeline(user("u1"), strange));
    expect(model.eventCount).toBe(2);
    expect(turnOf(model).items).toEqual([]);
  });
});

describe("tool calls", () => {
  it("merges updates: status and title replace, content replaces only when present", () => {
    const content = [{ type: "text", text: "a.ts:1:match" }];
    const model = reduceTranscript(
      timeline(
        user("u1"),
        started("t1"),
        tool("c1", "search", "match"),
        {
          type: "tool_update",
          tool_call_id: "c1",
          status: null,
          title: "Searching",
          content: content as never,
        },
        { type: "tool_update", tool_call_id: "c1", status: "completed", title: null, content: [] },
      ),
    );
    const call = only(only(itemsOf(model, "tools")).calls);
    expect(call).toMatchObject({
      title: "Searching",
      status: "completed",
      text: "a.ts:1:match",
      endedAt: 5000,
    });
    expect(call.search).toEqual({
      pattern: "match",
      matches: [{ path: "a.ts", line: 1, text: "match" }],
    });
  });

  it("shows an update for a call it never saw as a generic tool", () => {
    const model = reduceTranscript(
      timeline(user("u1"), started("t1"), {
        type: "tool_update",
        tool_call_id: "ghost",
        status: "in_progress",
        title: null,
        content: [],
      }),
    );
    const call = only(only(itemsOf(model, "tools")).calls);
    expect(call).toMatchObject({
      id: "ghost",
      kind: "other",
      title: "Tool",
      status: "in_progress",
    });
  });

  it("groups consecutive calls of one family, and breaks runs on messages, not plans", () => {
    const model = reduceTranscript(
      timeline(
        user("u1"),
        started("t1"),
        tool("r1", "read", "a.ts"),
        done("r1"),
        tool("r2", "read", "b.ts"),
        done("r2"),
        tool("s1", "search", "needle"),
        done("s1"),
        {
          type: "plan_updated",
          entries: [{ content: "Step", status: "in_progress", priority: null }],
        },
        tool("e1", "edit", "a.ts"),
        done("e1"),
        say("m1", "Now the tests."),
        tool("x1", "execute", "pnpm test"),
      ),
    );
    const runs = itemsOf(model, "tools");
    expect(runs.map((run) => [run.family, run.summary, run.status])).toEqual([
      ["explore", "Read 2 files, searched 1 pattern", "completed"],
      ["edit", "Edited a.ts", "completed"],
      ["execute", "Ran pnpm test", "running"],
    ]);
    expect(runs[2]?.label).toBe("Running pnpm test");
    expect(model.activity).toBe("tool");
  });

  it("labels a running run by what runs now, and a finished one by its summary", () => {
    const model = reduceTranscript(
      timeline(
        user("u1"),
        started("t1"),
        tool("r1", "read", "src/a.ts"),
        done("r1"),
        tool("r2", "read", "src/b.ts"),
      ),
    );
    const run = only(itemsOf(model, "tools"));
    expect(run).toMatchObject({
      label: "Reading b.ts",
      summary: "Read 2 files",
      status: "running",
    });
    expect(run.calls.map((call) => [call.label, call.pastLabel])).toEqual([
      ["Read a.ts", "Read a.ts"],
      ["Reading b.ts", "Read b.ts"],
    ]);
  });

  it("gives calls still running at the end of a turn the turn's outcome", () => {
    const completed = reduceTranscript(
      timeline(user("u1"), started("t1"), tool("x1", "execute", "make"), ended("t1")),
    );
    expect(only(only(itemsOf(completed, "tools")).calls)).toMatchObject({
      status: "completed",
      endedAt: 4000,
    });
    const cancelled = reduceTranscript(
      timeline(user("u1"), started("t1"), tool("x1", "execute", "make"), ended("t1", "cancelled")),
    );
    expect(only(itemsOf(cancelled, "tools"))).toMatchObject({ status: "failed" });
  });

  it("counts diffs per run and per turn", () => {
    const diff = (path: string, oldText: string | null, newText: string) => ({
      type: "diff",
      path,
      old_text: oldText,
      new_text: newText,
    });
    const model = reduceTranscript(
      timeline(
        user("u1"),
        started("t1"),
        tool("e1", "edit", "a.ts"),
        done("e1", [diff("a.ts", "x\n", "y\n")]),
        tool("e2", "edit", "b.ts"),
        done("e2", [diff("b.ts", null, "1\n2\n")]),
        tool("e3", "edit", "a.ts"),
        done("e3", [diff("a.ts", "y\n", "y\nz\n")]),
        ended("t1"),
      ),
    );
    const run = only(itemsOf(model, "tools"));
    expect(run).toMatchObject({
      summary: "Edited 2 files",
      added: 4,
      removed: 1,
      files: ["a.ts", "b.ts"],
    });
    expect(turnOf(model).footer).toMatchObject({
      added: 4,
      removed: 1,
      files: [
        { path: "a.ts", added: 2, removed: 1, isNew: false },
        { path: "b.ts", added: 2, removed: 0, isNew: true },
      ],
    });
  });

  it("parses terminal output and knows when it still runs", () => {
    const model = reduceTranscript(
      timeline(user("u1"), started("t1"), tool("x1", "execute", "pnpm test"), {
        type: "tool_update",
        tool_call_id: "x1",
        status: null,
        title: null,
        content: [
          {
            type: "terminal",
            output: "\u001b[32mok\u001b[39m\n",
            exit_code: null,
            truncated: false,
          },
        ],
      }),
    );
    const call = only(only(itemsOf(model, "tools")).calls);
    expect(call.terminal).toMatchObject({
      command: "pnpm test",
      exitCode: null,
      running: true,
      truncated: false,
    });
    expect(call.terminal?.lines[0]?.[0]).toMatchObject({ text: "ok", fg: "green" });
  });
});

describe("permissions", () => {
  const request = (id: string, callId: string | null): LabEvent => ({
    type: "permission_requested",
    request_id: id,
    tool_call_id: callId,
    title: "Run make?",
    options: [
      { option_id: "yes", kind: "allow_once", label: "Allow once" },
      { option_id: "always", kind: "allow_always", label: "Always allow" },
      { option_id: "no", kind: "reject_once", label: "Reject" },
    ],
  });

  it("waits on a pending request, inline and in the pending list", () => {
    const model = reduceTranscript(
      timeline(
        user("u1"),
        started("t1"),
        tool("x1", "execute", "make", "pending"),
        request("p1", "x1"),
      ),
    );
    expect(turnOf(model).status).toBe("waiting");
    expect(model.activity).toBe("waiting");
    expect(model.aura).toBe("alert");
    const card = only(itemsOf(model, "permission"));
    expect(card.permission).toMatchObject({
      requestId: "p1",
      status: "pending",
      title: "Run make?",
    });
    expect(card.permission.tool).toMatchObject({ id: "x1", awaitingPermission: true });
    expect(only(model.pendingPermissions).requestId).toBe("p1");
    const run = only(itemsOf(model, "tools"));
    expect(run).toMatchObject({ status: "waiting", label: "Waiting to run make" });
  });

  it("resolves by the chosen option's kind", () => {
    const resolve = (decision: unknown) =>
      reduceTranscript(
        timeline(user("u1"), started("t1"), request("p1", null), {
          type: "permission_resolved",
          request_id: "p1",
          decision,
        } as LabEvent),
      );
    const status = (model: TranscriptModel) =>
      (turnOf(model).items[0] as PermissionItem).permission.status;
    expect(status(resolve({ type: "selected", option_id: "yes" }))).toBe("allowed");
    expect(status(resolve({ type: "selected", option_id: "always" }))).toBe("allowed");
    expect(status(resolve({ type: "selected", option_id: "no" }))).toBe("rejected");
    expect(status(resolve({ type: "selected", option_id: "nonsense" }))).toBe("cancelled");
    expect(status(resolve({ type: "cancelled" }))).toBe("cancelled");
    const allowed = resolve({ type: "selected", option_id: "yes" });
    expect(allowed.pendingPermissions).toEqual([]);
    expect(allowed.activity).toBe("thinking");
  });

  it("cancels what is still pending when the turn ends", () => {
    const model = reduceTranscript(
      timeline(user("u1"), started("t1"), request("p1", null), ended("t1", "cancelled")),
    );
    expect((turnOf(model).items[0] as PermissionItem).permission).toMatchObject({
      status: "cancelled",
      resolvedAt: 4000,
    });
    expect(model.pendingPermissions).toEqual([]);
  });
});

describe("session, usage, plan, notices", () => {
  it("keeps the latest of each", () => {
    const model = reduceTranscript(
      timeline(
        user("u1"),
        started("t1"),
        {
          type: "session_updated",
          native_session_id: "s",
          title: "T",
          model: "m1",
          effort: "low",
          mode: null,
          commands: [],
        },
        {
          type: "session_updated",
          native_session_id: "s",
          title: "T",
          model: "m2",
          effort: "high",
          mode: "x",
          commands: [],
        },
        {
          type: "usage_updated",
          context_used: 10,
          context_size: 100,
          cost: { amount: 0.5, currency: "USD" },
        },
        {
          type: "plan_updated",
          entries: [
            { content: "A", status: "completed", priority: null },
            { content: "B", status: "pending", priority: "high" },
          ],
        },
        { type: "limits_updated", windows: [{ label: "5h", used_percent: 31, resets_at: null }] },
        { type: "notice", level: "warning", message: "untested version" },
        { type: "error", message: "boom", recoverable: true },
        ended("t1"),
      ),
    );
    expect(model.session).toEqual({
      nativeSessionId: "s",
      title: "T",
      model: "m2",
      effort: "high",
      mode: "x",
    });
    expect(model.usage).toEqual({
      context_used: 10,
      context_size: 100,
      cost: { amount: 0.5, currency: "USD" },
    });
    expect(model.plan).toMatchObject({
      completed: 1,
      total: 2,
      done: false,
      turnId: "t1",
      current: { content: "B" },
    });
    expect(model.limits).toHaveLength(1);
    expect(model.notices).toEqual([
      { level: "warning", message: "untested version", at: 8000, turnId: "t1" },
      { level: "error", message: "boom", at: 9000, turnId: "t1" },
    ]);
    expect(turnOf(model).footer).toMatchObject({
      cost: { amount: 0.5 },
      contextUsed: 10,
      contextSize: 100,
      model: "m2",
      effort: "high",
    });
  });
});

describe("eventsAt", () => {
  it("counts events that have arrived, ties included", () => {
    const events = [0, 100, 100, 250].map((at) => ({ at, event: say("m", "x") }));
    expect(eventsAt(events, -1)).toBe(0);
    expect(eventsAt(events, 0)).toBe(1);
    expect(eventsAt(events, 99)).toBe(1);
    expect(eventsAt(events, 100)).toBe(3);
    expect(eventsAt(events, 1_000)).toBe(4);
    expect(eventsAt([], 5)).toBe(0);
  });
});

describe("structural sharing", () => {
  it("keeps finished blocks identical while the last one streams", () => {
    const count = eventsAt(GOLDEN.events, 30_000);
    const before = reduceTranscript(GOLDEN.events.slice(0, count));
    const after = reduceTranscript(GOLDEN.events.slice(0, count + 1), before);
    const beforeItems = turnOf(before).items;
    const afterItems = turnOf(after).items;
    expect(after).not.toBe(before);
    for (let index = 0; index < beforeItems.length - 1; index += 1) {
      expect(afterItems[index]).toBe(beforeItems[index]);
    }
    expect(reduceTranscript(GOLDEN.events.slice(0, count), before)).toBe(before);
  });
});

describe("the golden turn", () => {
  it("opens on the prompt and thinks", () => {
    const model = at(0);
    expect(turnOf(model).user?.text).toBe(GOLDEN_PROMPT);
    expect(model.activity).toBe("thinking");
    const thinking = at(7.5);
    expect(only(itemsOf(thinking, "thought"))).toMatchObject({ streaming: true, startedAt: 1600 });
    expect(thinking.activity).toBe("thinking");
  });

  it("explores in one run: two reads and a search with three results", () => {
    const model = at(10);
    const run = only(itemsOf(model, "tools"));
    expect(run).toMatchObject({
      family: "explore",
      summary: "Read 2 files, searched 1 pattern",
      status: "completed",
    });
    expect(run.calls.map((call) => call.label)).toEqual([
      "Read pacing.ts",
      "Read pacing.test.ts",
      'Searched for "performance.now"',
    ]);
    expect(run.calls[2]?.search?.matches).toHaveLength(3);
    expect(only(itemsOf(model, "thought")).durationMs).toBe(6000);
  });

  it("runs the tests under the plan at 24 s", () => {
    const model = at(24);
    expect(model.activity).toBe("tool");
    expect(model.plan?.entries.map((entry) => entry.status)).toEqual([
      "completed",
      "completed",
      "in_progress",
    ]);
    const runs = itemsOf(model, "tools");
    expect(runs.map((run) => run.label)).toEqual([
      "Read 2 files, searched 1 pattern",
      "Edited 2 files",
      "Running pnpm --filter @ghost/ui test pacing",
    ]);
    expect(runs[1]).toMatchObject({ added: 59, removed: 23 });
    expect(runs[2]?.calls[0]?.terminal).toMatchObject({ running: true, exitCode: null });
  });

  it("ends done: 48 s, usage, $0.21, two files changed, and a markdown answer", () => {
    const model = reduceTranscript(GOLDEN.events);
    const turn = turnOf(model);
    expect(turn).toMatchObject({ id: GOLDEN_TURN_ID, status: "completed" });
    expect(model.activity).toBe("done");
    expect(model.plan).toMatchObject({ done: true, completed: 3, total: 3, current: null });
    expect(turn.footer).toMatchObject({
      durationMs: 48_000,
      stopReason: "end_turn",
      usage: { input_tokens: 4812, output_tokens: 2147, cached_input_tokens: 96_384 },
      cost: { amount: 0.21, currency: "USD" },
      model: "claude-opus-5-5",
      effort: "high",
    });
    expect(turn.footer?.files.map((file) => file.path)).toEqual([
      "packages/ui/src/orb/pacing.ts",
      "packages/ui/src/orb/pacing.test.ts",
    ]);
    const answer = itemsOf(model, "message").at(-1);
    expect(answer?.text).toBe(GOLDEN_ANSWER);
    const kinds = parseMarkdown(answer?.text ?? "").map((entry) => entry.block.kind);
    expect(kinds.filter((kind) => kind === "list")).toHaveLength(1);
    expect(kinds.filter((kind) => kind === "code")).toHaveLength(1);
    const list = parseMarkdown(answer?.text ?? "").find((entry) => entry.block.kind === "list");
    expect(list?.block.kind === "list" ? list.block.items : []).toHaveLength(3);
    const exec = itemsOf(model, "tools").at(-1)?.calls[0];
    expect(exec?.terminal).toMatchObject({ exitCode: 0, running: false });
  });
});

describe("the approval turn", () => {
  it("ends waiting on Codex's permission card", () => {
    const model = reduceTranscript(APPROVAL.events);
    expect(model.activity).toBe("waiting");
    const permission = only(model.pendingPermissions);
    expect(permission).toMatchObject({
      requestId: APPROVAL_REQUEST_ID,
      title: "Run cargo test -p aura-drivers?",
    });
    expect(permission.options.map((option) => [option.kind, option.label])).toEqual([
      ["allow_once", "Allow once"],
      ["allow_always", "Always allow"],
      ["reject_once", "Reject"],
    ]);
    expect(permission.tool).toMatchObject({
      kind: "execute",
      status: "pending",
      target: "cargo test -p aura-drivers",
    });
    const runs = itemsOf(model, "tools") as ToolRunItem[];
    expect(runs.map((run) => run.label)).toEqual([
      "Read 2 files",
      "Edited 2 files",
      "Waiting to run cargo test -p aura-drivers",
    ]);
  });
});
