import { AGENT_BRANDS, type AgentBrandId } from "@ghost/ui";
import { Logo } from "@ghost/ui/graphite";
import { type RefObject, useEffect, useState } from "react";
import { useInView, usePageVisible, useReducedMotion } from "../motion";
import {
  Actions,
  Composer,
  Fold,
  Header,
  Icon,
  type IconName,
  Loader,
  SendDisc,
  Stat,
  Streamed,
  Tool,
  TurnAgent,
  Typed,
} from "./Mock";
import { SpriteMark } from "./Sprite";

/**
 * One session handed from agent to agent: each turn is a real kind of work (an edit, a test run, a
 * search) and each handoff is the app's own "Next message includes a handoff" flow.
 */
interface Turn {
  readonly agent: AgentBrandId;
  readonly prompt: string;
  readonly thought: string;
  readonly tool: {
    readonly icon: IconName;
    readonly verb: string;
    readonly object: string;
    readonly result?: string;
    readonly add?: number;
    readonly del?: number;
  };
  readonly reply: string;
  readonly time: string;
}

export const TURNS: readonly Turn[] = [
  {
    agent: "codex",
    prompt: "Check the pacing change for edge cases before we ship.",
    thought: "3.1 s",
    tool: { icon: "edit", verb: "Edited", object: "src/pacing.ts", add: 12, del: 3 },
    reply:
      "Frame pacing now waits on the clock and the tests pass. I did not look at what happens when a frame lands late.",
    time: "42 s",
  },
  {
    agent: "claude-code",
    prompt: "Check what a late frame does.",
    thought: "2.2 s",
    tool: { icon: "terminal", verb: "Ran", object: "cargo test pacing", result: "14 passed" },
    reply:
      "A late frame now skips ahead instead of stacking up. I added a test for two late frames in a row.",
    time: "31 s",
  },
  {
    agent: "pi",
    prompt: "Measure it on the slow machine profile.",
    thought: "1.4 s",
    tool: { icon: "terminal", verb: "Ran", object: "bench --profile slow", result: "p99 16.4 ms" },
    reply:
      "The worst frame under load is 16.4 ms, inside one frame budget. The old build hit 41 ms.",
    time: "1 m",
  },
  {
    agent: "antigravity",
    prompt: "Show the frame budget in the debug overlay.",
    thought: "2.8 s",
    tool: { icon: "edit", verb: "Edited", object: "src/overlay.tsx", add: 31, del: 2 },
    reply: "The overlay draws a budget bar for every frame and turns it red past 16.7 ms.",
    time: "48 s",
  },
  {
    agent: "cursor",
    prompt: "Tidy the pacing module before review.",
    thought: "1.9 s",
    tool: { icon: "edit", verb: "Edited", object: "src/pacing.ts", add: 6, del: 14 },
    reply:
      "The clock math lives in one function now and the old fallback is gone. Nothing behaves differently.",
    time: "27 s",
  },
  {
    agent: "grok",
    prompt: "Look for anything else that blocks a frame.",
    thought: "2.5 s",
    tool: { icon: "search", verb: "Searched", object: "await src/render", result: "3 matches" },
    reply:
      "Two awaits in the render path can stall a frame. I left a note on each with a suggested fix.",
    time: "19 s",
  },
  {
    agent: "opencode",
    prompt: "Fix those two awaits.",
    thought: "1.7 s",
    tool: { icon: "edit", verb: "Edited", object: "src/render/loop.ts", add: 9, del: 7 },
    reply: "Both now run off the frame path. The overlay stays green through the stress scene.",
    time: "36 s",
  },
  {
    agent: "oh-my-pi",
    prompt: "Write the changelog entry.",
    thought: "1.1 s",
    tool: { icon: "edit", verb: "Edited", object: "CHANGELOG.md", add: 5 },
    reply: "Added one entry that covers frame pacing, late frames and the new overlay.",
    time: "12 s",
  },
  {
    agent: "hermes",
    prompt: "Open a pull request with all of it.",
    thought: "1.3 s",
    tool: { icon: "terminal", verb: "Ran", object: "gh pr create", result: "opened" },
    reply: "The pull request lists every change and links the benchmark from the slow profile.",
    time: "22 s",
  },
];

type Phase = "pick" | "type" | "send" | "think" | "tool" | "reply" | "rest";

const ORDER: readonly Phase[] = ["pick", "type", "send", "think", "tool", "reply", "rest"];

/** How long each beat holds, in ms. The reply's length sets its streaming time. */
function holds(phase: Phase, turn: Turn): number {
  switch (phase) {
    case "pick":
      return 1000;
    case "type":
      return 500 + turn.prompt.length * 26;
    case "send":
      return 700;
    case "think":
      return 900;
    case "tool":
      return 950;
    case "reply":
      return 600 + turn.reply.split(" ").length * 46;
    case "rest":
      return 1500;
  }
}

export interface HandoffState {
  /** The turn being prepared (pick, type) or worked (send to rest). */
  readonly k: number;
  readonly phase: Phase;
  /** How many times the session has come round, so turn keys stay unique across laps. */
  readonly lap: number;
  /** The first, server-rendered frame: everything shown at rest, nothing animating in. */
  readonly settled: boolean;
}

/**
 * The page as it is printed and as it stays without motion: Codex's turn, the handoff to Claude
 * Code and its finished turn, and the next message, to Pi, typed in the composer. The window shows
 * the app's core move complete.
 */
export const INITIAL: HandoffState = { k: 2, phase: "type", lap: 0, settled: true };

function advance(state: HandoffState): HandoffState {
  const at = ORDER.indexOf(state.phase);
  if (at < ORDER.length - 1) return { ...state, phase: ORDER[at + 1] ?? "pick", settled: false };
  const k = (state.k + 1) % TURNS.length;
  return { k, phase: "pick", lap: k === 0 ? state.lap + 1 : state.lap, settled: false };
}

const turnAt = (k: number): Turn => TURNS[(k + TURNS.length) % TURNS.length] ?? (TURNS[0] as Turn);

/**
 * Runs the handoff loop while the hero is on screen, the tab is visible and motion is allowed.
 * It pauses mid-beat and resumes there; reduced motion keeps the printed frame.
 */
export function useHandoffLoop(target: RefObject<Element | null>): HandoffState {
  const [state, setState] = useState<HandoffState>(INITIAL);
  const [hydrated, setHydrated] = useState(false);
  const reduced = useReducedMotion();
  const visible = usePageVisible();
  const inView = useInView(target, { threshold: 0.15 });
  useEffect(() => setHydrated(true), []);
  const live = hydrated && !reduced && visible && inView;
  useEffect(() => {
    if (!live) return;
    const delay = state.settled ? 1800 : holds(state.phase, turnAt(state.k));
    const timer = window.setTimeout(() => setState(advance(state)), delay);
    return () => window.clearTimeout(timer);
  }, [live, state]);
  // Reduced motion returns to the printed frame, wherever the loop was.
  useEffect(() => {
    if (reduced) setState(INITIAL);
  }, [reduced]);
  return state;
}

const WORKING: readonly Phase[] = ["send", "think", "tool", "reply"];

function ToolResult({ tool }: { tool: Turn["tool"] }) {
  if (tool.result) return <span className="m-result">{tool.result}</span>;
  return <Stat add={tool.add} del={tool.del} />;
}

/** One turn: the handoff it began with, the message, and the agent's work as far as it got. */
function TurnBlock({
  turn,
  stage,
  handed,
}: {
  turn: Turn;
  stage: Phase | "done";
  handed: boolean;
}) {
  const reached = (phase: Phase) =>
    stage === "done" || ORDER.indexOf(stage) >= ORDER.indexOf(phase);
  const name = AGENT_BRANDS[turn.agent].name;
  return (
    <div className="hw-turn">
      {handed ? (
        <p className="hw-handed">
          <span className="hw-handed-rule" />
          <SpriteMark agent={turn.agent} size={14} />
          Handed to {name}
          <span className="hw-handed-rule" />
        </p>
      ) : null}
      <p className="m-user">{turn.prompt}</p>
      {reached("think") ? (
        <div className="m-turn">
          <TurnAgent agent={turn.agent} />
          {stage === "think" ? (
            <p className="m-line m-thinking">
              <span className="m-gutter">
                <Loader />
              </span>
              <span className="m-shimmer">Thinking</span>
            </p>
          ) : (
            <Fold>Thought for {turn.thought}</Fold>
          )}
          {reached("tool") ? (
            <Tool
              icon={turn.tool.icon}
              verb={turn.tool.verb}
              object={turn.tool.object}
              live={stage === "tool"}
            >
              {stage === "tool" ? null : <ToolResult tool={turn.tool} />}
            </Tool>
          ) : null}
          {reached("reply") ? (
            <p className="m-prose">
              {stage === "reply" ? <Streamed text={turn.reply} /> : turn.reply}
            </p>
          ) : null}
          {reached("rest") ? <Actions time={turn.time} /> : null}
        </div>
      ) : null}
    </div>
  );
}

const SIDEBAR_CHATS = [
  { title: "Tighten the hero spacing", time: "Tue" },
  { title: "Update the changelog", time: "4m" },
] as const;

/**
 * The hero's window, in the desktop's flat look: the sidebar on the window's colour with the
 * window's controls at its top, then the open chat on its pane: the toolbar, a transcript that
 * keeps the last turns, and the composer with its handoff. `.handoff-prior` and `.handoff-next`
 * name the parts the browser checks look for.
 */
export function HeroWindow({ state }: { state: HandoffState }) {
  const cur = turnAt(state.k);
  const prev = turnAt(state.k - 1);
  const before = state.phase === "pick" || state.phase === "type";
  const working = !before && WORKING.includes(state.phase);
  const nextName = AGENT_BRANDS[cur.agent].name;

  /*
   * The transcript keeps the newest turns. Each is keyed by its place in the session (lap and
   * index), so a finished turn keeps its DOM, and its finished look, as newer ones arrive under it.
   * Only the session's very first turn began without a handoff.
   */
  const slot = (offset: number) => {
    const raw = state.k + offset;
    const index = (raw + TURNS.length) % TURNS.length;
    const lap = raw < 0 ? state.lap - 1 : state.lap;
    return { index, lap, key: `${lap}:${index}`, handed: !(index === 0 && lap === 0) };
  };
  const shown = (before ? [slot(-2), slot(-1)] : [slot(-1), slot(0)]).filter(
    (turn) => turn.lap >= 0,
  );

  return (
    <div
      className="hw"
      data-phase={state.phase}
      data-settled={state.settled ? "" : undefined}
      data-working={working ? "" : undefined}
    >
      <div className="hw-body">
        <div className="hw-side">
          <div className="hw-bar">
            <span className="hw-dots">
              <i />
              <i />
              <i />
            </span>
          </div>
          <p className="m-nav m-search">
            <Icon name="search" />
            Search
          </p>
          <p className="m-nav">
            <Icon name="new" />
            New chat
          </p>
          <p className="m-nav">
            <Icon name="sliders" />
            Customize
          </p>
          <p className="m-section">Projects</p>
          <p className="m-row m-project-name">
            <Icon name="folder" size={14} />
            Ghost
          </p>
          <p className="m-row m-chat is-open">
            <span className="m-row-title">Ship the frame pacing fix</span>
            {working ? (
              <Loader />
            ) : (
              <SpriteMark agent={before ? prev.agent : cur.agent} size={14} />
            )}
          </p>
          {SIDEBAR_CHATS.map((chat) => (
            <p key={chat.title} className="m-row m-chat">
              <span className="m-row-title">{chat.title}</span>
              <span className="m-time">{chat.time}</span>
            </p>
          ))}
          <p className="m-row m-project-name">
            <Icon name="folder" size={14} />
            parser
          </p>
          <div className="m-foot">
            <Logo size={28} />
            <span className="m-icon-button">
              <Icon name="settings" />
            </span>
          </div>
        </div>
        <div className="hw-main">
          <Header title="Ship the frame pacing fix" project="Ghost" working={working} />
          <div className="handoff-stage">
            <div className="handoff-prior hw-transcript">
              {shown.map((turn) => (
                <TurnBlock
                  key={turn.key}
                  turn={turnAt(turn.index)}
                  stage={!before && turn.index === state.k ? state.phase : "done"}
                  handed={turn.handed}
                />
              ))}
            </div>
            <div className="handoff-next">
              <p className="m-handoff" data-shown={before ? "" : undefined}>
                <SpriteMark agent={cur.agent} size={16} />
                <span className="m-handoff-text">
                  Next message includes a handoff to {nextName}
                </span>
                <span className="m-strong">Preview</span>
              </p>
              <Composer
                agent={cur.agent}
                text={
                  state.phase === "type" ? (
                    state.settled ? (
                      cur.prompt
                    ) : (
                      <Typed text={cur.prompt} />
                    )
                  ) : undefined
                }
                typed={state.phase === "type"}
                placeholder={working ? `Queue a message for ${nextName}` : `Message ${nextName}`}
                send={
                  working ? (
                    <span className="m-disc m-stop">
                      <span className="m-stop-square" />
                    </span>
                  ) : (
                    <SendDisc ready={state.phase === "type"} />
                  )
                }
              />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
