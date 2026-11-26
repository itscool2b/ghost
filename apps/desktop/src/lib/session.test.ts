import { describe, expect, it } from "vitest";
import type { DemoKind, EventBatch, GhostEvent } from "../bindings";
import { isRunning, StreamStore, type StreamTransport } from "./session";

/** A shell stand-in: records calls and lets the test push frames into the latest channel. */
class FakeShell implements StreamTransport {
  readonly cancelled: string[] = [];
  readonly sinks: ((batch: EventBatch) => void)[] = [];
  readonly sessions: string[] = [];
  #pending: { resolve: (id: string) => void; reject: (error: Error) => void } | null = null;
  holdNextStart = false;
  failNextStart: string | null = null;

  start(_kind: DemoKind, _rate: number, onBatch: (batch: EventBatch) => void): Promise<string> {
    const id = `s${this.sessions.length + 1}`;
    this.sessions.push(id);
    this.sinks.push(onBatch);
    if (this.failNextStart !== null) {
      const message = this.failNextStart;
      this.failNextStart = null;
      return Promise.reject(new Error(message));
    }
    if (this.holdNextStart) {
      this.holdNextStart = false;
      return new Promise((resolve, reject) => {
        this.#pending = { resolve: () => resolve(id), reject };
      });
    }
    return Promise.resolve(id);
  }

  release(): void {
    this.#pending?.resolve("");
    this.#pending = null;
  }

  cancel(sessionId: string): Promise<void> {
    this.cancelled.push(sessionId);
    return Promise.resolve();
  }

  emit(index: number, seqStart: number, events: GhostEvent[]): void {
    this.sinks[index]?.({ chat_id: this.sessions[index] ?? "", seq_start: seqStart, events });
  }
}

const started: GhostEvent = { type: "turn_started", turn_id: "turn-1", prompt_id: null };
const ended = (reason: "end_turn" | "cancelled"): GhostEvent => ({
  type: "turn_ended",
  turn_id: "turn-1",
  stop_reason: reason,
  usage: null,
  duration_ms: null,
  recovered: false,
});
const delta = (text: string): GhostEvent => ({ type: "message_delta", message_id: "m", text });

describe("StreamStore", () => {
  it("runs a stream from starting to ended and counts what arrived", async () => {
    const shell = new FakeShell();
    let clock = 100;
    const store = new StreamStore(shell, () => clock);
    const phases: string[] = [];
    store.subscribe(() => phases.push(store.getSnapshot().phase));

    const id = await store.start({ type: "demo" }, 120);
    expect(id).toBe("s1");
    expect(store.getSnapshot().phase).toBe("streaming");
    expect(isRunning(store.getSnapshot())).toBe(true);

    shell.emit(0, 0, [started, delta("Hel")]);
    clock = 250;
    shell.emit(0, 2, [delta("lo"), ended("end_turn")]);

    const snapshot = store.getSnapshot();
    expect(snapshot.phase).toBe("ended");
    expect(snapshot.stream.messages.get("m")?.text).toBe("Hello");
    expect(snapshot.startedAt).toBe(100);
    expect(snapshot.endedAt).toBe(250);
    expect(store.totals).toEqual({
      runs: 1,
      frames: 2,
      events: 4,
      gaps: 0,
      missed: 0,
      duplicates: 0,
    });
    expect(phases[0]).toBe("starting");
    expect(phases.at(-1)).toBe("ended");
  });

  it("accepts frames that arrive before the shell confirms the start", async () => {
    const shell = new FakeShell();
    shell.holdNextStart = true;
    const store = new StreamStore(shell);
    const starting = store.start({ type: "demo" }, 100);
    shell.emit(0, 0, [started]);
    expect(store.getSnapshot().phase).toBe("streaming");
    shell.release();
    await starting;
    expect(store.getSnapshot().sessionId).toBe("s1");
  });

  it("ignores late frames from a replaced run and cancels the old stream", async () => {
    const shell = new FakeShell();
    const store = new StreamStore(shell);
    await store.start({ type: "demo" }, 100);
    shell.emit(0, 0, [started, delta("old")]);

    await store.start({ type: "stress", total_tokens: 600 }, 200);
    expect(shell.cancelled).toEqual(["s1"]);

    // The first stream's final frame arrives after the switch.
    shell.emit(0, 2, [delta(" tail"), ended("cancelled")]);
    shell.emit(1, 0, [started, delta("new")]);

    const snapshot = store.getSnapshot();
    expect(snapshot.sessionId).toBe("s2");
    expect(snapshot.stream.messages.get("m")?.text).toBe("new");
    expect(snapshot.phase).toBe("streaming");
    expect(store.totals.runs).toBe(2);
  });

  it("cancels a stream stopped before its session id arrived", async () => {
    const shell = new FakeShell();
    shell.holdNextStart = true;
    const store = new StreamStore(shell);
    const starting = store.start({ type: "demo" }, 100);
    await store.stop();
    expect(shell.cancelled).toEqual([]);
    shell.release();
    await starting;
    expect(shell.cancelled).toEqual(["s1"]);
  });

  it("stop is a no-op when nothing runs, and stops a running stream", async () => {
    const shell = new FakeShell();
    const store = new StreamStore(shell);
    await store.stop();
    expect(shell.cancelled).toEqual([]);
    await store.start({ type: "demo" }, 100);
    await store.stop();
    expect(shell.cancelled).toEqual(["s1"]);
    shell.emit(0, 0, [started, ended("cancelled")]);
    expect(store.getSnapshot().stream.turn?.stopReason).toBe("cancelled");
    await store.stop();
    expect(shell.cancelled).toEqual(["s1"]);
  });

  it("reports a refused start as failed", async () => {
    const shell = new FakeShell();
    shell.failNextStart = "8 demo streams are already running. Stop one first.";
    const store = new StreamStore(shell);
    await expect(store.start({ type: "demo" }, 100)).rejects.toThrow("already running");
    expect(store.getSnapshot().phase).toBe("failed");
    expect(store.getSnapshot().error).toContain("already running");
  });

  it("waitFor resolves on a matching snapshot and times out otherwise", async () => {
    const shell = new FakeShell();
    const store = new StreamStore(shell);
    await store.start({ type: "demo" }, 100);
    const done = store.waitFor((s) => s.phase === "ended", 1000);
    shell.emit(0, 0, [started, ended("end_turn")]);
    await expect(done).resolves.toMatchObject({ phase: "ended" });
    await expect(store.waitFor((s) => s.phase === "failed", 10, "a failure")).rejects.toThrow(
      "timed out after 10 ms waiting for a failure",
    );
  });

  it("reset drops the current run", async () => {
    const shell = new FakeShell();
    const store = new StreamStore(shell);
    await store.start({ type: "demo" }, 100);
    store.reset();
    shell.emit(0, 0, [started, delta("late")]);
    expect(store.getSnapshot().phase).toBe("idle");
    expect(store.getSnapshot().stream.events).toBe(0);
  });

  it("abandon resets at once and stops the old stream in the shell", async () => {
    const shell = new FakeShell();
    const store = new StreamStore(shell);
    await store.start({ type: "demo" }, 100);
    shell.emit(0, 0, [started]);

    const abandoning = store.abandon();
    // Synchronous: a stream started now cannot be confused with the abandoned one.
    expect(store.getSnapshot().phase).toBe("idle");
    const next = store.start({ type: "stress", total_tokens: 600 }, 200);
    await abandoning;
    await next;
    expect(shell.cancelled).toEqual(["s1"]);

    shell.emit(0, 1, [delta("old")]);
    shell.emit(1, 0, [started, delta("new")]);
    expect(store.getSnapshot().sessionId).toBe("s2");
    expect(store.getSnapshot().stream.messages.get("m")?.text).toBe("new");
  });

  it("abandon cancels a stream still waiting for its session id once the id arrives", async () => {
    const shell = new FakeShell();
    shell.holdNextStart = true;
    const store = new StreamStore(shell);
    const starting = store.start({ type: "demo" }, 100);
    await store.abandon();
    expect(shell.cancelled).toEqual([]);
    shell.release();
    await starting;
    expect(shell.cancelled).toEqual(["s1"]);
    expect(store.getSnapshot().phase).toBe("idle");
  });

  it("abandon does nothing in the shell when nothing runs", async () => {
    const shell = new FakeShell();
    const store = new StreamStore(shell);
    await store.abandon();
    expect(shell.cancelled).toEqual([]);
    expect(store.getSnapshot().phase).toBe("idle");
  });
});
