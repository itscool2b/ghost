import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { GhostEvent } from "../../bindings";
import { newestTurnKey } from "../../lib/chat/tasks";
import { ev, numbered } from "../../lib/chat/testing";
import {
  applyEntries,
  emptyTranscript,
  type ToolItem,
  type Transcript,
} from "../../lib/chat/transcript";
import { BackgroundTasks } from "./BackgroundTasks";
import { StopButton } from "./Composer";
import { TaskPanel } from "./TaskPanel";
import { ToolBlock } from "./ToolBlock";
import { TaskLinkContext } from "./taskLinks";

function transcript(events: readonly GhostEvent[]): Transcript {
  return applyEntries(emptyTranscript(), numbered(events.map((event) => ["t", event] as const)));
}

const strip = (model: Transcript, live: readonly string[]) =>
  renderToStaticMarkup(
    <BackgroundTasks
      tasks={model.tasks}
      turnKey={newestTurnKey(model.segments)}
      live
      liveTasks={new Set(live)}
      openTaskId={null}
      onOpen={() => {}}
    />,
  );

describe("the background strip", () => {
  it("shows a pill per task with its kind, title, facts and an accessible name", () => {
    const now = Date.now();
    const model = transcript([
      ev.turnStarted("t"),
      ev.task("a", {
        title: "Map account switching code",
        agent_type: "Explore",
        activity: "Reading a.rs",
        tool_uses: 7,
        started_at: now - 72_000,
      }),
      ev.task("b", { kind: "command", title: "Run the tests", started_at: now - 5_000 }),
      ev.task("c", { title: "Audit", status: "completed" }),
    ]);
    const html = strip(model, ["a", "b", "c"]);
    expect(html.match(/data-testid="task-pill"/g)).toHaveLength(3);
    expect(html).toContain("Map account switching code");
    expect(html).toContain("1m 12s · 7 tools");
    // The words are the name, in the order they show; the type and status follow, unseen.
    expect(html).not.toMatch(/<button[^>]*aria-label/);
    expect(html).toContain('<span class="sr-only">Explore, Running</span>');
    // An ended pill says only that it ended.
    expect(html).toMatch(/Audit<\/span> <span class="sr-only">Done<\/span>/);
    expect(html).toContain('role="status"');
  });

  it("collapses more than three into one pill that lists them", () => {
    const model = transcript([
      ev.turnStarted("t"),
      ...["a", "b", "c", "d"].map((id) => ev.task(id, { title: `Task ${id}` })),
    ]);
    const html = strip(model, ["a", "b", "c", "d"]);
    expect(html).not.toContain('data-testid="task-pill"');
    expect(html).toContain("4 background tasks");
    expect(html).toContain('aria-label="4 background tasks 4 running"');
  });

  it("shows nothing but its live region without tasks", () => {
    const html = strip(transcript([ev.turnStarted("t")]), []);
    expect(html).not.toContain("task-strip");
    expect(html).toContain('data-testid="task-announcement"');
  });

  it("keeps earlier turns' tasks in one pill, so the next message never hides them", () => {
    const model = transcript([
      ev.turnStarted("t1"),
      ev.task("a", { title: "Explore dictation" }),
      ev.task("a", { title: "Explore dictation", status: "stopped" }),
      ev.turnStarted("t2"),
      ev.task("b", { title: "Run the tests", kind: "command" }),
    ]);
    const html = strip(model, ["b"]);
    expect(html.match(/data-testid="task-pill"/g)).toHaveLength(1);
    expect(html).toContain("Run the tests");
    expect(html).toContain('data-testid="task-earlier"');
    expect(html).toContain('aria-label="1 earlier task"');
    expect(html).toContain("Explore dictation");
    // Only earlier tasks: the strip stays, with just that pill.
    const quiet = strip(
      transcript([
        ev.turnStarted("t1"),
        ev.task("a", { title: "Explore", status: "completed" }),
        ev.turnStarted("t2"),
      ]),
      [],
    );
    expect(quiet).toContain('data-testid="task-earlier"');
    expect(quiet).not.toContain('data-testid="task-pill"');
  });
});

describe("the stop button", () => {
  it("says that stopping also ends the running background tasks", () => {
    const html = (runningTasks: number) =>
      renderToStaticMarkup(
        <StopButton
          status="running"
          onStop={() => {}}
          onForceStop={() => {}}
          runningTasks={runningTasks}
        />,
      );
    expect(html(0)).toContain('title="Stop (Esc)"');
    expect(html(0)).not.toContain("aria-description");
    expect(html(3)).toContain('title="Stop (Esc). Also ends 3 background tasks."');
    expect(html(1)).toContain('aria-description="Also ends 1 background task."');
    expect(html(1)).toContain('aria-label="Stop"');
  });
});

describe("the task panel", () => {
  it("shows the facts, the prompt, the steps, the tasks it started and the report", () => {
    const model = transcript([
      ev.turnStarted("t"),
      ev.task("a", {
        title: "Map account switching code",
        agent_type: "Explore",
        status: "completed",
        tokens: 48_210,
        tool_uses: 4,
        duration_ms: 61_000,
        prompt: "Find where accounts switch.",
        summary: "## Findings\n\nIn `accounts.rs`.",
      }),
      ev.taskStep("a", "s1", { title: "Read accounts.rs" }),
      ev.taskStep("a", "s2", { title: "Search for switch", tool_kind: "search", status: "failed" }),
      ev.taskStep("a", "s3", { title: "Read b.rs", status: "in_progress" }),
      ev.task("child", { kind: "command", title: "Run the tests", parent_task_id: "a" }),
    ]);
    const item = model.tasks.get("a");
    if (!item) throw new Error("no task");
    const html = renderToStaticMarkup(
      <TaskPanel
        item={item}
        tasks={model.tasks}
        live
        liveTasks={new Set(["a", "child"])}
        blobs={new Map()}
        onLoadBlob={() => {}}
        onOpen={() => {}}
        onClose={() => {}}
      />,
    );
    expect(html).toContain("Explore · Done · 1 min 1 s · 48.2k tokens · 4 tools");
    expect(html).toContain("<summary>Prompt</summary>");
    expect(html.match(/data-testid="task-step"/g)).toHaveLength(3);
    expect(html).toContain('data-test-status="failed"');
    // A step its ended task left running shows as stopped.
    expect(html).toContain('data-test-status="stopped"');
    expect(html).toContain('aria-label="Tasks it started"');
    expect(html).toContain("Run the tests");
    expect(html).toContain('data-testid="task-summary"');
    expect(html).toContain("Findings");
    expect(html).toContain('aria-label="Close task"');
  });

  it("shows a command's line", () => {
    const model = transcript([ev.task("b", { kind: "command", title: "Run the tests" })]);
    const item = model.tasks.get("b");
    if (!item) throw new Error("no task");
    const html = renderToStaticMarkup(
      <TaskPanel
        item={item}
        tasks={model.tasks}
        command="pnpm test"
        live
        liveTasks={new Set(["b"])}
        blobs={new Map()}
        onLoadBlob={() => {}}
        onOpen={() => {}}
        onClose={() => {}}
      />,
    );
    expect(html).toContain('data-testid="task-command"');
    expect(html).toContain("pnpm test");
  });
});

describe("a call that started a task", () => {
  it("offers Open beside its head", () => {
    const tool: ToolItem = {
      kind: "tool",
      key: "tool:call-a",
      id: "call-a",
      toolKind: "other",
      title: "Map account switching code",
      status: "completed",
      locations: [],
      rawInput: null,
      toolName: "Agent",
      content: [],
      truncated: null,
    };
    const html = renderToStaticMarkup(
      <TaskLinkContext.Provider
        value={(id) => (id === "call-a" ? { title: "Map it", open: () => {} } : null)}
      >
        <ToolBlock tool={tool} display="completed" blobs={new Map()} onLoadBlob={() => {}} />
      </TaskLinkContext.Provider>,
    );
    expect(html).toContain('data-testid="tool-open-task"');
    expect(html).toContain('aria-label="Open task: Map it"');
  });
});
