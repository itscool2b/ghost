import { brandCssVariables, getBrand, Orb, type OrbState } from "@ghost/ui";
import {
  ArrowRight,
  BookOpen,
  Bug,
  Check,
  ChevronDown,
  ChevronRight,
  CornerDownRight,
  FileDiff,
  GitCommitHorizontal,
  type LucideIcon,
  PanelRight,
  X,
} from "lucide-react";
import { motion } from "motion/react";
import { type CSSProperties, type ReactNode, useState } from "react";
import {
  type PermissionModel,
  type PlanEntry,
  type PlanModel,
  type PlanStatus,
  type Player,
  type Project,
  type TranscriptModel,
  type Turn,
  usePlayhead,
} from "../../kit";
import { LiveElapsed } from "./blocks";
import {
  Collapse,
  cx,
  Glass,
  IconButton,
  Kbd,
  LineCounts,
  RADIUS,
  useDepth,
  useEntrance,
} from "./primitives";
import { MiniOrb } from "./sidebar";
import {
  type HeldStatus,
  holdStatus,
  type LiveStatus,
  liveStatus,
  permissionTarget,
  VERB_HOLD_MS,
} from "./status";
import type { Halt } from "./transcript";

/** Files edited so far in the open chat, with line counts, for the header's changes button. */
export function changesOf(model: TranscriptModel): {
  readonly files: number;
  readonly added: number;
  readonly removed: number;
} {
  const files = new Set<string>();
  let added = 0;
  let removed = 0;
  for (const turn of model.turns) {
    for (const item of turn.items) {
      if (item.kind !== "tools" || item.family !== "edit") continue;
      for (const file of item.files) files.add(file);
      added += item.added;
      removed += item.removed;
    }
  }
  return { files: files.size, added, removed };
}

/**
 * The window's top band (and drag area): where you are, and the side panel. Content scrolls up
 * under it and fades out before it reaches the title.
 */
export function Header({
  project,
  title,
  changes,
}: {
  readonly project: Project | null;
  readonly title: string;
  readonly changes: ReturnType<typeof changesOf>;
}) {
  return (
    <header className="halo-header">
      <div className="halo-crumbs">
        {project ? (
          <span className="halo-crumb-project">
            <span
              className="halo-project-dot"
              style={{ "--project": project.color } as CSSProperties}
              aria-hidden="true"
            />
            {project.name}
          </span>
        ) : null}
        <ChevronRight size={14} className="halo-crumb-sep" aria-hidden="true" />
        <h1 className="halo-title">{title}</h1>
      </div>
      <div className="halo-toolbar">
        {changes.files > 0 ? (
          <button type="button" className="halo-tool-chip" data-halo-light="" title="Show changes">
            <FileDiff size={14} aria-hidden="true" />
            <span>
              {changes.files} {changes.files === 1 ? "file" : "files"}
            </span>
            <LineCounts added={changes.added} removed={changes.removed} />
          </button>
        ) : null}
        <button
          type="button"
          className="halo-tool-chip halo-tool-icon"
          aria-label="Show side panel"
          title="Side panel"
        >
          <PanelRight size={15} aria-hidden="true" />
        </button>
      </div>
    </header>
  );
}

/* ------------------------------------------------------------------------------ the plan */

/**
 * The plan's progress as a row of pips, one per step: lit when done, glowing while in progress.
 * Long plans become one bar. Distinct from the round spinners that mean "running".
 */
function PlanPips({ entries }: { readonly entries: readonly PlanEntry[] }) {
  if (entries.length > 8) {
    const done = entries.filter((entry) => entry.status === "completed").length;
    return (
      <span className="halo-pips-bar" aria-hidden="true">
        <span style={{ width: `${(done / entries.length) * 100}%` }} />
      </span>
    );
  }
  return (
    <span className="halo-pips" aria-hidden="true">
      {entries.map((entry, index) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: one pip per step, in order.
        <span key={index} data-status={entry.status} />
      ))}
    </span>
  );
}

function PlanMark({ status }: { readonly status: PlanStatus }) {
  const enter = useEntrance();
  if (status === "in_progress") {
    return <span className="halo-plan-mark halo-plan-now" aria-hidden="true" />;
  }
  if (status === "completed") {
    return (
      <span
        className={cx("halo-plan-mark halo-plan-done", enter && "halo-check-in")}
        aria-hidden="true"
      >
        <Check size={11} strokeWidth={3} />
      </span>
    );
  }
  return <span className="halo-plan-mark halo-plan-todo" aria-hidden="true" />;
}

const PLAN_STATUS: Readonly<Record<PlanStatus, string>> = {
  completed: "done",
  in_progress: "in progress",
  pending: "to do",
};

function PlanList({ plan }: { readonly plan: PlanModel }) {
  return (
    <ol className="halo-plan-list" aria-label="Plan">
      {plan.entries.map((entry) => (
        <li
          key={entry.content}
          className="halo-plan-entry"
          data-status={entry.status}
          aria-label={`${entry.content}, ${PLAN_STATUS[entry.status]}`}
        >
          <PlanMark key={entry.status} status={entry.status} />
          <span className="halo-plan-text">{entry.content}</span>
        </li>
      ))}
    </ol>
  );
}

/* -------------------------------------------------------------------------- status line */

/** A message typed while the agent works, sent when it finishes. */
export interface QueuedMessage {
  readonly id: string;
  readonly text: string;
}

/** A permission answered here before the recording moved on: the agent reads as carrying on. */
export interface Resumed {
  readonly permission: PermissionModel;
  readonly allowed: boolean;
}

/** The live status with each verb held on the line for a moment (see status.ts). */
function useHeldStatus(raw: LiveStatus, now: number, playing: boolean): LiveStatus {
  const [held, setHeld] = useState<HeldStatus>(() => holdStatus(null, raw, now));
  const next = holdStatus(held, raw, now, playing ? VERB_HOLD_MS : 0);
  // Adjusted during render: the line never shows a frame of the status it is holding back.
  if (next !== held) setHeld(next);
  return next.status;
}

function StatusText({ status }: { readonly status: LiveStatus }) {
  return (
    <span className="halo-status-text">
      <span className={cx("halo-status-verb", status.tone === "work" && "halo-shimmer")}>
        {status.verb}
      </span>
      {status.target ? (
        <span className="halo-status-target" data-mono={status.mono ? "" : undefined}>
          {status.target}
        </span>
      ) : null}
    </span>
  );
}

/** What the line says right after you answer a permission: the command running, or thinking. */
function resumedStatus(resumed: Resumed): LiveStatus {
  if (!resumed.allowed) return { verb: "Thinking", target: null, mono: false, tone: "work" };
  const target = permissionTarget(resumed.permission);
  return { verb: "Running", target: target.text, mono: target.mono, tone: "work" };
}

/** The running turn: what it does now, the plan's progress, the turn's clock, and how to stop. */
function LiveRow({
  turn,
  model,
  activity,
  resumed,
  player,
  planOpen,
  onPlan,
}: {
  readonly turn: Turn;
  readonly model: TranscriptModel;
  readonly activity: OrbState;
  readonly resumed: Resumed | null;
  readonly player: Player;
  readonly planOpen: boolean;
  readonly onPlan: () => void;
}) {
  const now = usePlayhead(player, 250);
  const raw = resumed ? resumedStatus(resumed) : liveStatus(turn, activity, model.plan, now);
  const status = useHeldStatus(raw, now, player.playing);
  const waiting = status.tone === "wait";
  const plan = model.plan !== null && model.plan.turnId === turn.id ? model.plan : null;
  return (
    <div className="halo-status-row" data-tone={status.tone}>
      <MiniOrb
        agent={turn.agent ?? "generic"}
        status={waiting ? "needs_approval" : "running"}
        size={14}
      />
      <StatusText status={status} />
      {plan ? (
        <button
          type="button"
          className="halo-plan-chip"
          data-halo-light=""
          aria-expanded={planOpen}
          aria-label={`Plan, ${plan.completed} of ${plan.total} done${plan.current ? `, now: ${plan.current.content}` : ""}`}
          title={plan.current ? `Now: ${plan.current.content}` : "Plan"}
          onClick={onPlan}
        >
          <PlanPips entries={plan.entries} />
          <span className="halo-plan-chip-count">
            {plan.completed} of {plan.total}
          </span>
          {/* A command or pattern on the line needs the room; the step is in the tooltip then. */}
          {plan.current && !status.mono ? (
            <span className="halo-plan-chip-step">{plan.current.content}</span>
          ) : null}
          <ChevronDown size={13} className="halo-chevron" aria-hidden="true" />
        </button>
      ) : null}
      <LiveElapsed player={player} since={turn.startedAt} className="halo-status-time" />
      <span className="halo-status-keys">
        <Kbd>esc</Kbd>
        {waiting ? "reject" : "stop"}
      </span>
    </div>
  );
}

/**
 * The status slab: a narrower pane tucked behind the composer, the one place that always says what
 * is going on. While a turn runs it is the status line (what the agent does now, the plan's
 * progress, the turn's clock, Escape to stop); after a stop it says so; when you pick another
 * agent it says the next message carries a handoff; otherwise it holds the plan, folded. The plan's
 * checklist opens above the line on click, and queued messages wait under it. It wears the colours
 * of the agent it speaks for, whoever the composer points at.
 */
export function StatusSlab({
  model,
  activity,
  resumed,
  player,
  halt,
  handoff,
  queued,
  onUnqueue,
}: {
  readonly model: TranscriptModel;
  /** What the agent does now, after a stop or an answer given here. */
  readonly activity: OrbState;
  readonly resumed: Resumed | null;
  readonly player: Player;
  readonly halt: Halt;
  /** The agent the next message goes to, when it is not the one this chat has been talking to. */
  readonly handoff: { readonly from: string; readonly to: string } | null;
  readonly queued: readonly QueuedMessage[];
  readonly onUnqueue: (id: string) => void;
}) {
  const [planOpen, setPlanOpen] = useState(false);
  // Parallax relative to the dock it sits in: a little behind the composer.
  const { x, y } = useDepth(-2);
  const turn = model.turns[model.turns.length - 1] ?? null;
  const running = turn !== null && (turn.status === "running" || turn.status === "waiting");
  const plan = model.plan;
  const mode: "live" | "halted" | "handoff" | "plan" | null = running
    ? halt === null
      ? "live"
      : "halted"
    : handoff
      ? "handoff"
      : plan
        ? "plan"
        : null;
  if (mode === null && queued.length === 0) return null;

  const planTurn = plan
    ? (model.turns.find((candidate) => candidate.id === plan.turnId) ?? null)
    : null;
  const agent =
    mode === "handoff" && handoff
      ? handoff.to
      : mode === "plan"
        ? (planTurn?.agent ?? turn?.agent ?? "generic")
        : (turn?.agent ?? "generic");
  const showPlan = plan !== null && planOpen && (mode === "plan" || plan.turnId === turn?.id);
  const togglePlan = (): void => setPlanOpen((value) => !value);

  let row: ReactNode = null;
  if (mode === "live" && turn) {
    row = (
      <LiveRow
        turn={turn}
        model={model}
        activity={activity}
        resumed={resumed}
        player={player}
        planOpen={planOpen}
        onPlan={togglePlan}
      />
    );
  } else if (mode === "halted" && turn) {
    row = (
      <div className="halo-status-row" data-tone="halted">
        <MiniOrb agent={turn.agent ?? "generic"} status="idle" size={14} />
        <span className="halo-status-text">
          <span className="halo-status-verb">{halt === "stopping" ? "Stopping" : "Stopped"}</span>
        </span>
        <LiveElapsed player={player} since={turn.startedAt} className="halo-status-time" />
        {halt === "stopping" ? (
          <span className="halo-status-keys">
            <Kbd>esc</Kbd>
            force stop
          </span>
        ) : null}
      </div>
    );
  } else if (mode === "handoff" && handoff) {
    row = (
      <div className="halo-status-row" data-tone="handoff">
        <span className="halo-handoff-orbs" aria-hidden="true">
          <MiniOrb agent={handoff.from} status="idle" size={14} />
          <ArrowRight size={12} />
          <MiniOrb agent={handoff.to} status="done_unseen" size={14} />
        </span>
        <span className="halo-status-text">
          <span className="halo-status-verb">{getBrand(handoff.to).name} takes over</span>
          <span className="halo-status-target">
            Your next message includes a handoff of this chat
          </span>
        </span>
      </div>
    );
  } else if (mode === "plan" && plan) {
    row = (
      <button
        type="button"
        className="halo-status-row halo-plan-head"
        data-halo-light=""
        aria-expanded={planOpen}
        onClick={togglePlan}
      >
        {plan.done ? (
          <span className="halo-plan-mark halo-plan-done halo-plan-badge" aria-hidden="true">
            <Check size={12} strokeWidth={3} />
          </span>
        ) : (
          <PlanPips entries={plan.entries} />
        )}
        <span className="halo-plan-title">{plan.done ? "Plan complete" : "Plan"}</span>
        <span className="halo-plan-count">
          {plan.completed} of {plan.total}
        </span>
        {!plan.done && plan.current ? (
          <span className="halo-plan-current">{plan.current.content}</span>
        ) : null}
        <ChevronDown size={14} className="halo-chevron halo-plan-chevron" aria-hidden="true" />
      </button>
    );
  }

  return (
    <motion.div
      className="halo-status"
      data-mode={mode ?? "queue"}
      style={{ x, y, ...(brandCssVariables(getBrand(agent)) as CSSProperties) }}
    >
      <Glass material="near" radius={RADIUS.bubble} />
      {plan ? (
        <Collapse open={showPlan}>
          <PlanList plan={plan} />
        </Collapse>
      ) : null}
      {row}
      {queued.length > 0 ? (
        <ul className="halo-queue" aria-label="Queued messages">
          {queued.map((message) => (
            <li key={message.id} className="halo-queue-row">
              <CornerDownRight size={13} className="halo-queue-icon" aria-hidden="true" />
              <span className="halo-queue-label">Queued</span>
              <span className="halo-queue-text">{message.text}</span>
              <IconButton
                icon={X}
                label="Remove from queue"
                size={13}
                className="halo-queue-remove"
                onClick={() => onUnqueue(message.id)}
              />
            </li>
          ))}
        </ul>
      ) : null}
    </motion.div>
  );
}

/* --------------------------------------------------------------------------------- hero */

const SUGGESTIONS: readonly { readonly icon: LucideIcon; readonly text: string }[] = [
  { icon: Bug, text: "Find out why a test is flaky" },
  { icon: BookOpen, text: "Explain how the orb renderer works" },
  { icon: GitCommitHorizontal, text: "Review my last commit" },
];

/**
 * A new chat: the agent's orb floating in its halo rings, one question, and the composer right
 * under it, nearest of all.
 */
export function EmptyHero({
  agent,
  project,
  composer,
}: {
  readonly agent: string;
  readonly project: Project | null;
  readonly composer: ReactNode;
}) {
  const brand = getBrand(agent);
  const orb = useDepth(9);
  return (
    <div className="halo-hero">
      <motion.div className="halo-hero-orb" style={{ x: orb.x, y: orb.y }}>
        <span className="halo-hero-glow" aria-hidden="true" />
        {/* The halo: one tilted ring, drawn as two halves so it passes behind the orb and in front. */}
        <span className="halo-hero-halo" data-half="back" aria-hidden="true">
          <span className="halo-hero-halo-bloom" />
          <span className="halo-hero-halo-ring" />
        </span>
        <span className="halo-hero-lens" aria-hidden="true" />
        <Orb brand={brand} state="idle" size={120} className="halo-hero-core" />
        <span className="halo-hero-halo" data-half="front" aria-hidden="true">
          <span className="halo-hero-halo-bloom" />
          <span className="halo-hero-halo-ring" />
        </span>
      </motion.div>
      <h2 className="halo-hero-title">
        What should we build in{" "}
        <span className="halo-hero-project">{project?.name ?? "this project"}</span>?
      </h2>
      <p className="halo-hero-sub">{brand.name} is ready. Ask for a fix, a feature or a review.</p>
      <div className="halo-hero-composer" data-parallax-hold="">
        {composer}
      </div>
      <ul className="halo-suggestions" aria-label="Suggestions">
        {SUGGESTIONS.map(({ icon: Icon, text }) => (
          <li key={text}>
            <button type="button" className="halo-suggestion" data-halo-light="">
              <Icon size={14} className="halo-suggestion-icon" aria-hidden="true" />
              {text}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
