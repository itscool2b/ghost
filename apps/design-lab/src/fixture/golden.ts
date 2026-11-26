import { PACING_AFTER, PACING_BEFORE, PACING_TEST_AFTER, PACING_TEST_BEFORE } from "./golden-files";
import { TimelineBuilder } from "./timeline";
import type { ChatRecording, LabEvent } from "./types";
import { CHAT_PACING } from "./workspace";

/**
 * The golden turn every direction renders: Claude Code (Opus 5.5, high effort, Full auto) finds and
 * fixes a flaky test in this repo. About 48 s at 1x: a 6 s thought, two reads and a search, an
 * explanation, a plan, two edits, a test run with coloured output, and a markdown answer, then
 * usage, cost, and two files changed. The technical story is real: "after" is the code in
 * packages/ui/src/orb/pacing.ts today.
 *
 * Moments worth a screenshot: t=7.5 s the thought is finishing; t=24 s the tests are running under
 * the plan; t=48.3 s the turn ends.
 */

export const GOLDEN_PROMPT =
  "The orb pacing test fails about 1 in 20 runs on CI. Can you find out why and fix it?";

export const GOLDEN_TURN_ID = "turn-orb-pacing";

const PACING = "packages/ui/src/orb/pacing.ts";
const PACING_TEST = "packages/ui/src/orb/pacing.test.ts";
const TEST_COMMAND = "pnpm --filter @ghost/ui test pacing";

/** The files the golden turn edits, in the order it first edits them. */
export const GOLDEN_EDITED_FILES: readonly string[] = [PACING, PACING_TEST];

export const GOLDEN_THOUGHT = `The test only fails on CI, about 1 run in 20, so something in it depends on timing. Let me read pacing.ts and its test. If the test drives shouldDrawFrame from a real clock, timer jitter on a busy runner could make a tick land just before the 16.7 ms interval, and that frame would be skipped. I should check whether shouldDrawFrame tolerates early frames at all, and whether anything in the test reads performance.now() directly.`;

export const GOLDEN_CAUSE = `Found it. The flaky case is "draws a 60 fps orb on every tick" in \`pacing.test.ts\`. It sleeps with \`setTimeout\` and reads \`performance.now()\`, so it assumes each frame arrives exactly 16.7 ms after the last one. Node only fires timers on whole milliseconds, and a shared CI runner adds jitter on top, so now and then a tick lands at 16.4 ms. \`shouldDrawFrame\` demands the full interval, skips that frame, and the test counts 59 instead of 60.

The fix has two parts: make pacing tolerate a little jitter (real displays jitter too), and drive the test from a simulated clock instead of the runner's.`;

export const GOLDEN_ANSWER = `Fixed. The flake was in the test, not the renderer. It slept with real timers and read \`performance.now()\`, so it assumed every frame arrived exactly 16.7 ms after the last one. On a busy CI runner a timer can fire a fraction of a millisecond early; \`shouldDrawFrame\` then refused the frame and the test counted 59 instead of 60.

- **Jitter tolerance.** \`shouldDrawFrame\` now accepts a frame up to 2 ms early (a quarter of the interval at most), so ordinary display jitter never drops a beat.
- **Fixed grid.** \`nextFrameTime\` advances by whole intervals instead of jumping to \`now\`, so 24 fps still averages 24 on a 60 Hz display and one late frame no longer delays every frame after it.
- **Simulated clock.** The test drives a fake display through \`countFrames\`, and a new case replays up to ±0.9 ms of jitter at 60 Hz and 120 Hz. Nothing in the suite reads the wall clock any more.

The core of the change:

\`\`\`ts
function slackMs(intervalMs: number): number {
  return Math.min(2, intervalMs / 4);
}

export function shouldDrawFrame(now: number, lastDrawAt: number, fps: number): boolean {
  const interval = frameIntervalMs(fps);
  if (interval === Number.POSITIVE_INFINITY) return false;
  if (!Number.isFinite(lastDrawAt)) return true;
  const elapsed = now - lastDrawAt;
  if (elapsed < 0) return true;
  return elapsed >= interval - slackMs(interval);
}
\`\`\`

All 19 pacing tests pass. The renderer needed no change: it already stores \`nextFrameTime\` after every draw.`;

const SEARCH_RESULTS = [
  `${PACING_TEST}:24:    const start = performance.now();`,
  `${PACING_TEST}:28:      const now = performance.now() - start;`,
  "packages/ui/src/orb/renderer.ts:673:    now: () => performance.now(),",
].join("\n");

/* Terminal styling, as vitest prints it. */
const sgr = (code: string): string => `\u001b[${code}m`;
const bold = (text: string): string => `${sgr("1")}${text}${sgr("22")}`;
const dim = (text: string): string => `${sgr("2")}${text}${sgr("22")}`;
const green = (text: string): string => `${sgr("32")}${text}${sgr("39")}`;
const cyan = (text: string): string => `${sgr("36")}${text}${sgr("39")}`;
const gray = (text: string): string => `${sgr("90")}${text}${sgr("39")}`;
const runBadge = `${bold(`${sgr("46")}${sgr("30")} RUN ${sgr("39")}${sgr("49")}`)}`;

const TEST_NAMES: readonly (readonly [suite: string, name: string, ms?: number])[] = [
  ["frame pacing", "converts fps to an interval, with Infinity for never"],
  ["frame pacing", "always draws an orb that has never drawn"],
  ["frame pacing", "never draws at a non-positive rate"],
  ["frame pacing", "draws a 60 fps orb on every tick of a 60 Hz display, even with jitter", 2],
  ["frame pacing", "keeps every frame under CI timer jitter at 60 and 120 Hz", 3],
  ["frame pacing", "averages 24 fps on 60 Hz and 120 Hz displays", 1],
  ["frame pacing", "halves 60 fps work on a 120 Hz display"],
  ["frame pacing", "waits a full interval between frames"],
  ["frame pacing", "restarts after a stall instead of bursting to catch up"],
  ["frame pacing", "draws and resyncs when the clock goes backwards"],
  ["frame pacing", "raises the rate while an orb is settling"],
  ["pixel sizing", "clamps devicePixelRatio to 2 and repairs nonsense"],
  ["pixel sizing", "sizes the shared canvas to the largest orb, rounded up to a step"],
  ["pixel sizing", "sizes the drawing buffer from CSS size and clamped DPR"],
  ["loop policy", "draws a visible orb when its frame is due"],
  ["loop policy", "skips offscreen orbs entirely"],
  ["loop policy", "stops everything while the document is hidden or the context is lost"],
  [
    "loop policy",
    "under reduced motion draws only pending static frames and never loops otherwise",
  ],
  ["loop policy", "draws a dirty orb immediately, regardless of pacing"],
];

const testLine = ([suite, name, ms]: (typeof TEST_NAMES)[number]): string =>
  `   ${green("✓")} ${suite} ${gray(">")} ${name}${ms ? ` ${dim(`${ms}ms`)}` : ""}`;

const TERMINAL_HEAD = [
  "",
  `> @ghost/ui@0.0.0 test`,
  "> vitest run pacing",
  "",
  ` ${runBadge} ${cyan("v5.0.5 ")}${gray("packages/ui")}`,
  "",
].join("\n");

const TERMINAL_FILE = ` ${green("✓")} src/orb/pacing.test.ts ${gray("(19 tests)")} ${dim("8ms")}`;

const TERMINAL_SUMMARY = [
  "",
  ` ${dim("Test Files")}  ${bold(green("1 passed"))} ${gray("(1)")}`,
  `      ${dim("Tests")}  ${bold(green("19 passed"))} ${gray("(19)")}`,
  `   ${dim("Start at")}  14:32:07`,
  `   ${dim("Duration")}  412ms ${gray("(transform 46ms, setup 0ms, collect 39ms, tests 8ms, environment 0ms, prepare 61ms)")}`,
  "",
].join("\n");

const terminal = (output: string, exitCode: number | null): LabEvent => ({
  type: "tool_update",
  tool_call_id: "toolu-run-tests",
  status: null,
  title: null,
  content: [{ type: "terminal", output, exit_code: exitCode, truncated: false }],
});

const PART_1 = TERMINAL_HEAD;
const PART_2 = [TERMINAL_HEAD, TERMINAL_FILE, ...TEST_NAMES.slice(0, 9).map(testLine)].join("\n");
const PART_3 = [TERMINAL_HEAD, TERMINAL_FILE, ...TEST_NAMES.map(testLine)].join("\n");
export const GOLDEN_TEST_OUTPUT = `${PART_3}\n${TERMINAL_SUMMARY}`;

const plan = (
  first: "pending" | "in_progress" | "completed",
  second: "pending" | "in_progress" | "completed",
  third: "pending" | "in_progress" | "completed",
): LabEvent => ({
  type: "plan_updated",
  entries: [
    { content: "Reproduce the flake", status: first, priority: null },
    { content: "Make pacing tolerant to frame jitter", status: second, priority: null },
    { content: "Add a regression test", status: third, priority: null },
  ],
});

function buildGolden(): ChatRecording {
  const t = new TimelineBuilder();
  t.at(0, {
    type: "user_message",
    message_id: "msg-user-pacing",
    text: GOLDEN_PROMPT,
    agent: "claude-code",
  })
    .at(0.3, { type: "turn_started", turn_id: GOLDEN_TURN_ID })
    .at(0.35, {
      type: "session_updated",
      native_session_id: "4b1f0c52-7e9a-4d61-9a3e-2f8c5d7b6e10",
      title: "Fix the flaky orb pacing test",
      model: "claude-opus-5-5",
      effort: "high",
      mode: "bypassPermissions",
      commands: [],
    })
    // The thought: about 6 s.
    .stream(1.6, 7.6, GOLDEN_THOUGHT, "thought", (text) => ({
      type: "thought_delta",
      thought_id: "thought-pacing",
      text,
    }))
    .at(7.65, { type: "thought_completed", thought_id: "thought-pacing", duration_ms: 6000 })
    // Explore: two reads and a search, issued back to back.
    .at(8.4, {
      type: "tool_call",
      tool_call_id: "toolu-read-pacing",
      kind: "read",
      title: `Read ${PACING}`,
      status: "in_progress",
      locations: [{ path: PACING, line: null }],
      raw_input: { file_path: PACING },
    })
    .at(8.75, {
      type: "tool_update",
      tool_call_id: "toolu-read-pacing",
      status: "completed",
      title: null,
      content: [],
    })
    .at(8.9, {
      type: "tool_call",
      tool_call_id: "toolu-read-pacing-test",
      kind: "read",
      title: `Read ${PACING_TEST}`,
      status: "in_progress",
      locations: [{ path: PACING_TEST, line: null }],
      raw_input: { file_path: PACING_TEST },
    })
    .at(9.2, {
      type: "tool_update",
      tool_call_id: "toolu-read-pacing-test",
      status: "completed",
      title: null,
      content: [],
    })
    .at(9.35, {
      type: "tool_call",
      tool_call_id: "toolu-grep-now",
      kind: "search",
      title: 'grep "performance.now" packages/ui/src',
      status: "in_progress",
      locations: [],
      raw_input: { pattern: "performance.now", path: "packages/ui/src", output_mode: "content" },
    })
    .at(9.9, {
      type: "tool_update",
      tool_call_id: "toolu-grep-now",
      status: "completed",
      title: null,
      content: [{ type: "text", text: SEARCH_RESULTS }],
    })
    // The cause, explained.
    .stream(11.6, 18.4, GOLDEN_CAUSE, "cause", (text) => ({
      type: "message_delta",
      message_id: "msg-cause",
      text,
    }))
    .at(18.45, { type: "message_completed", message_id: "msg-cause" })
    .at(19.0, plan("completed", "in_progress", "pending"))
    // Edit 1: jitter tolerance and a fixed frame grid.
    .at(19.8, {
      type: "tool_call",
      tool_call_id: "toolu-edit-pacing",
      kind: "edit",
      title: `Edit ${PACING}`,
      status: "in_progress",
      locations: [{ path: PACING, line: 58 }],
      raw_input: {
        file_path: PACING,
        old_string: "  return elapsed >= interval;",
        new_string: "  return elapsed >= interval - slackMs(interval);",
      },
    })
    .at(20.5, {
      type: "tool_update",
      tool_call_id: "toolu-edit-pacing",
      status: "completed",
      title: null,
      content: [{ type: "diff", path: PACING, old_text: PACING_BEFORE, new_text: PACING_AFTER }],
    })
    .at(20.9, plan("completed", "completed", "in_progress"))
    // Edit 2: a simulated clock and the regression test.
    .at(21.6, {
      type: "tool_call",
      tool_call_id: "toolu-edit-pacing-test",
      kind: "edit",
      title: `Edit ${PACING_TEST}`,
      status: "in_progress",
      locations: [{ path: PACING_TEST, line: 18 }],
      raw_input: {
        file_path: PACING_TEST,
        old_string:
          "const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));",
        new_string:
          "function countFrames(fps: number, hz: number, seconds: number, jitterMs = 0): number {",
      },
    })
    .at(22.2, {
      type: "tool_update",
      tool_call_id: "toolu-edit-pacing-test",
      status: "completed",
      title: null,
      content: [
        {
          type: "diff",
          path: PACING_TEST,
          old_text: PACING_TEST_BEFORE,
          new_text: PACING_TEST_AFTER,
        },
      ],
    })
    // The test run, output streaming in.
    .at(23.0, {
      type: "tool_call",
      tool_call_id: "toolu-run-tests",
      kind: "execute",
      title: TEST_COMMAND,
      status: "in_progress",
      locations: [],
      raw_input: { command: TEST_COMMAND, description: "Run the pacing tests" },
    })
    .at(23.5, terminal(PART_1, null))
    .at(24.6, terminal(PART_2, null))
    .at(25.6, terminal(PART_3, null))
    .at(26.2, terminal(GOLDEN_TEST_OUTPUT, null))
    .at(26.5, {
      type: "tool_update",
      tool_call_id: "toolu-run-tests",
      status: "completed",
      title: null,
      content: [{ type: "terminal", output: GOLDEN_TEST_OUTPUT, exit_code: 0, truncated: false }],
    })
    .at(27.4, plan("completed", "completed", "completed"))
    // The answer.
    .stream(29.2, 47.1, GOLDEN_ANSWER, "answer", (text) => ({
      type: "message_delta",
      message_id: "msg-answer",
      text,
    }))
    .at(47.15, { type: "message_completed", message_id: "msg-answer" })
    .at(47.6, {
      type: "usage_updated",
      context_used: 41_380,
      context_size: 1_000_000,
      cost: { amount: 0.21, currency: "USD" },
    })
    .at(48.3, {
      type: "turn_ended",
      turn_id: GOLDEN_TURN_ID,
      stop_reason: "end_turn",
      usage: {
        input_tokens: 4_812,
        output_tokens: 2_147,
        cached_input_tokens: 96_384,
        reasoning_tokens: null,
      },
    });
  const events = t.build();
  return { chatId: CHAT_PACING, events, duration: events[events.length - 1]?.at ?? 0 };
}

export const GOLDEN: ChatRecording = buildGolden();
