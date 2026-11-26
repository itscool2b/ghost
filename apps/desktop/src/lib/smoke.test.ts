import { describe, expect, it, vi } from "vitest";
import type { AppInfo, DemoKind, EventBatch, GhostEvent } from "../bindings";
import type { FpsMeter } from "./fps";
import { StreamStore, type StreamTransport } from "./session";
import { demoProblems, runSmoke, type SmokeHooks } from "./smoke";
import { applyBatch, initialStreamState } from "./stream";

const INFO: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: "macos",
  os_version: "26.6.2",
  arch: "aarch64",
  data_dir: "/data",
  device: null,
  profile: null,
  profile_switching: false,
  db: null,
  glass: "liquid_glass",
  launch: { type: "smoke", plan: { scenario: "stream", agent_id: null, folder: null } },
  notices: [],
  recovery_notice: null,
  fatal: null,
  media: true,
  media_notice: null,
};

function demoEvents(): GhostEvent[] {
  const tool = (id: string, kind: "read" | "edit"): GhostEvent[] => [
    {
      type: "tool_call",
      tool_call_id: id,
      kind,
      title: id,
      status: "in_progress",
      locations: [],
      raw_input: null,
      tool_name: null,
      content: [],
      truncated: null,
    },
    {
      type: "tool_update",
      tool_call_id: id,
      status: "completed",
      title: null,
      kind: null,
      locations: null,
      raw_input: null,
      tool_name: null,
      content: null,
      truncated: null,
    },
  ];
  return [
    { type: "turn_started", turn_id: "turn-1", prompt_id: null },
    { type: "thought_delta", thought_id: "t", text: "Hmm." },
    { type: "thought_completed", thought_id: "t", duration_ms: 100 },
    ...tool("read", "read"),
    ...tool("edit", "edit"),
    {
      type: "plan_updated",
      entries: ["a", "b", "c"].map((content) => ({
        content,
        status: "completed" as const,
        priority: null,
      })),
    },
    { type: "message_delta", message_id: "m", text: "## Done\n\nHello, world!" },
    { type: "message_completed", message_id: "m" },
    { type: "usage_updated", context_used: 1, context_size: 2, cost: null },
    {
      type: "turn_ended",
      turn_id: "turn-1",
      stop_reason: "end_turn",
      usage: null,
      duration_ms: null,
      recovered: false,
    },
  ];
}

/** Plays scripted turns over a fake channel, one event per frame, asynchronously like IPC. */
class ScriptedShell implements StreamTransport {
  #sessions = 0;
  readonly #open = new Map<string, (events: GhostEvent[]) => void>();
  gapAt: number | null = null;

  start(kind: DemoKind, rate: number, onBatch: (batch: EventBatch) => void): Promise<string> {
    this.#sessions += 1;
    const id = `s${this.#sessions}`;
    let seq = 0;
    const send = (events: GhostEvent[]): void => {
      if (this.gapAt !== null && seq === this.gapAt) seq += 1;
      onBatch({ chat_id: id, seq_start: seq, events });
      seq += events.length;
    };
    this.#open.set(id, send);
    const script =
      kind.type === "demo" && rate < 100
        ? // A slow turn that only ends when cancelled.
          [{ type: "turn_started", turn_id: "turn-1", prompt_id: null } as GhostEvent]
        : kind.type === "demo"
          ? demoEvents()
          : [
              { type: "turn_started", turn_id: "turn-1", prompt_id: null } as GhostEvent,
              { type: "message_delta", message_id: "m", text: "x ".repeat(300) } as GhostEvent,
              {
                type: "turn_ended",
                turn_id: "turn-1",
                stop_reason: "end_turn",
                usage: null,
                duration_ms: null,
                recovered: false,
              } as GhostEvent,
            ];
    setTimeout(() => {
      for (const event of script) send([event]);
    }, 0);
    return Promise.resolve(id);
  }

  cancel(sessionId: string): Promise<void> {
    const send = this.#open.get(sessionId);
    this.#open.delete(sessionId);
    setTimeout(
      () =>
        send?.([
          {
            type: "turn_ended",
            turn_id: "turn-1",
            stop_reason: "cancelled",
            usage: null,
            duration_ms: null,
            recovered: false,
          },
        ]),
      0,
    );
    return Promise.resolve();
  }
}

function hooksFor(store: StreamStore, rendered?: () => string): SmokeHooks {
  return {
    store,
    showHome: () => Promise.resolve(),
    showStress: () => Promise.resolve(),
    renderedAnswer: rendered ?? (() => store.getSnapshot().stream.messages.get("m")?.text ?? ""),
    settle: () => Promise.resolve(),
    trackFrames: (meter: FpsMeter) => {
      for (let frame = 0; frame <= 60; frame += 1) meter.tick(frame * (1000 / 60));
      return () => undefined;
    },
  };
}

describe("runSmoke", () => {
  it("passes a healthy run and reports what it measured", async () => {
    const store = new StreamStore(new ScriptedShell());
    const report = await runSmoke(INFO, hooksFor(store));
    expect(report.errors).toEqual([]);
    expect(report.ok).toBe(true);
    expect(report.scenario).toBe("stream");
    expect(report.checks).toEqual([]);
    const stream = report.stream;
    expect(stream?.seq_gaps).toBe(0);
    expect(stream?.events_received).toBeGreaterThan(10);
    expect(stream?.frames_received).toBe(stream?.events_received);
    expect(stream?.rendered_chars).toBeGreaterThan(0);
    expect(stream?.fps_avg).toBeCloseTo(60, 0);
  });

  it("fails when startup failed, without streaming", async () => {
    const shell = new ScriptedShell();
    const start = vi.spyOn(shell, "start");
    const report = await runSmoke(
      { ...INFO, fatal: "Could not access /data: Permission denied" },
      hooksFor(new StreamStore(shell)),
    );
    expect(report.ok).toBe(false);
    expect(report.errors).toEqual(["startup failed: Could not access /data: Permission denied"]);
    expect(start).not.toHaveBeenCalled();
  });

  it("fails on sequence gaps", async () => {
    const shell = new ScriptedShell();
    shell.gapAt = 3;
    const report = await runSmoke(INFO, hooksFor(new StreamStore(shell)));
    expect(report.ok).toBe(false);
    expect(report.stream?.seq_gaps).toBeGreaterThan(0);
  });

  it("fails when the answer does not reach the DOM", async () => {
    const report = await runSmoke(
      INFO,
      hooksFor(new StreamStore(new ScriptedShell()), () => ""),
    );
    expect(report.ok).toBe(false);
    expect(report.errors).toContain("the demo answer did not render");
  });
});

describe("demoProblems", () => {
  it("accepts a complete demo turn", () => {
    const stream = applyBatch(initialStreamState(), {
      chat_id: "s",
      seq_start: 0,
      events: demoEvents(),
    });
    expect(
      demoProblems({
        phase: "ended",
        kind: { type: "demo" },
        tokensPerSec: 400,
        sessionId: "s",
        stream,
        error: null,
        startedAt: 0,
        endedAt: 1,
      }),
    ).toEqual([]);
  });

  it("lists everything a truncated turn is missing", () => {
    const stream = applyBatch(initialStreamState(), {
      chat_id: "s",
      seq_start: 0,
      events: [
        { type: "turn_started", turn_id: "turn-1", prompt_id: null },
        {
          type: "turn_ended",
          turn_id: "turn-1",
          stop_reason: "cancelled",
          usage: null,
          duration_ms: null,
          recovered: false,
        },
      ],
    });
    const problems = demoProblems({
      phase: "ended",
      kind: { type: "demo" },
      tokensPerSec: 400,
      sessionId: "s",
      stream,
      error: null,
      startedAt: 0,
      endedAt: 1,
    });
    expect(problems).toEqual([
      "the demo turn ended with cancelled",
      "the demo answer is missing or incomplete",
      "the demo thought is missing or incomplete",
      "expected 2 completed tool calls, got 0 of 0",
      "expected a 3-step plan, got 0",
      "the demo turn reported no usage",
    ]);
  });
});
