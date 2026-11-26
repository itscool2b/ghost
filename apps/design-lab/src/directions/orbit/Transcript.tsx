import type { AgentBrandId } from "@ghost/ui";
import {
  Check,
  ChevronRight,
  Copy,
  FileDiff,
  GitFork,
  History,
  Info,
  MoreHorizontal,
  Pause,
  PenLine,
  RotateCcw,
  ScrollText,
  Trash2,
  X,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { memo, type ReactNode, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  type ChatSummary,
  effortLabel,
  findAgent,
  formatCount,
  formatElapsed,
  modelLabel,
  type NoticeModel,
  type PlanModel,
  type Player,
  type ThoughtItem,
  type TranscriptModel,
  type Turn,
  type TurnFooter,
  type TurnItem,
  usePlayhead,
  type Workspace,
} from "../../kit";
import type { MarkTimeline } from "./geometry";
import { Markdown } from "./Markdown";
import { Menu } from "./Menu";
import { OrbitMark } from "./OrbitMark";
import { PlanRow } from "./Plan";
import type { Extra, NowRow } from "./state";
import { ToolRun } from "./Tools";
import { formatClock, thoughtSummary } from "./text";
import { DiffCounts, DiffSquares, Elapsed, IconButton, Shimmer, TearLine, useReduced } from "./ui";

/*
 * The transcript (visual-language.md 4.4): the user's message as the one ink block (Grok Bot),
 * agent prose bare on the pane, finished work folded to one line, the agent's live mark in the
 * 22px gutter of the line that is live (or of a now-row in the gaps), and an X Money receipt under
 * every finished turn. A 720px column, 32px between turns, 12 between items. Every live line puts
 * its words at x 22 and its mark in the same gutter, so when one live line hands over to the next,
 * the words do not move.
 */

function agentName(workspace: Workspace, agent: string | null): string {
  if (agent === null) return "Agent";
  return findAgent(workspace, agent)?.name ?? agent;
}

const NO_EXTRAS: readonly Extra[] = [];

/** How an answer is copied: the words a reader sees, or the markdown source. */
export type CopyAs = "plain" | "markdown";

/** User messages longer than this clamp, fade into their own fill and offer "Show all". */
const CLAMP_LINES = 12;

/** A fold's open and close: height and opacity over --o-dock, a 120ms fade under reduced motion. */
export function foldMotion(reduced: boolean) {
  return {
    initial: { height: 0, opacity: 0 },
    animate: { height: "auto", opacity: 1 },
    exit: { height: 0, opacity: 0 },
    transition: { duration: reduced ? 0.12 : 0.26, ease: [0.23, 1, 0.32, 1] as const },
  };
}

/** The user's message: an ink block, its tail corner pointing back at the composer. */
export function UserBlock({ text, tag }: { readonly text: string; readonly tag?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [long, setLong] = useState(false);
  const [all, setAll] = useState(false);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    // 21px lines and 20px of padding: over 12 lines the block clamps.
    setLong(element.scrollHeight > CLAMP_LINES * 21 + 20 + 1);
  }, []);
  return (
    <div className="o-user" data-tag={tag ? "" : undefined}>
      {tag ? <span className="o-user-tag o-t-caps o-c3">{tag}</span> : null}
      <div ref={ref} className="o-user-block o-t-body" data-clamped={long && !all ? "" : undefined}>
        {text}
        {long ? (
          <button
            type="button"
            className="o-user-more"
            aria-expanded={all}
            onClick={() => setAll((value) => !value)}
          >
            {all ? "Show less" : "Show all"}
          </button>
        ) : null}
      </div>
    </div>
  );
}

/**
 * "Claude Code Opus 5.5 High": the name, then the model and effort a step quieter each. The model
 * and effort show on the first turn and whenever they change; between turns that run on the same
 * thing the byline is the mark and the name.
 */
function Byline({
  agent,
  workspace,
  model,
  effort,
  full,
}: {
  readonly agent: AgentBrandId;
  readonly workspace: Workspace;
  readonly model: string | null;
  readonly effort: string | null;
  readonly full: boolean;
}) {
  // Before the agent reports its session, the composer already knows what the turn runs on.
  const known = model === null && workspace.composer.agent === agent;
  const modelId = known ? workspace.composer.model : model;
  const effortId = known ? workspace.composer.effort : effort;
  const modelText = full ? modelLabel(workspace, agent, modelId) : "";
  const effortText = full ? effortLabel(workspace, agent, modelId, effortId) : "";
  return (
    <div className="o-byline o-t-ui">
      <span className="o-gutter" aria-hidden="true">
        <OrbitMark agent={agent} size={16} label={agentName(workspace, agent)} />
      </span>
      <span className="o-byline-text">
        <span className="o-w-strong o-c1">{agentName(workspace, agent)}</span>
        {modelText ? <span className="o-c2"> {modelText}</span> : null}
        {effortText ? <span className="o-c3"> {effortText}</span> : null}
      </span>
    </div>
  );
}

/** "Thought for 6s", or the summary the agent gave it; "Thinking · 4s" while it runs. */
const ThoughtLine = memo(function ThoughtLine({
  item,
  player,
  live,
  last,
  mark,
}: {
  readonly item: ThoughtItem;
  readonly player: Player;
  /** False once the turn was stopped: a thought cut short reads as finished. */
  readonly live: boolean;
  /** The thought is the turn's last item: it stays open until the next one arrives. */
  readonly last: boolean;
  /** The agent's live mark, in the gutter while this is the turn's live line. */
  readonly mark: ReactNode;
}) {
  const reduced = useReduced();
  const [toggled, setToggled] = useState<boolean | null>(null);
  const streaming = item.streaming && live;
  // Open while the thought arrives, and after, until the next item mounts, so the words being
  // read never vanish under the reader; folded once something follows it.
  const open = toggled ?? (streaming || (live && last));
  const { summary, body } = thoughtSummary(item.text);
  const measured = item.durationMs ?? (item.endedAt ?? player.getTime()) - item.startedAt;
  const seconds = Math.max(1, Math.round(measured / 1000));
  return (
    <div className="o-thought" data-open={open ? "" : undefined}>
      <button
        type="button"
        className="o-thought-line"
        aria-expanded={open}
        onClick={() => setToggled(!open)}
      >
        <span className="o-gutter">
          {mark && streaming ? (
            mark
          ) : (
            <ChevronRight
              className="o-thought-chev"
              size={12}
              strokeWidth={1.75}
              aria-hidden="true"
            />
          )}
        </span>
        {streaming ? (
          <span className="o-thought-label">
            <Shimmer>{summary ?? "Thinking"}</Shimmer>
            <span className="o-c3">
              {" · "}
              <Elapsed player={player} since={item.startedAt} />
            </span>
          </span>
        ) : summary ? (
          <span className="o-thought-label o-c3">
            {summary} · <span className="o-num">{seconds}s</span>
          </span>
        ) : (
          <span className="o-thought-label o-c3">
            Thought for <span className="o-num">{seconds}s</span>
          </span>
        )}
      </button>
      <AnimatePresence initial={false}>
        {open && body.trim() !== "" ? (
          <motion.div key="body" className="o-fold" {...foldMotion(reduced)}>
            <ThoughtBody body={body} />
          </motion.div>
        ) : null}
      </AnimatePresence>
    </div>
  );
});

/** An open thought: 280px at most, with a fade at the bottom only when more text is below. */
function ThoughtBody({ body }: { readonly body: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLParagraphElement>(null);
  const [more, setMore] = useState(false);
  useLayoutEffect(() => {
    const element = ref.current;
    const text = textRef.current;
    if (!element || !text) return;
    const measure = (): void =>
      setMore(element.scrollHeight - element.scrollTop - element.clientHeight > 1);
    measure();
    element.addEventListener("scroll", measure, { passive: true });
    // The text grows while the thought streams: measure again whenever its height changes.
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
    observer?.observe(text);
    return () => {
      element.removeEventListener("scroll", measure);
      observer?.disconnect();
    };
  }, []);
  return (
    <div ref={ref} className="o-thought-body o-scroll" data-more={more ? "" : undefined}>
      <p ref={textRef}>{body}</p>
    </div>
  );
}

/**
 * A centered one-line event (Grok Bot): agent switches, answers, stops, autonomy changes. The
 * entity it names is a chip: an agent's 14px mark (the smallest a mark is drawn) and its name in
 * ink, a literal in mono.
 */
export function SystemEvent({
  icon,
  text,
  agent,
  agentName: name,
  literal,
  detail,
}: {
  readonly icon: "check" | "x" | "stop" | "level" | "info" | null;
  readonly text: string;
  readonly agent?: string;
  readonly agentName?: string;
  readonly literal?: string;
  readonly detail?: string;
}) {
  return (
    <div className="o-event o-t-meta o-c3" role="note">
      {icon === "check" ? (
        <Check className="o-add" size={12} strokeWidth={2} aria-hidden="true" />
      ) : icon === "x" ? (
        <X className="o-del" size={12} strokeWidth={2} aria-hidden="true" />
      ) : icon === "stop" ? (
        <Pause size={11} strokeWidth={2} aria-hidden="true" />
      ) : icon === "info" ? (
        <Info size={12} strokeWidth={1.75} aria-hidden="true" />
      ) : null}
      <span className="o-event-text">
        {text}
        {agent ? (
          <span className="o-entity">
            <OrbitMark agent={agent} size={14} label={name ?? agent} />
            <span className="o-c1 o-w-strong">{name ?? agent}</span>
          </span>
        ) : null}
        {literal ? (
          <>
            <span className="o-c4"> · </span>
            <span className="o-event-literal">{literal}</span>
          </>
        ) : null}
        {detail ? (
          <>
            <span className="o-c4"> · </span>
            <span className="o-event-detail">{detail}</span>
          </>
        ) : null}
      </span>
    </div>
  );
}

/** The dashed warning notice (section 4.8): a fact that changes how the chat runs. */
export function WarningNotice({
  text,
  action,
  onAction,
}: {
  readonly text: string;
  readonly action: string | null;
  readonly onAction: () => void;
}) {
  return (
    <div className="o-notice o-t-meta" role="note">
      <span className="o-diamond" aria-hidden="true" />
      <span className="o-c2 o-notice-text">{text}</span>
      {action ? (
        <button type="button" className="o-text-btn o-t-meta" onClick={onAction}>
          {action}
        </button>
      ) : null}
    </div>
  );
}

/**
 * The turn error card (section 4.8, Warp's exit edge): where the turn died, what the agent said
 * last, and the ways on.
 */
function ErrorCard({
  notice,
  agent,
  onRetry,
  onContinue,
  onSoon,
}: {
  readonly notice: NoticeModel | null;
  readonly agent: string;
  readonly onRetry: (() => void) | null;
  readonly onContinue: (() => void) | null;
  readonly onSoon: (what: string) => void;
}) {
  const [summary = "stopped with an error", ...rest] = (notice?.message ?? "").split("\n");
  const detail = rest.join(" ").trim();
  return (
    <div className="o-error-card" role="alert">
      <div className="o-error-title o-t-ui">
        <X className="o-del" size={12} strokeWidth={2.25} aria-hidden="true" />
        <span className="o-c1 o-w-strong">
          {agent} stopped: {summary.charAt(0).toLowerCase() + summary.slice(1)}
        </span>
      </div>
      {detail ? <p className="o-error-detail o-t-mono o-c2">{detail}</p> : null}
      <div className="o-error-actions">
        <button
          type="button"
          className="o-btn o-btn-soft o-btn-sm"
          onClick={onRetry ?? undefined}
          disabled={onRetry === null}
        >
          <RotateCcw size={14} strokeWidth={1.5} aria-hidden="true" />
          Retry
        </button>
        <button
          type="button"
          className="o-btn o-btn-ghost o-btn-sm"
          onClick={() => onSoon("The agent log")}
        >
          <ScrollText size={14} strokeWidth={1.5} aria-hidden="true" />
          Open log
        </button>
        {onContinue ? (
          <button type="button" className="o-btn o-btn-ghost o-btn-sm" onClick={onContinue}>
            Continue with Codex
          </button>
        ) : null}
      </div>
    </div>
  );
}

/**
 * The live end of a running turn in the gaps between items (Grok Bot's pending row): the agent's
 * mark in the gutter and what it is doing, laid out exactly like the line that will replace it (a
 * thought line at the start of a turn, a tool row after one), so a gap reads as a status line
 * updating in place. In a gap after an item it waits a beat before showing, so it never flashes.
 */
export function NowRowView({
  now,
  player,
  timeline,
}: {
  readonly now: NowRow;
  readonly player: Player;
  readonly timeline: MarkTimeline;
}) {
  const t = usePlayhead(player, 100);
  if (t - now.since < now.delayMs) return null;
  const waiting = now.state === "waiting";
  return (
    <div className="o-now" data-state={now.state} aria-live="polite">
      <span className="o-gutter">
        <OrbitMark
          agent={now.agent}
          size={16}
          live={{ timeline, player }}
          label={`${now.verb}${now.object ? ` ${now.object}` : ""}`}
        />
      </span>
      <span className="o-now-text o-t-ui">
        {waiting ? (
          <span className="o-signal-text o-w-strong">
            {now.verb} · <Clock player={player} since={now.since} />
          </span>
        ) : (
          <>
            <span className="o-c2">
              <Shimmer>{now.verb}</Shimmer>
            </span>
            {now.object ? <span className="o-now-object"> {now.object}</span> : null}
            <span className="o-c3">
              {" · "}
              <Elapsed player={player} since={now.since} />
            </span>
          </>
        )}
      </span>
    </div>
  );
}

function Clock({ player, since }: { readonly player: Player; readonly since: number }) {
  const now = usePlayhead(player, 1000);
  return <span className="o-num">{formatClock(Math.max(0, now - since))}</span>;
}

/** X Money's receipt, Grok Bot's checklist: what the turn cost and what it changed. */
function Receipt({
  footer,
  latest,
  onOpenChanges,
  onCopy,
  onRetry,
  onEdit,
  onCopyReceipt,
  onSoon,
}: {
  readonly footer: TurnFooter;
  readonly latest: boolean;
  readonly onOpenChanges: () => void;
  readonly onCopy: (as: CopyAs) => void;
  readonly onRetry: (() => void) | null;
  readonly onEdit: (() => void) | null;
  readonly onCopyReceipt: () => void;
  readonly onSoon: (what: string) => void;
}) {
  const [menu, setMenu] = useState(false);
  const moreRef = useRef<HTMLButtonElement>(null);
  const usage = footer.usage;
  const cost =
    footer.cost && footer.cost.amount !== null ? `$${footer.cost.amount.toFixed(2)}` : null;
  const parts = [
    footer.stopReason === "cancelled"
      ? `Stopped after ${formatElapsed(footer.durationMs)}`
      : footer.stopReason === "error"
        ? `Failed after ${formatElapsed(footer.durationMs)}`
        : `Done in ${formatElapsed(footer.durationMs)}`,
    usage ? `${formatCount(usage.input_tokens)} in` : null,
    usage ? `${formatCount(usage.output_tokens)} out` : null,
    cost,
  ].filter((part): part is string => part !== null);
  return (
    <div
      className="o-receipt"
      data-latest={latest ? "" : undefined}
      data-open={menu ? "" : undefined}
    >
      <TearLine />
      <div className="o-receipt-row">
        <span className="o-receipt-facts o-t-meta o-c3 o-num">
          {parts.map((part, index) => (
            <span key={part}>
              {index > 0 ? <span className="o-c4"> · </span> : null}
              {part}
            </span>
          ))}
        </span>
        {footer.files.length > 0 ? (
          <button type="button" className="o-files-chip" onClick={onOpenChanges}>
            <FileDiff size={12} strokeWidth={1.5} aria-hidden="true" />
            <span className="o-t-meta o-c2">
              {footer.files.length} {footer.files.length === 1 ? "file" : "files"}
            </span>
            <DiffCounts added={footer.added} removed={footer.removed} />
            <DiffSquares added={footer.added} removed={footer.removed} />
          </button>
        ) : null}
        <span className="o-receipt-actions">
          <IconButton
            icon={Copy}
            label="Copy the answer"
            size={24}
            onClick={() => onCopy("plain")}
          />
          <IconButton
            icon={RotateCcw}
            label="Retry"
            size={24}
            onClick={onRetry ?? undefined}
            disabled={onRetry === null}
          />
          <IconButton
            icon={GitFork}
            label="Fork into a new chat"
            size={24}
            onClick={() => onSoon("Forking a chat")}
          />
          <span className="o-menu-anchor">
            <IconButton
              ref={moreRef}
              icon={MoreHorizontal}
              label="More"
              size={24}
              active={menu}
              aria-haspopup="menu"
              aria-expanded={menu}
              onClick={() => setMenu((value) => !value)}
            />
            {menu ? (
              <Menu
                label="Message"
                anchor={moreRef}
                side="top"
                onClose={() => setMenu(false)}
                groups={[
                  [
                    { label: "Copy", icon: Copy, onSelect: () => onCopy("plain") },
                    {
                      label: "Copy as Markdown",
                      icon: Copy,
                      onSelect: () => onCopy("markdown"),
                    },
                    { label: "Copy receipt", icon: ScrollText, onSelect: onCopyReceipt },
                  ],
                  [
                    {
                      label: "Retry",
                      icon: RotateCcw,
                      onSelect: onRetry ?? undefined,
                      soon: onRetry === null,
                    },
                    {
                      label: "Edit and resend",
                      icon: PenLine,
                      onSelect: onEdit ?? undefined,
                      soon: onEdit === null,
                    },
                    { label: "Fork into new chat", icon: GitFork, soon: true },
                  ],
                  [{ label: "Delete", icon: Trash2, destructive: true, soon: true }],
                ]}
              />
            ) : null}
          </span>
        </span>
      </div>
    </div>
  );
}

interface ItemProps {
  readonly player: Player;
  readonly live: boolean;
  readonly stopped: boolean;
  readonly denied: ReadonlySet<string>;
  readonly runFrom: ReadonlyMap<string, number>;
  readonly hideWaitingCall: boolean;
  readonly onOpenChanges: (path?: string) => void;
  readonly onCopyText: (text: string, done: string) => void;
}

function renderItem(item: TurnItem, props: ItemProps, mark: ReactNode, last: boolean): ReactNode {
  switch (item.kind) {
    case "thought":
      return (
        <ThoughtLine
          key={item.id}
          item={item}
          player={props.player}
          live={props.live}
          last={last}
          mark={mark}
        />
      );
    case "message":
      return (
        <div key={item.id} className="o-message">
          <Markdown text={item.text} streaming={item.streaming && props.live} />
        </div>
      );
    case "tools": {
      // While its card is up, a call that waits on the user is said once: by the card itself.
      const calls = props.hideWaitingCall
        ? item.calls.filter((call) => !call.awaitingPermission)
        : item.calls;
      if (calls.length === 0) return null;
      const run = calls.length === item.calls.length ? item : { ...item, calls };
      return (
        <ToolRun
          key={item.id}
          run={run}
          player={props.player}
          live={props.live}
          stopped={props.stopped}
          denied={props.denied}
          runFrom={props.runFrom}
          mark={mark}
          onOpenChanges={props.onOpenChanges}
          onCopy={props.onCopyText}
        />
      );
    }
    default:
      // A permission request is answered from the card in the composer's place.
      return null;
  }
}

/** The one item that is live right now, which carries the agent's mark in its gutter. */
function liveItemId(turn: Turn): string | null {
  const last = turn.items[turn.items.length - 1];
  if (!last) return null;
  if (last.kind === "thought" && last.streaming) return last.id;
  if (last.kind === "tools" && last.status === "running") return last.id;
  return null;
}

function renderExtra(extra: Extra, onSoon: (what: string) => void): ReactNode {
  switch (extra.kind) {
    case "event":
      return (
        <SystemEvent
          key={extra.id}
          icon={extra.icon}
          text={extra.text}
          agent={extra.agent}
          agentName={extra.agentName}
          literal={extra.literal}
          detail={extra.detail}
        />
      );
    case "steer":
      return <UserBlock key={extra.id} text={extra.text} tag={`Steered at ${extra.clock}`} />;
    default:
      return (
        <WarningNotice
          key={extra.id}
          text={extra.text}
          action={extra.action}
          onAction={() => onSoon(extra.action ?? "Settings")}
        />
      );
  }
}

/** One turn: the prompt, the byline, the work with what was added here, the receipt. */
const TurnView = memo(function TurnView({
  turn,
  workspace,
  player,
  timeline,
  live,
  plan,
  latest,
  fullByline,
  denied,
  runFrom,
  hideWaitingCall,
  extras,
  now,
  notice,
  sessionModel,
  sessionEffort,
  onOpenChanges,
  onRetry,
  onEdit,
  onCopy,
  onCopyReceipt,
  onCopyText,
  onContinue,
  onSoon,
}: {
  readonly turn: Turn;
  readonly workspace: Workspace;
  readonly player: Player;
  readonly timeline: MarkTimeline;
  readonly live: boolean;
  readonly plan: PlanModel | null;
  readonly latest: boolean;
  /** The byline names the model and effort: the first turn, or a change since the last. */
  readonly fullByline: boolean;
  readonly denied: ReadonlySet<string>;
  readonly runFrom: ReadonlyMap<string, number>;
  readonly hideWaitingCall: boolean;
  /** Lines the window added inside or after this turn. */
  readonly extras: readonly Extra[];
  /** The live end, when this is the running turn. */
  readonly now: NowRow | null;
  /** The error that ended this turn, when it failed. */
  readonly notice: NoticeModel | null;
  readonly sessionModel: string | null;
  readonly sessionEffort: string | null;
  readonly onOpenChanges: (path?: string) => void;
  readonly onRetry: (text: string) => void;
  readonly onEdit: (text: string) => void;
  readonly onCopy: (turn: Turn, as: CopyAs) => void;
  readonly onCopyReceipt: (turn: Turn) => void;
  readonly onCopyText: (text: string, done: string) => void;
  readonly onContinue: (() => void) | null;
  readonly onSoon: (what: string) => void;
}) {
  const agent = (turn.agent ?? "generic") as AgentBrandId;
  // A finished plan joins the turn where it finished: before the first item after its last update.
  const showPlan = plan !== null && plan.turnId === turn.id && (plan.done || !live);
  const planAt = showPlan
    ? turn.items.findIndex((item) => "startedAt" in item && item.startedAt > plan.updatedAt)
    : -1;
  const liveId = live ? liveItemId(turn) : null;
  const mark = live ? (
    <OrbitMark
      agent={agent}
      size={16}
      live={{ timeline, player }}
      label={`${agentName(workspace, agent)}, working`}
    />
  ) : null;
  const itemProps: ItemProps = {
    player,
    live,
    stopped: turn.status === "cancelled",
    denied,
    runFrom,
    hideWaitingCall,
    onOpenChanges,
    onCopyText,
  };
  const items: ReactNode[] = [];
  const at = (index: number) =>
    extras.filter((extra) => extra.anchor.after !== "end" && extra.anchor.after === index);
  turn.items.forEach((item, index) => {
    for (const extra of at(index)) items.push(renderExtra(extra, onSoon));
    if (showPlan && index === planAt && plan) items.push(<PlanRow key="plan" plan={plan} />);
    items.push(
      renderItem(
        item,
        itemProps,
        item.id === liveId ? mark : null,
        index === turn.items.length - 1,
      ),
    );
  });
  for (const extra of extras) {
    if (extra.anchor.after !== "end" && extra.anchor.after >= turn.items.length) {
      items.push(renderExtra(extra, onSoon));
    }
  }
  if (showPlan && planAt === -1 && plan) items.push(<PlanRow key="plan" plan={plan} />);
  if (turn.status === "failed") {
    items.push(
      <ErrorCard
        key="error"
        notice={notice}
        agent={agentName(workspace, agent)}
        onRetry={turn.user ? () => onRetry(turn.user?.text ?? "") : null}
        onContinue={agent === "codex" ? null : onContinue}
        onSoon={onSoon}
      />,
    );
  }
  if (now) items.push(<NowRowView key="now" now={now} player={player} timeline={timeline} />);
  const after = extras.filter((extra) => extra.anchor.after === "end");
  const prompt = turn.user?.text ?? null;
  return (
    <>
      <section className="o-turn" aria-label={prompt ?? "Agent turn"} data-turn={turn.id}>
        {turn.user ? <UserBlock text={turn.user.text} /> : null}
        <Byline
          agent={agent}
          workspace={workspace}
          model={turn.footer?.model ?? sessionModel}
          effort={turn.footer?.effort ?? sessionEffort}
          full={fullByline}
        />
        <div className="o-turn-items">{items}</div>
        {turn.footer && turn.status !== "failed" ? (
          <Receipt
            footer={turn.footer}
            latest={latest}
            onOpenChanges={() => onOpenChanges()}
            onCopy={(as) => onCopy(turn, as)}
            onRetry={prompt !== null ? () => onRetry(prompt) : null}
            onEdit={prompt !== null ? () => onEdit(prompt) : null}
            onCopyReceipt={() => onCopyReceipt(turn)}
            onSoon={onSoon}
          />
        ) : null}
      </section>
      {after.length > 0 ? (
        <div className="o-after">{after.map((extra) => renderExtra(extra, onSoon))}</div>
      ) : null}
    </>
  );
});

/**
 * Claude's sticky prompt: while the reader is below the turn's message, it pins under the header
 * as one ink line; clicking it scrolls back up to the message. Hidden, it is out of the tab order
 * and takes no clicks.
 */
function StickyPrompt({
  scrollRef,
  turnId,
  text,
}: {
  readonly scrollRef: { readonly current: HTMLElement | null };
  readonly turnId: string;
  readonly text: string;
}) {
  const [shown, setShown] = useState(false);
  useEffect(() => {
    const root = scrollRef.current;
    const block = root?.querySelector(`[data-turn="${turnId}"] .o-user-block`);
    if (!root || !block || typeof IntersectionObserver !== "function") return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry) return;
        const above = entry.boundingClientRect.bottom < (entry.rootBounds?.top ?? 0) + 1;
        setShown(!entry.isIntersecting && above);
      },
      { root, threshold: 0 },
    );
    observer.observe(block);
    return () => observer.disconnect();
  }, [scrollRef, turnId]);
  return (
    <div className="o-sticky" data-shown={shown ? "" : undefined} aria-hidden={!shown}>
      <button
        type="button"
        className="o-sticky-prompt o-t-ui"
        tabIndex={shown ? 0 : -1}
        onClick={() => {
          const root = scrollRef.current;
          const turn = root?.querySelector<HTMLElement>(`[data-turn="${turnId}"]`);
          if (root && turn) root.scrollTo({ top: turn.offsetTop - 24, behavior: "smooth" });
        }}
      >
        {text}
      </button>
    </div>
  );
}

/**
 * A chat whose history the lab does not carry (it has no recording): its title as the prompt, one
 * folded line for the work it did, and a centered line that says what is missing. Better than
 * pretending the chat is new.
 */
function HistoryStub({
  chat,
  workspace,
}: {
  readonly chat: ChatSummary;
  readonly workspace: Workspace;
}) {
  const agent = chat.agent;
  return (
    <section className="o-turn" aria-label={chat.title} data-turn="history">
      <UserBlock text={chat.title} />
      <Byline agent={agent} workspace={workspace} model={null} effort={null} full={false} />
      <div className="o-turn-items">
        <div className="o-call">
          <div className="o-tool-row" data-static="">
            <span className="o-gutter">
              <History className="o-tool-icon" size={14} strokeWidth={1.5} aria-hidden="true" />
            </span>
            <span className="o-tool-text o-t-ui">
              <span className="o-tool-verb">Earlier work</span>
              <span className="o-tool-result">{chat.updated}</span>
            </span>
            <span className="o-tool-side">
              {chat.changes ? (
                <>
                  <DiffCounts added={chat.changes.added} removed={chat.changes.removed} size={11} />
                  <DiffSquares added={chat.changes.added} removed={chat.changes.removed} />
                </>
              ) : null}
            </span>
          </div>
        </div>
        <SystemEvent icon="info" text="This chat's history is not in the lab" />
      </div>
    </section>
  );
}

export interface TranscriptProps {
  readonly model: TranscriptModel;
  readonly workspace: Workspace;
  readonly player: Player;
  readonly timeline: MarkTimeline;
  /** The turn still running or waiting, or null. */
  readonly liveTurnId: string | null;
  readonly extras: readonly Extra[];
  readonly now: NowRow | null;
  /** The permission card is up: the call it asks about is not repeated in the transcript. */
  readonly hideWaitingCall: boolean;
  readonly denied: ReadonlySet<string>;
  /** Calls that waited on an answer, and when they actually began to run. */
  readonly runFrom: ReadonlyMap<string, number>;
  /** An opened chat whose earlier turns the lab does not carry. */
  readonly history: ChatSummary | null;
  readonly scrollRef: { readonly current: HTMLElement | null };
  readonly onOpenChanges: (path?: string) => void;
  readonly onRetry: (text: string) => void;
  readonly onEdit: (text: string) => void;
  readonly onCopy: (turn: Turn, as: CopyAs) => void;
  readonly onCopyReceipt: (turn: Turn) => void;
  readonly onCopyText: (text: string, done: string) => void;
  readonly onContinue?: (() => void) | null;
  readonly onSoon: (what: string) => void;
}

/** The chat: every turn, with what happened here placed where it happened. */
export function Transcript({
  model,
  workspace,
  player,
  timeline,
  liveTurnId,
  extras,
  now,
  hideWaitingCall,
  denied,
  runFrom,
  history,
  scrollRef,
  onOpenChanges,
  onRetry,
  onEdit,
  onCopy,
  onCopyReceipt,
  onCopyText,
  onContinue = null,
  onSoon,
}: TranscriptProps) {
  // Grouped once per change, so a turn whose lines did not change keeps the same array.
  const byTurn = useMemo(() => {
    const groups = new Map<string | null, Extra[]>();
    for (const extra of extras) {
      const group = groups.get(extra.anchor.turnId) ?? [];
      group.push(extra);
      groups.set(extra.anchor.turnId, group);
    }
    return groups;
  }, [extras]);
  const first = byTurn.get(null) ?? NO_EXTRAS;
  const lastWithPrompt = [...model.turns].reverse().find((turn) => turn.user !== null) ?? null;
  return (
    <div className="o-transcript">
      {lastWithPrompt?.user ? (
        <StickyPrompt
          key={lastWithPrompt.id}
          scrollRef={scrollRef}
          turnId={lastWithPrompt.id}
          text={lastWithPrompt.user.text}
        />
      ) : null}
      {history ? <HistoryStub chat={history} workspace={workspace} /> : null}
      {first.length > 0 ? (
        <div className="o-after">{first.map((extra) => renderExtra(extra, onSoon))}</div>
      ) : null}
      {model.turns.map((turn, index) => {
        const last = index === model.turns.length - 1;
        const live = turn.id === liveTurnId;
        const previous = index > 0 ? model.turns[index - 1] : null;
        const modelOf = (entry: Turn) => entry.footer?.model ?? model.session.model;
        const effortOf = (entry: Turn) => entry.footer?.effort ?? model.session.effort;
        const fullByline =
          !previous ||
          previous.agent !== turn.agent ||
          modelOf(previous) !== modelOf(turn) ||
          effortOf(previous) !== effortOf(turn);
        return (
          <TurnView
            key={turn.id}
            turn={turn}
            workspace={workspace}
            player={player}
            timeline={timeline}
            live={live}
            plan={model.plan}
            latest={last}
            fullByline={fullByline}
            denied={denied}
            runFrom={runFrom}
            hideWaitingCall={live && hideWaitingCall}
            extras={byTurn.get(turn.id) ?? NO_EXTRAS}
            now={live ? now : null}
            notice={
              turn.status === "failed"
                ? ([...model.notices]
                    .reverse()
                    .find((notice) => notice.turnId === turn.id && notice.level === "error") ??
                  null)
                : null
            }
            sessionModel={model.session.model}
            sessionEffort={model.session.effort}
            onOpenChanges={onOpenChanges}
            onRetry={onRetry}
            onEdit={onEdit}
            onCopy={onCopy}
            onCopyReceipt={onCopyReceipt}
            onCopyText={onCopyText}
            onContinue={onContinue}
            onSoon={onSoon}
          />
        );
      })}
    </div>
  );
}
