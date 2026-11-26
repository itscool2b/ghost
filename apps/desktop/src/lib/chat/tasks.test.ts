import { describe, expect, it } from "vitest";
import type { GhostEvent } from "../../bindings";
import { ChatStore } from "./chatStore";
import {
  childTasks,
  compactElapsed,
  endedAnnouncement,
  newestTurnKey,
  repeatsSummary,
  stripTasks,
  type TaskItem,
  taskDisplay,
  taskElapsed,
  tasksByToolCall,
} from "./tasks";
import {
  attachResult,
  backgroundTask,
  compactHistory,
  ev,
  FakeTransport,
  frame,
  historyPage,
  items,
  numbered,
  settled,
} from "./testing";
import { applyEntries, emptyTranscript, type Transcript, withTasks } from "./transcript";

const CHAT = "c1";

function transcript(events: readonly (readonly [string | null, GhostEvent])[]): Transcript {
  return applyEntries(emptyTranscript(), numbered(events));
}

function task(model: Transcript, id: string): TaskItem {
  const item = model.tasks.get(id);
  if (!item) throw new Error(`no task ${id}`);
  return item;
}

describe("the transcript's tasks", () => {
  it("keep the prompt and summary each record carried once, and steps in arrival order", () => {
    const model = transcript([
      ["t", ev.turnStarted("t")],
      ["t", ev.task("a", { prompt: "Map it", tool_call_id: "call-a" })],
      ["t", ev.taskStep("a", "s1", { status: "in_progress" })],
      ["t", ev.taskStep("a", "s2", { kind: "text", text: "Looking.", title: "" })],
      ["t", ev.task("a", { activity: "Reading a.rs", tool_uses: 2 })],
      ["t", ev.taskStep("a", "s1", { status: "completed" })],
      [null, ev.task("a", { status: "completed", summary: "Found it" })],
    ]);
    const item = task(model, "a");
    expect(item.task.prompt).toBe("Map it");
    expect(item.task.summary).toBe("Found it");
    expect(item.task.tool_uses).toBeNull();
    expect(item.steps.map((step) => [step.step_id, step.status])).toEqual([
      ["s1", "completed"],
      ["s2", "completed"],
    ]);
    expect(item.endedIn).toBe("turn:t");
    expect(tasksByToolCall(model.tasks).get("call-a")).toBe("a");
  });

  it("hold steps for a task whose record is in an older page, without showing it", () => {
    const model = transcript([[null, ev.taskStep("old", "s1")]]);
    expect(task(model, "old").placeholder).toBe(true);
    expect(stripTasks(model.tasks, null, true, new Set(["old"]))).toEqual([]);
    const known = applyEntries(model, [{ seq: 9, turnId: null, event: ev.task("old") }]);
    expect(task(known, "old").placeholder).toBe(false);
    expect(task(known, "old").steps).toHaveLength(1);
  });

  it("render the same from compacted history as from the live stream", () => {
    const live = numbered([
      ["t", ev.turnStarted("t")],
      ["t", ev.task("a", { prompt: "Map it" })],
      ["t", ev.taskStep("a", "s1", { status: "in_progress" })],
      ["t", ev.task("a", { activity: "Reading a.rs" })],
      ["t", ev.taskStep("a", "s1", { status: "completed" })],
      ["t", ev.task("a", { status: "completed", summary: "Found it" })],
      ["t", ev.task("a", { status: "completed", activity: "Done" })],
      ["t", ev.turnEnded("t")],
    ]);
    const fromLive = applyEntries(emptyTranscript(), live).tasks;
    const fromHistory = applyEntries(emptyTranscript(), compactHistory(live)).tasks;
    expect(fromHistory.get("a")?.task).toEqual(fromLive.get("a")?.task);
    expect(fromHistory.get("a")?.steps).toEqual(fromLive.get("a")?.steps);
  });
});

describe("what the strip lists", () => {
  const model = transcript([
    ["old", ev.turnStarted("old")],
    ["old", ev.task("earlier", { status: "completed" })],
    ["old", ev.turnEnded("old")],
    ["t", ev.turnStarted("t")],
    ["t", ev.task("run")],
    ["t", ev.task("child", { parent_task_id: "run" })],
    ["t", ev.task("done", { status: "failed" })],
    ["t", ev.task("stale")],
  ]);
  const turnKey = newestTurnKey(model.segments);

  it("is the running tasks no task started, and those that ended in the newest turn", () => {
    const live = new Set(["run", "child", "done"]);
    const ids = stripTasks(model.tasks, turnKey, true, live).map((item) => item.task.task_id);
    expect(ids).toEqual(["run", "done"]);
    expect(childTasks(model.tasks, "run").map((item) => item.task.task_id)).toEqual(["child"]);
  });

  it("shows a running task the live session does not know, or any without a process, stopped", () => {
    const stale = task(model, "stale");
    expect(taskDisplay(stale, true, new Set())).toBe("stopped");
    expect(taskDisplay(task(model, "run"), false, new Set(["run"]))).toBe("stopped");
    expect(taskDisplay(task(model, "run"), true, new Set(["run"]))).toBe("running");
  });
});

describe("a task's words", () => {
  it("counts its time from its start while it runs, then from what it reported", () => {
    const running = backgroundTask("a", { started_at: 1_000, duration_ms: 500 });
    expect(taskElapsed(running, "running", 61_000)).toBe(60_000);
    const done = backgroundTask("a", { started_at: 1_000, ended_at: 9_000 });
    expect(taskElapsed(done, "completed", 99_000)).toBe(8_000);
    expect(compactElapsed(73_000)).toBe("1m 13s");
    expect(compactElapsed(3_900_000)).toBe("1h 5m");
  });

  it("announces how it ended, and folds a last text step that repeats the report", () => {
    expect(endedAnnouncement(backgroundTask("a", { title: "Map it" }), "failed")).toBe(
      "Map it failed",
    );
    expect(endedAnnouncement(backgroundTask("a"), "running")).toBeNull();
    const step = {
      step_id: "s",
      kind: "text" as const,
      title: "",
      tool_kind: null,
      status: null,
      text: "## Found it",
      truncated: null,
    };
    expect(repeatsSummary(step, "## Found it\n\nIn a.rs.")).toBe(true);
    expect(repeatsSummary(step, "Something else")).toBe(false);
  });
});

describe("the chat store's tasks", () => {
  it("lays the snapshot's tasks between history and later frames, and knows which are live", async () => {
    const fake = new FakeTransport();
    const store = new ChatStore(CHAT, fake);
    const opening = store.open();
    const page = historyPage(
      CHAT,
      items(
        numbered([
          ["t", ev.turnStarted("t")],
          ["t", ev.task("a", { prompt: "Map it" })],
          ["t", ev.task("gone")],
        ]),
      ),
    );
    const pending = fake.lastAttach();
    pending.resolve(
      attachResult(CHAT, 3, page, {
        tasks: [backgroundTask("a", { activity: "Reading a.rs", tool_uses: 3 })],
      }),
    );
    await opening;
    await settled();
    let state = store.getSnapshot();
    expect(task(state.transcript, "a").task.activity).toBe("Reading a.rs");
    expect(task(state.transcript, "a").task.prompt).toBe("Map it");
    expect([...state.liveTasks]).toEqual(["a"]);
    expect(taskDisplay(task(state.transcript, "gone"), true, state.liveTasks)).toBe("stopped");
    pending.onFrame(frame(CHAT, 3, [ev.task("b"), ev.task("a", { status: "completed" })]));
    state = store.getSnapshot();
    expect(state.liveTasks.has("b")).toBe(true);
    expect(task(state.transcript, "a").task.status).toBe("completed");
  });

  it("keeps a later frame's record over the snapshot when an older page is loaded", () => {
    const history = applyEntries(
      emptyTranscript(),
      numbered([
        ["t", ev.turnStarted("t")],
        ["t", ev.task("a")],
      ]),
    );
    const snapshot = withTasks(history, [backgroundTask("a", { tool_uses: 1 })]);
    const later = applyEntries(snapshot, [
      { seq: 9, turnId: null, event: ev.task("a", { status: "completed" }) },
    ]);
    expect(task(later, "a").task.status).toBe("completed");
  });
});
