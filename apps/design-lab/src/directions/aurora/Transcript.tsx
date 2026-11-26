import {
  type AgentBrandId,
  brandCssVariables,
  getBrand,
  Orb,
  type OrbState,
  springs,
  usePrefersReducedMotion,
} from "@ghost/ui";
import {
  ArrowDown,
  ChevronDown,
  CircleAlert,
  CircleCheck,
  CircleStop,
  Copy,
  FilePenLine,
  GitFork,
  Info,
  ListChecks,
  Pencil,
  RotateCcw,
  TriangleAlert,
  Zap,
} from "lucide-react";
import { motion } from "motion/react";
import {
  type CSSProperties,
  memo,
  type ReactNode,
  type RefObject,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  type Autonomy,
  basename,
  dirname,
  effortLabel,
  formatCost,
  formatCount,
  formatDuration,
  modelLabel,
  type NoticeModel,
  type PermissionModel,
  type PermissionOption,
  type PlanModel,
  type Player,
  type SessionModel,
  type StopReason,
  type ToolRunItem,
  type TranscriptModel,
  type Turn,
  type TurnFooter,
  type TurnItem,
  usePlayhead,
  type Workspace,
} from "../../kit";
import { Glass } from "./Glass";
import { type Handoff, HandoffOrbs, HandoffView, type SentMessage } from "./Handoff";
import { Markdown } from "./Markdown";
import { ANSWER_TEXT, PermissionCard, PermissionDone } from "./Permission";
import { PlanSteps } from "./Plan";
import { LiveElapsed, ThoughtView } from "./Thought";
import { type RunAnswer, ToolRun } from "./ToolRun";
import { Collapse, DiffStat, IconButton, useBornLive } from "./ui";
import { useStickToBottom } from "./useStickToBottom";

/** Glow strength behind the orb per state, before the stream adds its breath. */
const GLOW_BASE: Readonly<Record<OrbState, number>> = {
  idle: 0.22,
  thinking: 0.72,
  streaming: 0.6,
  tool: 0.78,
  waiting: 0.7,
  error: 0.45,
  done: 0.3,
};

/**
 * The light behind the head orb. It breathes with the stream: every event that lands brightens it
 * a little, decaying over half a second, all read from the playhead, so a frozen frame is always
 * the same frame.
 */
function StreamGlow({
  player,
  lastEventAt,
  state,
}: {
  readonly player: Player;
  readonly lastEventAt: number | null;
  readonly state: OrbState;
}) {
  const reduced = usePrefersReducedMotion();
  const now = usePlayhead(player, reduced ? 1000 : 50);
  const working = state === "thinking" || state === "streaming" || state === "tool";
  const since = lastEventAt === null ? Number.POSITIVE_INFINITY : Math.max(0, now - lastEventAt);
  const breath = working && !reduced ? Math.exp(-since / 420) : 0;
  const energy = Math.min(1, GLOW_BASE[state] + breath * 0.3);
  return (
    <span
      className="au-glow"
      data-state={state}
      style={{
        opacity: energy,
        transform: reduced ? undefined : `scale(${(0.8 + energy * 0.34).toFixed(3)})`,
      }}
      aria-hidden="true"
    />
  );
}

/** The agent's permission mode, as the autonomy level it means. Null when the name is unknown. */
export function autonomyOfMode(mode: string | null): Autonomy | null {
  switch (mode) {
    case "bypassPermissions":
    case "never":
    case "full_auto":
      return "full_auto";
    case "acceptEdits":
    case "on-request":
    case "on-failure":
    case "auto_edit":
      return "auto_edit";
    case "default":
    case "untrusted":
    case "ask":
      return "ask";
    default:
      return null;
  }
}

const STOP_TEXT: Readonly<Record<StopReason, string>> = {
  end_turn: "Done in",
  max_tokens: "Stopped at the token limit after",
  max_turn_requests: "Stopped at the request limit after",
  refusal: "Declined after",
  cancelled: "Stopped after",
  error: "Failed after",
};

function TurnFooterView({
  footer,
  text,
  plan,
}: {
  readonly footer: TurnFooter;
  readonly text: string;
  /** The plan this turn followed, if it sent one. */
  readonly plan: PlanModel | null;
}) {
  const [open, setOpen] = useState<"files" | "plan" | null>(null);
  const usage = footer.usage;
  const tokens = usage ? usage.input_tokens + usage.output_tokens : null;
  const tokenTip = usage
    ? [
        `${formatCount(usage.input_tokens)} in`,
        `${formatCount(usage.output_tokens)} out`,
        usage.cached_input_tokens ? `${formatCount(usage.cached_input_tokens)} cached` : null,
      ]
        .filter(Boolean)
        .join(" · ")
    : "";
  const cost = footer.cost ? formatCost(footer.cost.amount, footer.cost.currency) : null;
  const ok = footer.stopReason === "end_turn";
  const StopIcon = ok ? CircleCheck : footer.stopReason === "cancelled" ? CircleStop : CircleAlert;
  const toggle = (which: "files" | "plan"): void => setOpen(open === which ? null : which);
  return (
    <div className="au-footer">
      <div className="au-footer-row">
        <span className="au-foot" data-tone={ok ? "ok" : "warn"}>
          <StopIcon className="au-foot-icon" size={13} strokeWidth={2} />
          {STOP_TEXT[footer.stopReason]} {formatDuration(footer.durationMs)}
        </span>
        {tokens !== null ? (
          <span className="au-foot" data-tip={tokenTip} data-tip-side="top">
            {formatCount(tokens)} tokens
          </span>
        ) : null}
        {cost ? <span className="au-foot">{cost}</span> : null}
        {footer.files.length > 0 ? (
          <button
            type="button"
            className="au-files-chip"
            aria-expanded={open === "files"}
            onClick={() => toggle("files")}
          >
            <FilePenLine size={13} strokeWidth={2} />
            {footer.files.length === 1 ? "1 file changed" : `${footer.files.length} files changed`}
            <DiffStat added={footer.added} removed={footer.removed} />
          </button>
        ) : null}
        {plan ? (
          <button
            type="button"
            className="au-files-chip"
            aria-expanded={open === "plan"}
            onClick={() => toggle("plan")}
          >
            <ListChecks size={13} strokeWidth={2} />
            Plan {plan.completed} of {plan.total}
          </button>
        ) : null}
        <span className="au-run-spacer" />
        <span className="au-turn-actions">
          <IconButton
            label="Copy"
            tip="top"
            onClick={() => void navigator.clipboard?.writeText(text).catch(() => {})}
          >
            <Copy size={14} strokeWidth={1.75} />
          </IconButton>
          <IconButton label="Retry" tip="top">
            <RotateCcw size={14} strokeWidth={1.75} />
          </IconButton>
          <IconButton label="Fork into a new chat" tip="top">
            <GitFork size={14} strokeWidth={1.75} />
          </IconButton>
        </span>
      </div>
      <Collapse open={open === "files"}>
        <ul className="au-files">
          {footer.files.map((file) => (
            <li key={file.path} className="au-file">
              <FilePenLine size={13} strokeWidth={2} />
              <span className="au-file-path">
                <span className="au-diff-dir">{dirname(file.path)}/</span>
                <span className="au-diff-base">{basename(file.path)}</span>
              </span>
              {file.isNew ? <span className="au-badge">New</span> : null}
              <span className="au-run-spacer" />
              <DiffStat added={file.added} removed={file.removed} />
            </li>
          ))}
        </ul>
      </Collapse>
      {plan ? (
        <Collapse open={open === "plan"}>
          <div className="au-plan-inline">
            <PlanSteps plan={plan} />
          </div>
        </Collapse>
      ) : null}
    </div>
  );
}

/** A sent message's handoff, folded to one line inside its bubble. */
function BubbleHandoff({ handoff }: { readonly handoff: Handoff }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="au-user-handoff">
      <button
        type="button"
        className="au-handoff-toggle"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <HandoffOrbs from={handoff.from} to={handoff.to} />
        <span className="au-strip-text">Handoff from {getBrand(handoff.from).name}</span>
        <span className="au-run-spacer" />
        <ChevronDown className="au-chevron" data-open={open ? "" : undefined} size={14} />
      </button>
      <Collapse open={open}>
        <HandoffView handoff={handoff} />
      </Collapse>
    </div>
  );
}

function UserBubble({
  text,
  handoff = null,
}: {
  readonly text: string;
  readonly handoff?: Handoff | null;
}) {
  const live = useBornLive();
  return (
    <motion.div
      className="au-user"
      initial={live ? { opacity: 0, y: 14, scale: 0.98 } : false}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={springs.smooth}
    >
      <Glass level={2} radius={16} className="au-user-bubble">
        <p className="au-user-text">{text}</p>
        {handoff ? <BubbleHandoff handoff={handoff} /> : null}
      </Glass>
      <div className="au-user-actions">
        <IconButton
          label="Copy"
          tip="bottom"
          onClick={() => void navigator.clipboard?.writeText(text).catch(() => {})}
        >
          <Copy size={13} strokeWidth={2} />
        </IconButton>
        <IconButton label="Edit and resend" tip="bottom">
          <Pencil size={13} strokeWidth={2} />
        </IconButton>
      </div>
    </motion.div>
  );
}

/** A local answer to a pending request: the recording never answers, so the window plays it on. */
export interface PendingAnswer {
  readonly requestId: string;
  readonly option: PermissionOption;
}

interface ItemContext {
  readonly player: Player;
  readonly agentName: string;
  readonly autonomy: Autonomy;
  readonly typing: boolean;
  readonly onAnswer: (option: PermissionOption) => void;
}

/**
 * How a turn's permission cards and the runs they hold share the space: while a request waits, its
 * card stands in for the run; once answered, the card goes and the run comes back with the answer
 * as a chip. A request with no run in the turn keeps a one-line answered card instead.
 */
export interface Arrangement {
  readonly hiddenRuns: ReadonlySet<string>;
  readonly runAnswers: ReadonlyMap<string, RunAnswer>;
  readonly cards: ReadonlyMap<string, "card" | "done" | "none">;
  readonly cardAnswers: ReadonlyMap<string, PermissionOption | null>;
  /** Items that can appear and disappear in place (they reveal with a height spring). */
  readonly revealable: ReadonlySet<string>;
}

export function arrange(turn: Turn, pendingAnswer: PendingAnswer | null): Arrangement {
  const runOfCall = new Map<string, ToolRunItem>();
  for (const item of turn.items) {
    if (item.kind === "tools") for (const call of item.calls) runOfCall.set(call.id, item);
  }
  const hiddenRuns = new Set<string>();
  const runAnswers = new Map<string, RunAnswer>();
  const cards = new Map<string, "card" | "done" | "none">();
  const cardAnswers = new Map<string, PermissionOption | null>();
  const revealable = new Set<string>();
  for (const item of turn.items) {
    if (item.kind !== "permission") continue;
    const permission: PermissionModel = item.permission;
    revealable.add(`permission:${item.id}`);
    const local =
      permission.status === "pending" && pendingAnswer?.requestId === permission.requestId
        ? pendingAnswer.option
        : null;
    const resolved =
      permission.status === "allowed" || permission.status === "rejected"
        ? (permission.options.find((option) => option.option_id === permission.selectedOptionId) ??
          null)
        : null;
    const answer = local ?? resolved;
    const run = permission.toolCallId === null ? undefined : runOfCall.get(permission.toolCallId);
    if (run) revealable.add(`tools:${run.id}`);
    cardAnswers.set(item.id, answer);
    if (permission.status === "pending" && local === null) {
      cards.set(item.id, "card");
      // The card stands in for a run that only holds this request.
      if (run?.calls.every((call) => call.awaitingPermission)) hiddenRuns.add(run.id);
    } else if (run) {
      cards.set(item.id, "none");
      if (answer) {
        runAnswers.set(run.id, {
          allowed: answer.kind.startsWith("allow"),
          label: ANSWER_TEXT[answer.kind],
          local: local !== null,
        });
      }
    } else {
      cards.set(item.id, permission.status === "cancelled" && !answer ? "none" : "done");
    }
  }
  return { hiddenRuns, runAnswers, cards, cardAnswers, revealable };
}

function ItemView({
  item,
  context,
  arrangement,
}: {
  readonly item: TurnItem;
  readonly context: ItemContext;
  readonly arrangement: Arrangement;
}) {
  const live = useBornLive();
  let content: ReactNode;
  let visible = true;
  switch (item.kind) {
    case "thought":
      content = <ThoughtView item={item} player={context.player} />;
      break;
    case "message":
      content = <Markdown text={item.text} streaming={item.streaming} caret={item.streaming} />;
      break;
    case "tools":
      visible = !arrangement.hiddenRuns.has(item.id);
      content = (
        <ToolRun
          item={item}
          player={context.player}
          answer={arrangement.runAnswers.get(item.id) ?? null}
        />
      );
      break;
    case "permission": {
      const mode = arrangement.cards.get(item.id) ?? "card";
      visible = mode !== "none";
      content =
        mode === "done" ? (
          <PermissionDone
            permission={item.permission}
            answer={arrangement.cardAnswers.get(item.id) ?? null}
          />
        ) : (
          <PermissionCard
            permission={item.permission}
            agentName={context.agentName}
            autonomy={context.autonomy}
            typing={context.typing}
            onAnswer={context.onAnswer}
          />
        );
      break;
    }
  }
  if (arrangement.revealable.has(`${item.kind}:${item.id}`)) {
    // Comes and goes in place: the height springs, so what follows glides instead of jumping.
    return (
      <Collapse open={visible} appear={live} className="au-item au-reveal">
        <div className="au-reveal-pad" data-kind={item.kind}>
          {content}
        </div>
      </Collapse>
    );
  }
  // Text blocks rise in on their own (block by block); everything else rises in as a whole.
  const rises = live && item.kind !== "message";
  return (
    <motion.div
      className="au-item"
      data-kind={item.kind}
      initial={rises ? { opacity: 0, y: 10 } : false}
      animate={{ opacity: 1, y: 0 }}
      transition={springs.smooth}
    >
      {content}
    </motion.div>
  );
}

function turnText(turn: Turn): string {
  return turn.items
    .filter((item) => item.kind === "message")
    .map((item) => item.text)
    .join("\n\n");
}

const NO_NOTICES: readonly NoticeModel[] = [];

/** What the agent said about the session itself (a notice, a warning, an error): a quiet system row. */
function NoticeRow({ notice }: { readonly notice: NoticeModel }) {
  const Icon =
    notice.level === "error" ? CircleAlert : notice.level === "warning" ? TriangleAlert : Info;
  return (
    <div
      className="au-notice"
      data-level={notice.level}
      role={notice.level === "error" ? "alert" : undefined}
    >
      <Icon className="au-notice-icon" size={14} strokeWidth={1.75} />
      <span>{notice.message}</span>
    </div>
  );
}

/** The orb in its gutter, with its glow and the Full auto badge on its rim. */
function GutterOrb({
  agent,
  state,
  autonomy,
  glow,
}: {
  readonly agent: AgentBrandId;
  readonly state: OrbState;
  readonly autonomy: Autonomy;
  readonly glow: ReactNode;
}) {
  return (
    <div className="au-gutter">
      <div className="au-gutter-orb">
        {glow}
        <Orb brand={agent} state={state} size={40} />
        {autonomy === "full_auto" ? (
          <span className="au-orb-badge" data-tip="Full auto" data-tip-side="right">
            <Zap size={9} strokeWidth={2.6} fill="currentColor" />
          </span>
        ) : null}
      </div>
    </div>
  );
}

/** A turn's own colours: its agent's brand, whatever the composer has selected now. */
function turnStyle(agent: AgentBrandId): CSSProperties {
  const brand = getBrand(agent);
  return { ...brandCssVariables(brand), "--au-turn-glow": brand.glow } as CSSProperties;
}

const TurnView = memo(function TurnView({
  turn,
  notices,
  orbState,
  session,
  workspace,
  player,
  lastEventAt,
  autonomy,
  pendingAnswer,
  typing,
  onAnswer,
  active,
  plan,
}: {
  readonly turn: Turn;
  /** Notices that arrived during this turn, in order. */
  readonly notices: readonly NoticeModel[];
  readonly orbState: OrbState;
  readonly session: SessionModel | null;
  readonly workspace: Workspace;
  readonly player: Player;
  /** Only for the turn that is running; null for the others, so they never re-render on events. */
  readonly lastEventAt: number | null;
  /** The turn's own autonomy (from its session mode), never the composer's current one. */
  readonly autonomy: Autonomy;
  readonly pendingAnswer: PendingAnswer | null;
  /** The composer holds text (the card's hints change). Only the last turn hears it. */
  readonly typing: boolean;
  readonly onAnswer: (option: PermissionOption) => void;
  readonly active: boolean;
  /** The plan this turn sent, for its footer, once the turn has ended. */
  readonly plan: PlanModel | null;
}) {
  const agent = turn.agent ?? "generic";
  const brand = getBrand(agent);
  const modelId = turn.footer?.model ?? session?.model ?? null;
  const effortId = turn.footer?.effort ?? session?.effort ?? null;
  const tag = [
    modelLabel(workspace, brand.id, modelId),
    effortLabel(workspace, brand.id, modelId, effortId),
  ]
    .filter((part) => part !== "")
    .join(" · ");
  const answered = turn.status === "waiting" ? pendingAnswer : null;
  const allowedHere = answered?.option.kind.startsWith("allow") ?? false;
  const running = turn.status === "running" || (turn.status === "waiting" && allowedHere);
  const waiting = turn.status === "waiting" && answered === null;
  // While a thought streams, its pill counts the seconds; one counter on screen is enough.
  const lastItem = turn.items[turn.items.length - 1];
  const thinkingNow = lastItem?.kind === "thought" && lastItem.streaming;
  const arrangement = useMemo(() => arrange(turn, pendingAnswer), [turn, pendingAnswer]);
  const context: ItemContext = {
    player,
    agentName: brand.name,
    autonomy,
    typing,
    onAnswer,
  };

  return (
    <section className="au-turn" data-status={turn.status}>
      {turn.user ? <UserBubble text={turn.user.text} /> : null}
      <div className="au-agent" style={turnStyle(agent)}>
        <GutterOrb
          agent={agent}
          state={orbState}
          autonomy={autonomy}
          glow={
            active ? (
              <StreamGlow player={player} lastEventAt={lastEventAt} state={orbState} />
            ) : (
              <span className="au-glow" data-state={orbState} aria-hidden="true" />
            )
          }
        />
        <div className="au-agent-body">
          <div className="au-head">
            <span className="au-head-name">{brand.name}</span>
            {tag ? <span className="au-head-tag">{tag}</span> : null}
            {waiting ? (
              <span className="au-head-status" data-tone="amber">
                Waiting for you
              </span>
            ) : running && !thinkingNow ? (
              <span className="au-head-status">
                <span className="au-live-dot" aria-hidden="true" />
                <LiveElapsed player={player} since={turn.startedAt} />
              </span>
            ) : null}
          </div>
          <div className="au-items">
            {turn.items.map((item) => (
              <ItemView
                key={`${item.kind}:${item.id}`}
                item={item}
                context={context}
                arrangement={arrangement}
              />
            ))}
          </div>
          {notices.length > 0 ? (
            <div className="au-notices">
              {notices.map((notice) => (
                <NoticeRow key={`${notice.at}:${notice.message}`} notice={notice} />
              ))}
            </div>
          ) : null}
          {turn.footer ? (
            <TurnFooterView footer={turn.footer} text={turnText(turn)} plan={plan} />
          ) : null}
        </div>
      </div>
    </section>
  );
});

/**
 * A message sent from this window. The recording cannot answer it, so its turn shows the agent
 * starting, until Stop ends it.
 */
const SentTurnView = memo(function SentTurnView({
  message,
  workspace,
  stopped,
}: {
  readonly message: SentMessage;
  readonly workspace: Workspace;
  readonly stopped: boolean;
}) {
  const brand = getBrand(message.agent);
  const tag = [
    modelLabel(workspace, brand.id, message.model),
    effortLabel(workspace, brand.id, message.model, message.effort),
  ]
    .filter((part) => part !== "")
    .join(" · ");
  const state: OrbState = stopped ? "idle" : "thinking";
  return (
    <section
      className="au-turn"
      data-status={stopped ? "cancelled" : "running"}
      data-sent-id={message.id}
    >
      <UserBubble text={message.text} handoff={message.handoff} />
      <div
        className="au-agent"
        style={turnStyle(message.agent)}
        // The first message of a new chat flies its orb in from the hero (see App).
        data-enter={message.fromHero ? undefined : ""}
        data-launch={message.fromHero ? "" : undefined}
      >
        <GutterOrb
          agent={message.agent}
          state={state}
          autonomy={message.autonomy}
          glow={<span className="au-glow" data-state={state} aria-hidden="true" />}
        />
        <motion.div
          className="au-agent-body"
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ ...springs.smooth, delay: 0.14 }}
        >
          <div className="au-head">
            <span className="au-head-name">{brand.name}</span>
            {tag ? <span className="au-head-tag">{tag}</span> : null}
            {stopped ? null : (
              <span className="au-head-status">
                <span className="au-live-dot" aria-hidden="true" />
                <span className="au-shimmer">Starting</span>
              </span>
            )}
          </div>
          {stopped ? (
            <div className="au-footer">
              <div className="au-footer-row">
                <span className="au-foot" data-tone="warn">
                  <CircleStop className="au-foot-icon" size={13} strokeWidth={2} />
                  Stopped
                </span>
              </div>
            </div>
          ) : null}
        </motion.div>
      </div>
    </section>
  );
});

/** The bottom of the transcript hidden behind the composer dock and its fade (see aurora.css). */
function coveredBottom(scroller: HTMLElement): number {
  const sheet = scroller.closest<HTMLElement>(".au-sheet");
  const dock = Number.parseFloat(sheet?.style.getPropertyValue("--au-dock-h") ?? "");
  return (Number.isFinite(dock) ? dock : 128) + 20 + 26;
}

const SCROLL_KEYS = new Set(["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "]);

/** What the composer can do to the transcript from the keyboard (PageUp, Command-arrows). */
export interface TranscriptKeys {
  /** Scrolls a page up (-1) or down (1). */
  page(direction: -1 | 1): void;
  /** Goes to the start (-1) or the newest output (1). */
  edge(direction: -1 | 1): void;
}

/**
 * The conversation: turns in a scrolling column that glides to the newest output while you are at
 * the end. The head orb of each turn sits in a gutter of its own and stays in view while its turn
 * scrolls past. Messages sent from this window follow the recorded turns.
 */
export function Transcript({
  model,
  workspace,
  player,
  activity,
  pendingAnswer,
  typing,
  onAnswer,
  sent,
  stoppedIds,
  keysRef,
}: {
  readonly model: TranscriptModel;
  readonly workspace: Workspace;
  readonly player: Player;
  /** The orb state of the last turn (the model's activity, or a local override). */
  readonly activity: OrbState;
  readonly pendingAnswer: PendingAnswer | null;
  readonly typing: boolean;
  readonly onAnswer: (option: PermissionOption) => void;
  readonly sent: readonly SentMessage[];
  /** Sent messages whose turn the user stopped. */
  readonly stoppedIds: ReadonlySet<string>;
  readonly keysRef: RefObject<TranscriptKeys | null>;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const columnRef = useRef<HTMLDivElement>(null);
  const reduced = usePrefersReducedMotion();
  const { pinned, toLatest, release } = useStickToBottom(
    scrollRef,
    columnRef,
    reduced,
    coveredBottom,
  );
  const lastIndex = model.turns.length - 1;
  // Notices by the turn they arrived in; ones outside any turn close the column.
  const notices = useMemo(() => {
    const byTurn = new Map<string | null, NoticeModel[]>();
    for (const notice of model.notices) {
      const list = byTurn.get(notice.turnId) ?? [];
      list.push(notice);
      byTurn.set(notice.turnId, list);
    }
    return byTurn;
  }, [model.notices]);
  const loose = notices.get(null) ?? NO_NOTICES;

  // Each turn keeps the autonomy its session reported while it ran, so changing the composer's
  // autonomy (or switching to an agent that has only one) never rewrites history.
  const modes = useRef(new Map<string, Autonomy>());
  const sessionAutonomy = autonomyOfMode(model.session.mode);
  const fallbackAutonomy = workspace.composer.autonomy;
  const keyOf = (turn: Turn): string => turn.user?.id ?? turn.id;
  const lastTurn = model.turns[lastIndex];
  if (lastTurn && sessionAutonomy !== null && !modes.current.has(keyOf(lastTurn))) {
    modes.current.set(keyOf(lastTurn), sessionAutonomy);
  }

  useEffect(() => {
    keysRef.current = {
      page: (direction) => {
        const scroller = scrollRef.current;
        if (!scroller) return;
        if (direction < 0) release();
        const step = Math.max(120, scroller.clientHeight - coveredBottom(scroller) - 96);
        scroller.scrollBy({ top: direction * step, behavior: reduced ? "auto" : "smooth" });
      },
      edge: (direction) => {
        const scroller = scrollRef.current;
        if (!scroller) return;
        if (direction > 0) {
          toLatest();
          return;
        }
        release();
        scroller.scrollTo({ top: 0, behavior: reduced ? "auto" : "smooth" });
      },
    };
    return () => {
      keysRef.current = null;
    };
  }, [keysRef, reduced, release, toLatest]);

  return (
    <>
      <div
        className="au-scroll"
        ref={scrollRef}
        // Keyboard scrolling and a name for screen readers; announcements come from one live
        // region (see App), not from every streamed word.
        role="log"
        aria-live="off"
        aria-label="Conversation"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrolling region must be focusable to scroll from the keyboard.
        tabIndex={0}
        onKeyDown={(event) => {
          // Keys that scroll the transcript are the transcript's, not the lab's shortcuts.
          if (SCROLL_KEYS.has(event.key)) event.stopPropagation();
        }}
      >
        <div className="au-column" ref={columnRef}>
          {model.turns.map((turn, index) => {
            const last = index === lastIndex;
            const running = turn.status === "running" || turn.status === "waiting";
            return (
              <TurnView
                // The user's message id outlives the agent renaming its turn (turn_started), so the turn
                // never remounts (and never re-animates) when its name arrives.
                key={keyOf(turn)}
                turn={turn}
                notices={notices.get(turn.id) ?? NO_NOTICES}
                orbState={
                  last ? activity : turn.status === "failed" ? "error" : running ? "idle" : "done"
                }
                session={last ? model.session : null}
                workspace={workspace}
                player={player}
                lastEventAt={last && running ? model.lastEventAt : null}
                autonomy={modes.current.get(keyOf(turn)) ?? sessionAutonomy ?? fallbackAutonomy}
                pendingAnswer={last ? pendingAnswer : null}
                typing={last ? typing : false}
                onAnswer={onAnswer}
                active={last && running}
                plan={model.plan && model.plan.turnId === turn.id && !running ? model.plan : null}
              />
            );
          })}
          {loose.length > 0 ? (
            <div className="au-notices">
              {loose.map((notice) => (
                <NoticeRow key={`${notice.at}:${notice.message}`} notice={notice} />
              ))}
            </div>
          ) : null}
          {sent.map((message) => (
            <SentTurnView
              key={message.id}
              message={message}
              workspace={workspace}
              stopped={stoppedIds.has(message.id)}
            />
          ))}
        </div>
      </div>
      <button
        type="button"
        className="au-jump"
        data-visible={pinned ? undefined : ""}
        tabIndex={pinned ? -1 : 0}
        aria-hidden={pinned}
        onClick={toLatest}
      >
        <ArrowDown size={13} strokeWidth={2} />
        Jump to latest
      </button>
    </>
  );
}
