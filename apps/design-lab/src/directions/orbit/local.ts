import type { AgentBrandId } from "@ghost/ui";
import { chunkText, seededRandom } from "../../fixture/timeline";
import type { LabEvent, TimedEvent } from "../../fixture/types";
import type { PermissionModel, TranscriptModel } from "../../kit";
import type { AnswerKind } from "./state";

/*
 * What happens in the window after a recording ends, as events. The recordings stop where they
 * stop; when the user answers a request, sends a message or stops a turn, Orbit writes the events a
 * real agent would send next and runs them through the lab's own reducer, so a scripted reply, a
 * command let through and a stopped turn render with the same rows, timers and receipts as the
 * recording. Everything here is a pure function of the actions and their times, so it is tested
 * in Node and a given moment always renders the same way.
 */

/** Something the user did here, at a time on the window's clock (ms). */
export type LocalAction =
  | {
      readonly kind: "send";
      readonly id: string;
      readonly at: number;
      readonly text: string;
      readonly agent: AgentBrandId;
      readonly agentName: string;
      readonly model: string | null;
      readonly effort: string | null;
    }
  | {
      readonly kind: "answer";
      readonly id: string;
      readonly at: number;
      readonly request: PermissionModel;
      /** The turn the request holds. */
      readonly turnId: string;
      readonly answer: AnswerKind;
      /** Deny with instructions: a send follows, so the turn ends without a reply of its own. */
      readonly instead: boolean;
    }
  | {
      readonly kind: "stop";
      readonly id: string;
      readonly at: number;
      readonly turnId: string;
    };

interface Tagged extends TimedEvent {
  /** The turn the event belongs to, so a stop can cut what that turn would have sent next. */
  readonly turnId: string;
}

/**
 * What an agent does with a message sent here. No agent runs behind the lab, so each reply is a
 * script, written the way auracode's agents report work: one check run with its numbers, then two
 * plain sentences. The window says once, as a system line, that the reply is scripted; the reply
 * itself stays in the product's voice.
 */
interface Script {
  readonly thought: string;
  readonly command: string;
  readonly output: readonly string[];
  /** How long the check runs, ms. */
  readonly runMs: number;
  readonly reply: string;
}

const VITEST_LINES: readonly string[] = [
  "",
  " \u001b[46m\u001b[30m RUN \u001b[39m\u001b[49m v3.2.4 packages/ui",
  "",
  " \u001b[32m\u2713\u001b[39m src/orb/pacing.test.ts (19 tests) 41ms",
  "",
  " Test Files  \u001b[32m1 passed\u001b[39m (1)",
  "      Tests  \u001b[32m19 passed\u001b[39m (19)",
  "   Duration  2.38s",
];

const GIT_LINES: readonly string[] = [
  " packages/ui/src/orb/pacing.test.ts | 58 +++++++++++++++++++-------",
  " packages/ui/src/orb/pacing.ts      | 24 ++++++++----",
  " 2 files changed, 59 insertions(+), 23 deletions(-)",
];

export function scriptFor(agent: string): Script {
  if (agent === "codex") {
    return {
      thought: "Running the driver tests before answering.",
      command: "cargo test -p aura-drivers",
      output: CARGO_OUTPUT_LINES,
      runMs: 2400,
      reply:
        "The driver tests still pass: 12 of 12, in 2.4s. Tell me what to change next and I will make the edit.",
    };
  }
  if (agent === "claude-code") {
    return {
      thought: "Running the pacing tests again before answering.",
      command: "pnpm --filter @ghost/ui test pacing",
      output: VITEST_LINES,
      runMs: 2400,
      reply:
        "The pacing tests still pass: 19 of 19, in 2.4s. Tell me what to change next and I will make the edit.",
    };
  }
  return {
    thought: "Checking what changed in the working tree first.",
    command: "git diff --stat",
    output: GIT_LINES,
    runMs: 600,
    reply:
      "Two files changed since the last commit: 59 lines added, 23 removed. Tell me what to change next and I will make the edit.",
  };
}

/** The system line a scripted reply carries, once per chat. */
export const SCRIPTED_NOTE = "Scripted reply · no agent is connected in the lab";

/** What a command let through prints: cargo's test run, about 3 s, ending on its summary. */
export const CARGO_OUTPUT_LINES: readonly string[] = [
  "   Compiling aura-drivers v0.1.0 (crates/aura-drivers)",
  "    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.41s",
  "     Running unittests src/lib.rs (target/debug/deps/aura_drivers-3f9c2e1a07b4d8c6)",
  "",
  "running 12 tests",
  "test acp::tests::initialize_turns_fs_and_terminal_off ... \u001b[32mok\u001b[0m",
  "test acp::tests::maps_message_and_thought_chunks ... \u001b[32mok\u001b[0m",
  "test batch::tests::frames_stay_under_the_size_limit ... \u001b[32mok\u001b[0m",
  "",
  "test result: \u001b[32mok\u001b[0m. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.31s",
];

/** When each line of the canned run appears, ms after the command starts. */
const CARGO_LINE_AT: readonly number[] = [
  300, 1300, 1700, 1700, 1900, 2300, 2500, 2700, 2900, 3000,
];
/** The canned run's length, ms. */
export const CARGO_RUN_MS = 3100;

const ALLOWED_REPLY =
  "All 12 driver tests pass, including the new handshake test. The client spawns the agent, completes `initialize` and `session/new`, and maps message and thought chunks to `AuraEvent`s.";

const DENIED_REPLY =
  "Okay, I won't run it. The ACP client is written; run `cargo test -p aura-drivers` yourself when you want to check the handshake.";

/** Streams `text` as deltas from `start` to `end` ms, the way the fixtures do. */
function stream(
  out: Tagged[],
  turnId: string,
  start: number,
  end: number,
  text: string,
  seed: string,
  toEvent: (delta: string) => LabEvent,
): void {
  const chunks = chunkText(text, seed);
  const random = seededRandom(`${seed}:time`);
  const step = chunks.length > 1 ? (end - start) / (chunks.length - 1) : 0;
  chunks.forEach((chunk, index) => {
    const nudge = index === 0 || index === chunks.length - 1 ? 0 : (random() - 0.5) * (step / 2);
    out.push({ at: Math.round(start + step * index + nudge), event: toEvent(chunk), turnId });
  });
}

/** A rough token count for scripted usage: four characters a token. */
const tokens = (text: string): number => Math.max(1, Math.round(text.length / 4));

/** How long a scripted message takes to stream: about 110 characters a second. */
const streamMs = (text: string): number => Math.round((text.length / 110) * 1000);

function sendEvents(
  action: Extract<LocalAction, { kind: "send" }>,
  session: TranscriptModel["session"],
  contextTokens: number,
): Tagged[] {
  const out: Tagged[] = [];
  const turnId = localTurnId(action.id);
  const t = action.at;
  const script = scriptFor(action.agent);
  const push = (at: number, event: LabEvent): void => {
    out.push({ at: Math.round(at), event, turnId });
  };
  push(t, {
    type: "user_message",
    message_id: `local-user-${action.id}`,
    text: action.text,
    agent: action.agent,
  });
  push(t + 150, { type: "turn_started", turn_id: turnId });
  push(t + 160, {
    type: "session_updated",
    native_session_id: session.nativeSessionId,
    title: session.title ?? firstLine(action.text),
    model: action.model,
    effort: action.effort,
    mode: session.mode,
    commands: [],
  });
  stream(out, turnId, t + 400, t + 1200, script.thought, `${action.id}:thought`, (text) => ({
    type: "thought_delta",
    thought_id: `local-thought-${action.id}`,
    text,
  }));
  push(t + 1300, {
    type: "thought_completed",
    thought_id: `local-thought-${action.id}`,
    duration_ms: null,
  });
  // The check: one command, its output arriving in two beats, then its exit.
  const callId = `local-call-${action.id}`;
  const runAt = t + 1500;
  push(runAt, {
    type: "tool_call",
    tool_call_id: callId,
    kind: "execute",
    title: script.command,
    status: "in_progress",
    locations: [],
    raw_input: { command: script.command },
  });
  const half = Math.ceil(script.output.length / 2);
  push(runAt + script.runMs / 2, {
    type: "tool_update",
    tool_call_id: callId,
    status: null,
    title: null,
    content: [
      {
        type: "terminal",
        output: `${script.output.slice(0, half).join("\n")}\n`,
        exit_code: null,
        truncated: false,
      },
    ],
  });
  push(runAt + script.runMs, {
    type: "tool_update",
    tool_call_id: callId,
    status: "completed",
    title: null,
    content: [
      { type: "terminal", output: `${script.output.join("\n")}\n`, exit_code: 0, truncated: false },
    ],
  });
  const replyAt = runAt + script.runMs + 500;
  const replyEnd = replyAt + streamMs(script.reply);
  stream(out, turnId, replyAt, replyEnd, script.reply, `${action.id}:reply`, (text) => ({
    type: "message_delta",
    message_id: `local-reply-${action.id}`,
    text,
  }));
  push(replyEnd + 50, { type: "message_completed", message_id: `local-reply-${action.id}` });
  push(replyEnd + 300, {
    type: "turn_ended",
    turn_id: turnId,
    stop_reason: "end_turn",
    usage: {
      input_tokens: contextTokens + tokens(action.text),
      output_tokens: tokens(script.reply) + tokens(script.thought) + 40,
      cached_input_tokens: null,
      reasoning_tokens: null,
    },
  });
  return out;
}

/** The turn a message sent here starts, by the message's action id. */
export function localTurnId(actionId: string): string {
  return `local-turn-${actionId}`;
}

/** The option a lab answer selects among the agent's own options. */
function optionFor(request: PermissionModel, answer: AnswerKind): string | null {
  const kinds =
    answer === "deny"
      ? ["reject_once", "reject_always"]
      : answer === "allow_always"
        ? ["allow_always", "allow_once"]
        : ["allow_once", "allow_always"];
  for (const kind of kinds) {
    const option = request.options.find((candidate) => candidate.kind === kind);
    if (option) return option.option_id;
  }
  return null;
}

function answerEvents(action: Extract<LocalAction, { kind: "answer" }>): Tagged[] {
  const out: Tagged[] = [];
  const { request, turnId } = action;
  const t = action.at;
  const push = (at: number, event: LabEvent): void => {
    out.push({ at: Math.round(at), event, turnId });
  };
  const option = optionFor(request, action.answer);
  push(
    t,
    option === null
      ? {
          type: "permission_resolved",
          request_id: request.requestId,
          decision: { type: "cancelled" },
        }
      : {
          type: "permission_resolved",
          request_id: request.requestId,
          decision: { type: "selected", option_id: option },
        },
  );
  const callId = request.toolCallId;
  if (action.answer === "deny") {
    if (callId) {
      push(t + 1, {
        type: "tool_update",
        tool_call_id: callId,
        status: "failed",
        title: null,
        content: [],
      });
    }
    if (action.instead) {
      // The instructions go out as a message of their own: this turn ends here.
      push(t + 2, { type: "turn_ended", turn_id: turnId, stop_reason: "end_turn", usage: null });
      return out;
    }
    const replyAt = t + 500;
    const replyEnd = replyAt + streamMs(DENIED_REPLY);
    stream(out, turnId, replyAt, replyEnd, DENIED_REPLY, `${action.id}:deny`, (text) => ({
      type: "message_delta",
      message_id: `local-denied-${action.id}`,
      text,
    }));
    push(replyEnd + 50, { type: "message_completed", message_id: `local-denied-${action.id}` });
    push(replyEnd + 300, {
      type: "turn_ended",
      turn_id: turnId,
      stop_reason: "end_turn",
      usage: null,
    });
    return out;
  }
  // Let through: the command runs for about 3 s, prints its tests, and the agent reports back.
  if (callId) {
    push(t + 1, {
      type: "tool_update",
      tool_call_id: callId,
      status: "in_progress",
      title: null,
      content: [],
    });
    CARGO_OUTPUT_LINES.forEach((_, index) => {
      const at = CARGO_LINE_AT[index] ?? CARGO_RUN_MS;
      // Lines that share a moment arrive together.
      if (CARGO_LINE_AT[index + 1] === at) return;
      push(t + at, {
        type: "tool_update",
        tool_call_id: callId,
        status: null,
        title: null,
        content: [
          {
            type: "terminal",
            output: `${CARGO_OUTPUT_LINES.slice(0, index + 1).join("\n")}\n`,
            exit_code: null,
            truncated: false,
          },
        ],
      });
    });
    push(t + CARGO_RUN_MS, {
      type: "tool_update",
      tool_call_id: callId,
      status: "completed",
      title: null,
      content: [
        {
          type: "terminal",
          output: `${CARGO_OUTPUT_LINES.join("\n")}\n`,
          exit_code: 0,
          truncated: false,
        },
      ],
    });
  }
  const replyAt = t + CARGO_RUN_MS + 500;
  const replyEnd = replyAt + streamMs(ALLOWED_REPLY);
  stream(out, turnId, replyAt, replyEnd, ALLOWED_REPLY, `${action.id}:allowed`, (text) => ({
    type: "message_delta",
    message_id: `local-allowed-${action.id}`,
    text,
  }));
  push(replyEnd + 50, { type: "message_completed", message_id: `local-allowed-${action.id}` });
  push(replyEnd + 300, {
    type: "turn_ended",
    turn_id: turnId,
    stop_reason: "end_turn",
    usage: {
      input_tokens: 3140,
      output_tokens: tokens(ALLOWED_REPLY) + 220,
      cached_input_tokens: null,
      reasoning_tokens: null,
    },
  });
  return out;
}

/** A message's first line, cut to fit a header: the title of a chat that started here. */
export function firstLine(text: string, max = 60): string {
  const line = text.trim().split("\n")[0]?.trim() ?? "";
  return line.length > max ? `${line.slice(0, max - 1).trimEnd()}…` : line;
}

/**
 * The events the window's actions add after the recording, in time order. A stop cuts everything
 * its turn would have sent after it and ends the turn there, cancelled.
 */
export function localEvents(
  actions: readonly LocalAction[],
  base: TranscriptModel,
  contextTokens = 0,
): TimedEvent[] {
  let events: Tagged[] = [];
  const ordered = [...actions].sort((left, right) => left.at - right.at);
  for (const action of ordered) {
    if (action.kind === "send") {
      events = events.concat(sendEvents(action, base.session, contextTokens));
    } else if (action.kind === "answer") {
      events = events.concat(answerEvents(action));
    } else {
      events = events.filter((entry) => entry.turnId !== action.turnId || entry.at <= action.at);
      events.push({
        at: action.at,
        event: {
          type: "turn_ended",
          turn_id: action.turnId,
          stop_reason: "cancelled",
          usage: null,
        },
        turnId: action.turnId,
      });
    }
  }
  return events
    .map((entry, index) => ({ entry, index }))
    .sort((left, right) => left.entry.at - right.entry.at || left.index - right.index)
    .map(({ entry }) => ({ at: entry.at, event: entry.event }));
}

/**
 * A chat the lab has no recording for, when it failed: its last turn as the agent would have sent
 * it, ending on the error, so the chat opens on the error card instead of looking new. Times are
 * before the window's clock starts, so the turn is history the moment the chat opens. Null for any
 * chat that did not fail.
 */
export function failedHistory(chat: {
  readonly id: string;
  readonly title: string;
  readonly agent: AgentBrandId;
  readonly status: string;
}): TimedEvent[] | null {
  if (chat.status !== "error") return null;
  const turnId = `history-turn-${chat.id}`;
  const t = -600_000;
  const timed: TimedEvent[] = [];
  const push = (at: number, event: LabEvent): void => {
    timed.push({ at: t + at, event });
  };
  push(0, {
    type: "user_message",
    message_id: `history-user-${chat.id}`,
    text: `${chat.title}: the 6-digit code, a subject line and a plain-text version.`,
    agent: chat.agent,
  });
  push(150, { type: "turn_started", turn_id: turnId });
  push(160, {
    type: "session_updated",
    native_session_id: null,
    title: chat.title,
    model: "hermes-4.5-405b",
    effort: null,
    mode: null,
    commands: [],
  });
  push(400, {
    type: "thought_delta",
    thought_id: `history-thought-${chat.id}`,
    text: "Reading the existing email layout before writing the template.",
  });
  push(1600, {
    type: "thought_completed",
    thought_id: `history-thought-${chat.id}`,
    duration_ms: 1200,
  });
  push(1800, {
    type: "tool_call",
    tool_call_id: `history-read-${chat.id}`,
    kind: "read",
    title: "Read site/src/emails/layout.tsx",
    status: "in_progress",
    locations: [{ path: "site/src/emails/layout.tsx", line: null }],
    raw_input: null,
  });
  push(2100, {
    type: "tool_update",
    tool_call_id: `history-read-${chat.id}`,
    status: "completed",
    title: null,
    content: [],
  });
  push(2300, {
    type: "error",
    message:
      "Process exited with code 1\nError: model hermes-4.5-405b is not available on this endpoint (HTTP 404)",
    recoverable: false,
  });
  push(2350, { type: "turn_ended", turn_id: turnId, stop_reason: "error", usage: null });
  return timed;
}
