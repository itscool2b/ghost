import type { AppInfo, SmokeReport, StressConfig } from "../bindings";
import { errorMessage } from "./errors";
import { FpsMeter, trackFrames } from "./fps";
import type { StreamSnapshot, StreamStore } from "./session";
import { lastMessage } from "./stream";
import { stressTurnTokens } from "./stress";

/**
 * The stream check of smoke mode (`GHOST_SMOKE=1`): a scripted check of the real shell, through
 * the real UI. The chat scenarios are in `smokeChat.ts`.
 *
 * 1. A full demo turn: every block arrives complete, in order, and the answer renders.
 * 2. A cancelled turn: stopping mid-stream ends the turn as "cancelled".
 * 3. A stress burst (6 orbs, 200 tokens/s, 3 s) with the frame rate measured while it streams.
 *
 * The report goes to the shell, which prints `GHOST_SMOKE_RESULT <json>` and may exit with it.
 */

export const SMOKE_STRESS: StressConfig = { orbs: 6, rate: 200, aura: true, seconds: 3 };

/** Fast enough to keep the run short, slow enough to span many frames. */
export const SMOKE_DEMO_RATE = 400;

const TURN_TIMEOUT_MS = 30_000;

/** Markdown syntax is not rendered, so less text reaches the DOM than streamed; never this little. */
const MIN_RENDERED_SHARE = 0.5;

export interface SmokeHooks {
  readonly store: StreamStore;
  /** Shows the normal screen; resolves once it has rendered. */
  showHome(): Promise<void>;
  /** Shows the stress view with `config` (without it starting a stream); resolves once rendered. */
  showStress(config: StressConfig): Promise<void>;
  /** The answer text currently in the DOM. */
  renderedAnswer(): string;
  /** Waits until pending updates are committed and painted. */
  settle(): Promise<void>;
  /** Feeds `meter` until the returned function is called. Defaults to animation frames. */
  readonly trackFrames?: (meter: FpsMeter) => () => void;
}

const turnOver = (snapshot: StreamSnapshot): boolean =>
  snapshot.phase === "ended" || snapshot.phase === "failed";

export async function runSmoke(info: AppInfo, hooks: SmokeHooks): Promise<SmokeReport> {
  const errors: string[] = [];
  let renderedChars = 0;
  let fpsMin = 0;
  let fpsAvg = 0;

  const checkRendered = (label: string, snapshot: StreamSnapshot): void => {
    const streamed = lastMessage(snapshot.stream)?.text.length ?? 0;
    const rendered = hooks.renderedAnswer().length;
    renderedChars += rendered;
    if (rendered === 0) errors.push(`the ${label} answer did not render`);
    else if (rendered < streamed * MIN_RENDERED_SHARE) {
      errors.push(
        `only ${rendered} of ${streamed} streamed characters of the ${label} answer rendered`,
      );
    }
  };

  if (info.fatal !== null) {
    errors.push(`startup failed: ${info.fatal}`);
  } else {
    try {
      const { store } = hooks;

      await hooks.showHome();
      await store.start({ type: "demo" }, SMOKE_DEMO_RATE);
      const demo = await store.waitFor(turnOver, TURN_TIMEOUT_MS, "the demo turn to end");
      errors.push(...demoProblems(demo));
      await hooks.settle();
      checkRendered("demo", demo);

      await store.start({ type: "demo" }, 40);
      await store.waitFor((s) => s.stream.events > 0, TURN_TIMEOUT_MS, "the first frame");
      await store.stop();
      const cancelled = await store.waitFor(turnOver, TURN_TIMEOUT_MS, "the stopped turn to end");
      const reason = cancelled.stream.turn?.stopReason ?? "no turn_ended";
      if (reason !== "cancelled") errors.push(`stopping ended the turn with ${reason}`);

      await hooks.showStress(SMOKE_STRESS);
      const meter = new FpsMeter();
      const stopFrames = (hooks.trackFrames ?? trackFrames)(meter);
      let stress: StreamSnapshot;
      try {
        await store.start(
          { type: "stress", total_tokens: stressTurnTokens(SMOKE_STRESS) },
          SMOKE_STRESS.rate,
        );
        stress = await store.waitFor(turnOver, TURN_TIMEOUT_MS, "the stress turn to end");
      } finally {
        stopFrames();
      }
      const summary = meter.summary();
      fpsMin = summary.min;
      fpsAvg = summary.avg;
      if (summary.frames === 0) {
        // Not a failure: WebKit pauses animation frames while the window is hidden or covered
        // (a locked screen included), so there is no rate to measure.
        console.warn("Ghost smoke: no animation frames ran during the stress burst");
      }
      const stressReason = stress.stream.turn?.stopReason ?? "no turn_ended";
      if (stressReason !== "end_turn") errors.push(`the stress turn ended with ${stressReason}`);
      await hooks.settle();
      checkRendered("stress", stress);
      await hooks.showHome();
    } catch (error) {
      errors.push(errorMessage(error));
    }
  }

  const totals = hooks.store.totals;
  if (totals.duplicates > 0) errors.push(`${totals.duplicates} events arrived twice`);
  return {
    ok: errors.length === 0 && totals.gaps === 0,
    scenario: "stream",
    stream: {
      events_received: totals.events,
      frames_received: totals.frames,
      seq_gaps: totals.gaps,
      rendered_chars: renderedChars,
      fps_min: Math.round(fpsMin * 10) / 10,
      fps_avg: Math.round(fpsAvg * 10) / 10,
    },
    checks: [],
    errors,
  };
}

/** What a complete demo turn must contain. */
export function demoProblems(snapshot: StreamSnapshot): string[] {
  const problems: string[] = [];
  const { stream } = snapshot;
  if (snapshot.phase === "failed") problems.push(`the demo turn failed: ${snapshot.error}`);
  const reason = stream.turn?.stopReason ?? "no turn_ended";
  if (reason !== "end_turn") problems.push(`the demo turn ended with ${reason}`);
  const answer = lastMessage(stream);
  if (!answer?.completed || answer.text.length === 0) {
    problems.push("the demo answer is missing or incomplete");
  }
  const thoughts = [...stream.thoughts.values()];
  if (thoughts.length !== 1 || !thoughts[0]?.completed) {
    problems.push("the demo thought is missing or incomplete");
  }
  const tools = [...stream.tools.values()];
  const done = tools.filter((tool) => tool.status === "completed").length;
  if (tools.length !== 2 || done !== 2) {
    problems.push(`expected 2 completed tool calls, got ${done} of ${tools.length}`);
  }
  if (stream.plan.length !== 3) problems.push(`expected a 3-step plan, got ${stream.plan.length}`);
  if (stream.usage === null) problems.push("the demo turn reported no usage");
  return problems;
}
