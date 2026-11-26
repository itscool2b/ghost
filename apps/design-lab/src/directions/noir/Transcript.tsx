import { easings, GlassPanel, getBrand, Orb, type OrbState } from "@ghost/ui";
import {
  Brain,
  ChevronDown,
  CircleAlert,
  FileDiff,
  GitFork,
  Info,
  Pencil,
  RotateCcw,
  TriangleAlert,
} from "lucide-react";
import { AnimatePresence, type AnimationPlaybackControls, animate, motion } from "motion/react";
import {
  type CSSProperties,
  Fragment,
  memo,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  type Autonomy,
  effortLabel,
  formatCost,
  formatCount,
  formatDuration,
  type MessageItem,
  modelLabel,
  type NoticeModel,
  type PlanModel,
  type Player,
  plural,
  type SessionModel,
  type TranscriptModel,
  type Turn,
  type TurnFooter,
  type TurnItem,
  type UserMessageModel,
  type Workspace,
} from "../../kit";
import { CopyButton, Markdown } from "./Markdown";
import { PermissionCard } from "./Permission";
import { PlanStep } from "./Plan";
import { ThoughtStep } from "./Thought";
import { ToolRunStep } from "./ToolRun";
import { splitPath } from "./text";
import {
  DiffStat,
  Elapsed,
  EntranceProvider,
  Expand,
  GLIDE,
  IconButton,
  MiniOrb,
  riseIn,
  TRANSCRIPT_HOLD,
  TRANSCRIPT_RECHECK,
  useEntrance,
  useReduced,
} from "./ui";

interface TranscriptProps {
  readonly model: TranscriptModel;
  readonly workspace: Workspace;
  readonly player: Player;
  readonly autonomy: Autonomy;
  /** What the newest turn's agent is doing, for its live orb (a request answered here resumes it). */
  readonly activity: OrbState;
  /** The chat's title and project, set as the headline at the top of the conversation. */
  readonly title: string | null;
  readonly project: string | null;
  /** Told whether the headline is on screen, so the title bar can carry the title once it is not. */
  readonly onHeadline: (visible: boolean) => void;
}

/** Growth of one line of text (24 px) follows at once, in step with the caret; more glides. */
const FOLLOW_JUMP = 24;
/** A headline whose bottom is within this much of the top is under the title bar: gone. */
const HEADLINE_GONE = 64;

/**
 * The conversation: a 720 px column (prose on an 80-character measure inside it) on a strict
 * 24 px rhythm, fading out under the title bar and above the composer. While the reader is at the
 * bottom, the newest line stays in view as output arrives: a line at a time it follows at once,
 * a new block glides up (GLIDE). Scrolling up, or opening something, lets go until they come back.
 */
export function Transcript({
  model,
  workspace,
  player,
  autonomy,
  activity,
  title,
  project,
  onHeadline,
}: TranscriptProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const columnRef = useRef<HTMLDivElement>(null);
  const headlineRef = useRef<HTMLElement>(null);
  const headlineCallback = useRef(onHeadline);
  headlineCallback.current = onHeadline;
  const reduced = useReduced();
  const reducedRef = useRef(reduced);
  reducedRef.current = reduced;
  const pinned = useRef(true);
  const [live, setLive] = useState(false);

  // Blocks present on the first render appear in place; later ones play their entrance.
  useEffect(() => setLive(true), []);

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    const column = columnRef.current;
    if (!scroll || !column) return;
    // Where this effect last scrolled to: the scroll event that follows our own scrolling is not
    // the reader's, even if the content has grown under it by the time the event arrives.
    let ownTop = -1;
    let glide: AnimationPlaybackControls | null = null;
    let glideTo = -1;
    const place = (top: number): void => {
      scroll.scrollTop = top;
      ownTop = scroll.scrollTop;
    };
    const stopGlide = (): void => {
      glide?.stop();
      glide = null;
    };
    const toBottom = (smooth: boolean): void => {
      const end = Math.max(0, scroll.scrollHeight - scroll.clientHeight);
      if (glide && Math.abs(end - glideTo) < 1) return;
      const from = scroll.scrollTop;
      // A glide already under way retargets; otherwise only a real jump glides.
      if (!glide && (!smooth || reducedRef.current || Math.abs(end - from) <= FOLLOW_JUMP)) {
        place(end);
        return;
      }
      stopGlide();
      if (reducedRef.current) {
        place(end);
        return;
      }
      glideTo = end;
      const controls = animate(from, end, {
        ...GLIDE,
        onUpdate: place,
        onComplete: () => {
          if (glide === controls) glide = null;
        },
      });
      glide = controls;
    };
    toBottom(false);
    // Decided before the first paint, so the title bar never shows the title for a frame and then
    // hands it to the headline below.
    const headline = headlineRef.current;
    if (headline) {
      headlineCallback.current(
        headline.offsetTop + headline.offsetHeight - scroll.scrollTop > HEADLINE_GONE,
      );
    }
    const recheck = (): void => {
      pinned.current = scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 40;
    };
    const onScroll = (): void => {
      if (Math.abs(scroll.scrollTop - ownTop) < 1) return;
      // The reader scrolled: whatever was gliding stops where it is.
      stopGlide();
      ownTop = -1;
      recheck();
    };
    // A region the reader opens stops the follow while it unfolds; then the position decides.
    const onHold = (): void => {
      stopGlide();
      pinned.current = false;
      ownTop = -1;
    };
    scroll.addEventListener("scroll", onScroll, { passive: true });
    scroll.addEventListener(TRANSCRIPT_HOLD, onHold);
    scroll.addEventListener(TRANSCRIPT_RECHECK, recheck);
    const observer =
      typeof ResizeObserver === "function"
        ? new ResizeObserver(() => {
            if (pinned.current) toBottom(true);
          })
        : null;
    // The border box: the column's bottom padding tracks the dock, so a dock that grows (a plan,
    // a queued message) moves the bottom too, and the newest line must follow it up.
    observer?.observe(column, { box: "border-box" });
    observer?.observe(scroll);
    return () => {
      stopGlide();
      scroll.removeEventListener("scroll", onScroll);
      scroll.removeEventListener(TRANSCRIPT_HOLD, onHold);
      scroll.removeEventListener(TRANSCRIPT_RECHECK, recheck);
      observer?.disconnect();
    };
  }, []);

  // Later, as the reader scrolls, the title bar takes the title once the headline has gone under it.
  useEffect(() => {
    const scroll = scrollRef.current;
    const headline = headlineRef.current;
    if (!scroll || !headline || typeof IntersectionObserver !== "function") return;
    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[entries.length - 1];
        if (entry) headlineCallback.current(entry.isIntersecting);
      },
      { root: scroll, rootMargin: `-${HEADLINE_GONE}px 0px 0px 0px`, threshold: 0 },
    );
    observer.observe(headline);
    return () => observer.disconnect();
  }, []);

  // Notices grouped by the turn they arrived in; rebuilt only when a notice arrives.
  const notices = useMemo(() => {
    const byTurn = new Map<string | null, NoticeModel[]>();
    for (const notice of model.notices) {
      const list = byTurn.get(notice.turnId) ?? [];
      list.push(notice);
      byTurn.set(notice.turnId, list);
    }
    return byTurn;
  }, [model.notices]);

  const last = model.turns.length - 1;
  return (
    <div className="n-scroll" ref={scrollRef}>
      <EntranceProvider value={live}>
        <div className="n-column" ref={columnRef} role="log" aria-label="Conversation">
          <header className="n-headline" ref={headlineRef}>
            {project ? <p className="n-headline-kicker">{project}</p> : null}
            <h2 className="n-headline-title">{title ?? "Untitled chat"}</h2>
          </header>
          {model.turns.map((turn, index) => (
            <TurnView
              key={turn.id}
              turn={turn}
              session={index === last ? model.session : null}
              activity={index === last ? activity : null}
              plan={model.plan?.turnId === turn.id ? model.plan : null}
              workspace={workspace}
              player={player}
              autonomy={autonomy}
              notices={notices.get(turn.id) ?? NO_NOTICES}
            />
          ))}
          {(notices.get(null) ?? NO_NOTICES).map((notice) => (
            <Appear key={`${notice.at}:${notice.message}`} className="n-item n-notice-loose">
              <NoticeRow notice={notice} />
            </Appear>
          ))}
        </div>
      </EntranceProvider>
    </div>
  );
}

/** Where a turn's plan goes once it leaves the dock: after the last step that came before its last update. */
export function planSlot(items: readonly TurnItem[], plan: PlanModel): number {
  let slot = -1;
  items.forEach((item, index) => {
    const at = item.kind === "permission" ? item.permission.requestedAt : item.startedAt;
    if (at <= plan.updatedAt) slot = index;
  });
  return slot;
}

const TurnView = memo(function TurnView({
  turn,
  session,
  activity,
  plan,
  workspace,
  player,
  autonomy,
  notices,
}: {
  readonly turn: Turn;
  /** The session as last reported, for the newest turn (its model before the footer lands). */
  readonly session: SessionModel | null;
  /** For the newest turn, what its agent is doing now: its byline orb runs live while it works. */
  readonly activity: OrbState | null;
  /** The plan this turn sent, if any. */
  readonly plan: PlanModel | null;
  readonly workspace: Workspace;
  readonly player: Player;
  readonly autonomy: Autonomy;
  /** Notices and errors that arrived during this turn. */
  readonly notices: readonly NoticeModel[];
}) {
  const agent = turn.agent ?? "generic";
  const live = turn.status === "running" || turn.status === "waiting";
  const modelId = turn.footer?.model ?? session?.model ?? null;
  const effortId = turn.footer?.effort ?? session?.effort ?? null;
  const covered = new Set(
    turn.items.flatMap((item) =>
      item.kind === "permission" &&
      item.permission.status === "pending" &&
      item.permission.toolCallId
        ? [item.permission.toolCallId]
        : [],
    ),
  );
  const answer = turn.items
    .filter((item): item is MessageItem => item.kind === "message")
    .map((item) => item.text)
    .join("\n\n");
  // While the agent works through its plan, the plan sits in the dock; then it joins the turn.
  const settledPlan = plan && (plan.done || !live) ? plan : null;
  const slot = settledPlan ? planSlot(turn.items, settledPlan) : -2;
  const newest = turn.items.length - 1;
  const orbLive = live && activity !== null;

  return (
    <section
      className="n-turn"
      data-live={live ? "" : undefined}
      aria-busy={live ? true : undefined}
    >
      {turn.user ? <UserMessage message={turn.user} /> : null}
      <div className="n-reply">
        <Appear className="n-reply-head">
          <span
            className="n-reply-orb"
            style={{ "--n-orb-glow": getBrand(agent).glow } as CSSProperties}
          >
            <MiniOrb agent={agent} status="live" size={16} />
            <AnimatePresence initial={false}>
              {orbLive ? (
                <motion.span
                  key="live"
                  className="n-reply-orb-live"
                  initial={{ opacity: 0 }}
                  animate={{ opacity: 1 }}
                  exit={{ opacity: 0 }}
                  transition={{ duration: 0.2, ease: easings.out }}
                >
                  <Orb brand={agent} state={activity ?? "idle"} size={16} />
                </motion.span>
              ) : null}
            </AnimatePresence>
          </span>
          <span className="n-reply-agent">{getBrand(agent).name}</span>
          {modelId ? (
            <span className="n-reply-model">
              {modelLabel(workspace, agent, modelId)}
              {effortId ? (
                <span className="n-reply-effort">
                  {effortLabel(workspace, agent, modelId, effortId)}
                </span>
              ) : null}
            </span>
          ) : null}
          <span className="n-reply-rule" aria-hidden="true" />
          {live ? (
            <span className="n-reply-clock" title="Working">
              <Elapsed player={player} since={turn.startedAt} />
            </span>
          ) : null}
        </Appear>
        <div className="n-items">
          {slot === -1 && settledPlan ? <PlanAppear plan={settledPlan} /> : null}
          {turn.items.map((item, index) => (
            <Fragment key={`${item.kind}:${item.id}`}>
              {hidden(item, covered) ? null : (
                <Appear
                  className="n-item"
                  kind={item.kind}
                  // A first thought takes over from the placeholder in place, without an entrance.
                  enter={index === 0 && item.kind === "thought" && live ? false : undefined}
                >
                  <ItemView
                    item={item}
                    player={player}
                    agent={agent}
                    autonomy={autonomy}
                    turnLive={live}
                    latest={live && index === newest}
                  />
                </Appear>
              )}
              {index === slot && settledPlan ? <PlanAppear plan={settledPlan} /> : null}
            </Fragment>
          ))}
          {live && turn.items.length === 0 ? <Starting /> : null}
          {notices.map((notice) => (
            <Appear key={`${notice.at}:${notice.message}`} className="n-item" kind="notice">
              <NoticeRow notice={notice} />
            </Appear>
          ))}
        </div>
        {turn.footer ? (
          <Appear className="n-foot-wrap">
            <Footer footer={turn.footer} answer={answer} />
          </Appear>
        ) : null}
      </div>
    </section>
  );
});

function PlanAppear({ plan }: { readonly plan: PlanModel }) {
  return (
    <Appear className="n-item" kind="plan">
      <PlanStep plan={plan} />
    </Appear>
  );
}

/** A waiting run whose every unfinished call has its own permission card below says it twice. */
function hidden(item: TurnItem, covered: ReadonlySet<string>): boolean {
  if (item.kind !== "tools" || item.status !== "waiting") return false;
  return item.calls.every((call) => call.status === "completed" || covered.has(call.id));
}

/** A block that fades and rises in when it arrives after the first render. */
function Appear({
  children,
  className,
  kind,
  enter: forced,
}: {
  readonly children: ReactNode;
  readonly className: string;
  readonly kind?: string;
  /** Overrides whether the entrance plays. */
  readonly enter?: boolean;
}) {
  const reduced = useReduced();
  const entrance = useEntrance();
  const motionProps = riseIn(reduced, forced ?? entrance);
  return (
    <motion.div
      className={className}
      data-kind={kind}
      initial={motionProps.initial}
      animate={motionProps.animate}
      transition={motionProps.transition}
    >
      {children}
    </motion.div>
  );
}

const NO_NOTICES: readonly NoticeModel[] = [];

const NOTICE_ICON = { info: Info, warning: TriangleAlert, error: CircleAlert } as const;

/** An agent's notice or error, set like a step: a glyph in the notice's colour, then the text. */
function NoticeRow({ notice }: { readonly notice: NoticeModel }) {
  const Icon = NOTICE_ICON[notice.level] ?? Info;
  return (
    <div
      className="n-notice"
      data-level={notice.level}
      role={notice.level === "error" ? "alert" : "status"}
    >
      <span className="n-notice-glyph" aria-hidden="true">
        <Icon size={14} strokeWidth={1.6} />
      </span>
      <p className="n-notice-text">{notice.message}</p>
    </div>
  );
}

/**
 * The moment between sending and the agent's first output: a quiet "Thinking" step, shaped
 * exactly like the thought row that replaces it, so the hand-over is seamless.
 */
function Starting() {
  const reduced = useReduced();
  return (
    <motion.div
      className="n-item"
      data-kind="starting"
      initial={reduced ? false : { opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.2 }}
    >
      <div className="n-step-block" data-live="">
        <div className="n-step" data-static="">
          <span className="n-step-glyph" aria-hidden="true">
            <Brain size={14} strokeWidth={1.6} />
          </span>
          <span className="n-step-label" data-shimmer={reduced ? undefined : ""}>
            Thinking
          </span>
        </div>
      </div>
    </motion.div>
  );
}

const ItemView = memo(function ItemView({
  item,
  player,
  agent,
  autonomy,
  turnLive,
  latest,
}: {
  readonly item: TurnItem;
  readonly player: Player;
  readonly agent: string;
  readonly autonomy: Autonomy;
  readonly turnLive: boolean;
  /** The newest item of a live turn. */
  readonly latest: boolean;
}) {
  const reduced = useReduced();
  switch (item.kind) {
    case "thought":
      return <ThoughtStep item={item} />;
    case "message":
      return (
        <Markdown
          text={item.text}
          streaming={item.streaming}
          animate={item.streaming && !reduced}
        />
      );
    case "tools":
      return <ToolRunStep item={item} player={player} turnLive={turnLive} latest={latest} />;
    case "permission":
      return (
        <PermissionCard
          permission={item.permission}
          agentName={getBrand(agent).name}
          autonomy={autonomy}
        />
      );
    default:
      return null;
  }
});

/* ------------------------------------------------------------------------------- user message */

/** The bubble's padding on each side; the bubble is as wide as its longest line plus this. */
const BUBBLE_INSET = 16;

/**
 * The user's message in a quiet bubble on the right, shrink-wrapped to its longest line (CSS
 * alone would keep a wrapped bubble at its maximum width, leaving a ragged gap on the right).
 */
const UserMessage = memo(function UserMessage({ message }: { readonly message: UserMessageModel }) {
  const bubbleRef = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLParagraphElement>(null);
  const text = message.text;

  useLayoutEffect(() => {
    const bubble = bubbleRef.current;
    const paragraph = textRef.current;
    if (!bubble || !paragraph || text === "") return;
    const shrinkWrap = (): void => {
      bubble.style.width = "";
      if (bubble.offsetWidth === 0) return;
      // Bounding boxes include the lab's scale on the window; offsets do not.
      const scale = bubble.getBoundingClientRect().width / bubble.offsetWidth || 1;
      const range = document.createRange();
      range.selectNodeContents(paragraph);
      let widest = 0;
      for (const rect of range.getClientRects()) widest = Math.max(widest, rect.width / scale);
      if (widest > 0) bubble.style.width = `${Math.ceil(widest) + 2 * BUBBLE_INSET + 1}px`;
    };
    shrinkWrap();
    const fonts = typeof document.fonts === "undefined" ? null : document.fonts;
    fonts?.addEventListener("loadingdone", shrinkWrap);
    const row = bubble.parentElement;
    const observer =
      typeof ResizeObserver === "function" && row ? new ResizeObserver(shrinkWrap) : null;
    if (row) observer?.observe(row);
    return () => {
      fonts?.removeEventListener("loadingdone", shrinkWrap);
      observer?.disconnect();
    };
  }, [text]);

  return (
    <Appear className="n-user">
      <GlassPanel ref={bubbleRef} elevation={1} squircle radius={16} className="n-bubble">
        <p className="n-bubble-text" ref={textRef}>
          {text}
        </p>
      </GlassPanel>
      <div className="n-user-actions">
        <IconButton icon={Pencil} label="Edit and resend" size="sm" />
        <CopyButton text={text} label="Copy message" />
      </div>
    </Appear>
  );
});

/* ------------------------------------------------------------------------------------ footer */

function Footer({ footer, answer }: { readonly footer: TurnFooter; readonly answer: string }) {
  const [filesOpen, setFilesOpen] = useState(false);
  const cost = footer.cost ? formatCost(footer.cost.amount, footer.cost.currency) : null;
  const usage = footer.usage;
  const tokensTitle = usage
    ? [
        `${usage.input_tokens.toLocaleString("en-US")} input`,
        `${usage.output_tokens.toLocaleString("en-US")} output`,
        usage.cached_input_tokens
          ? `${usage.cached_input_tokens.toLocaleString("en-US")} cached`
          : null,
        usage.reasoning_tokens
          ? `${usage.reasoning_tokens.toLocaleString("en-US")} reasoning`
          : null,
      ]
        .filter(Boolean)
        .join(", ")
    : undefined;
  const files = footer.files;

  return (
    <div className="n-foot">
      <div className="n-foot-row">
        <div className="n-foot-actions">
          <CopyButton text={answer} label="Copy answer" />
          <IconButton icon={RotateCcw} label="Retry" size="sm" />
          <IconButton icon={GitFork} label="Fork into a new chat" size="sm" />
        </div>
        <dl className="n-foot-meta">
          <div>
            <dt>Time</dt>
            <dd className="n-num">{formatDuration(footer.durationMs)}</dd>
          </div>
          {usage ? (
            <div title={tokensTitle}>
              <dt>Tokens</dt>
              <dd className="n-num">
                {formatCount(usage.input_tokens)} in, {formatCount(usage.output_tokens)} out
              </dd>
            </div>
          ) : null}
          {cost ? (
            <div>
              <dt>Cost</dt>
              <dd className="n-num">{cost}</dd>
            </div>
          ) : null}
        </dl>
        {files.length > 0 ? (
          <button
            type="button"
            className="n-chip n-files-chip"
            aria-expanded={filesOpen}
            onClick={() => setFilesOpen((value) => !value)}
          >
            <FileDiff
              className="n-files-chip-icon"
              size={14}
              strokeWidth={1.6}
              aria-hidden="true"
            />
            <span>{plural(files.length, "file")} changed</span>
            <DiffStat added={footer.added} removed={footer.removed} />
            <ChevronDown className="n-chip-chev" size={12} strokeWidth={1.8} aria-hidden="true" />
          </button>
        ) : null}
      </div>
      <Expand open={filesOpen} follow>
        <ul className="n-foot-files">
          {files.map((file) => {
            const { dir, name } = splitPath(file.path);
            return (
              <li key={file.path} className="n-foot-file">
                <span className="n-call-target" title={file.path}>
                  <span className="n-path-dir">{dir}</span>
                  <span className="n-path-name">{name}</span>
                </span>
                {file.isNew ? <span className="n-step-tag">New</span> : null}
                <DiffStat added={file.added} removed={file.removed} />
              </li>
            );
          })}
        </ul>
      </Expand>
    </div>
  );
}
