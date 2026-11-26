import {
  brandCssVariables,
  getBrand,
  Orb,
  type OrbState,
  springs,
  usePrefersReducedMotion,
} from "@ghost/ui";
import { ArrowDown, ChevronDown, GitFork, Pencil, RotateCcw, Square } from "lucide-react";
import { motion } from "motion/react";
import {
  type CSSProperties,
  memo,
  type ReactNode,
  type RefObject,
  useLayoutEffect,
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
  modelLabel,
  type Player,
  plural,
  type StopReason,
  type TranscriptModel,
  type Turn,
  type TurnFooter,
  type TurnItem,
  type Workspace,
} from "../../kit";
import { DurationMetric, PermissionCard, ThoughtBlock, ToolRunBlock } from "./blocks";
import { Collapse, cx, Glass, IconButton, LineCounts, RADIUS, useEntrance } from "./primitives";
import { CopyButton, Markdown } from "./text";

/** What the permission cards need from the window: answers given here, and shortcut presses. */
export interface PermissionInteraction {
  readonly answers: Readonly<Record<string, string>>;
  readonly pressed: string | null;
  readonly answer: (requestId: string, optionId: string) => void;
}

/** Where a stop pressed in the lab has got to: asked for, or done. */
export type Halt = "stopping" | "stopped" | null;

/**
 * Keeps the newest output in view while the agent writes, gliding rather than jumping. Any scroll
 * up that is not the follower's own (wheel, keys, the scrollbar) lets go, and a "Latest" button
 * brings it back; reaching the bottom again holds on again. When the playhead is not moving, it
 * snaps, so a frozen frame is the same every time.
 */
function useFollow(smooth: boolean) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const smoothRef = useRef(smooth);
  const jumpRef = useRef<() => void>(() => {});
  const [away, setAway] = useState(false);

  useLayoutEffect(() => {
    smoothRef.current = smooth;
  }, [smooth]);

  useLayoutEffect(() => {
    const scroller = scrollRef.current;
    const content = contentRef.current;
    if (!scroller || !content) return;
    const bottom = (): number => Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    // The follower's own writes, told apart from scrolling by hand.
    let written = Number.NaN;
    let lastTop = 0;
    const write = (top: number): void => {
      scroller.scrollTop = top;
      written = scroller.scrollTop;
    };
    let frame = 0;
    let last = 0;
    const stop = (): void => {
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
      last = 0;
    };
    const tick = (now: number): void => {
      const goal = bottom();
      const current = scroller.scrollTop;
      const delta = goal - current;
      if (Math.abs(delta) < 1) {
        write(goal);
        stop();
        return;
      }
      const dt = last === 0 ? 16 : Math.min(64, now - last);
      last = now;
      const step = delta * (1 - Math.exp(-dt / 120));
      write(current + Math.sign(delta) * Math.max(1, Math.abs(step)));
      frame = requestAnimationFrame(tick);
    };
    const follow = (): void => {
      if (!pinned.current) return;
      if (!smoothRef.current) {
        stop();
        write(bottom());
        return;
      }
      if (!frame) frame = requestAnimationFrame(tick);
    };
    write(bottom());
    lastTop = scroller.scrollTop;

    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(follow) : null;
    observer?.observe(content);
    observer?.observe(scroller);
    // The column's bottom padding follows the dock's height (--halo-dock-h on the main pane), so a
    // taller dock (a two-line draft, an open plan) also pulls the end of the answer back above it.
    // Watched as a style change rather than a resize: the dock's own resize observer sets it, and
    // a second resize observer reacting in the same frame would loop.
    const dock = new MutationObserver(follow);
    if (scroller.parentElement) {
      dock.observe(scroller.parentElement, { attributes: true, attributeFilter: ["style"] });
    }
    const letGo = (): void => {
      pinned.current = false;
      stop();
      setAway(true);
    };
    const onWheel = (event: WheelEvent): void => {
      if (event.deltaY < 0 && bottom() > 0) letGo();
    };
    const onScroll = (): void => {
      const top = scroller.scrollTop;
      const atBottom = bottom() - top < 24;
      const own = Math.abs(top - written) < 1.5;
      if (atBottom) {
        pinned.current = true;
      } else if (!own && top < lastTop - 0.5) {
        // Keys, the scrollbar, a drag: anything that moved up and was not the follower.
        letGo();
      }
      lastTop = top;
      setAway(!pinned.current);
    };
    jumpRef.current = () => {
      pinned.current = true;
      setAway(false);
      follow();
    };
    scroller.addEventListener("wheel", onWheel, { passive: true });
    scroller.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      stop();
      observer?.disconnect();
      dock.disconnect();
      scroller.removeEventListener("wheel", onWheel);
      scroller.removeEventListener("scroll", onScroll);
    };
  }, []);

  return { scrollRef, contentRef, away, jumpToLatest: () => jumpRef.current() };
}

/**
 * A thin overlay scrollbar for the transcript: hidden at rest, shown while you scroll (wheel, keys,
 * touch) or when the pointer reaches the window's right edge, and draggable. The view following
 * the stream moves it without showing it. Placed by writing to the DOM on scroll, so scrolling
 * costs no React render.
 */
function OverlayScrollbar({ scrollRef }: { readonly scrollRef: RefObject<HTMLDivElement | null> }) {
  const trackRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const scroller = scrollRef.current;
    const track = trackRef.current;
    const thumb = thumbRef.current;
    if (!scroller || !track || !thumb) return;
    let hide = 0;
    const place = (): void => {
      const range = scroller.scrollHeight - scroller.clientHeight;
      const room = track.clientHeight;
      track.toggleAttribute("data-none", range <= 1 || room <= 0);
      if (range <= 1 || room <= 0) return;
      const height = Math.max(
        36,
        Math.round((room * scroller.clientHeight) / scroller.scrollHeight),
      );
      const top = Math.round((room - height) * Math.min(1, scroller.scrollTop / range));
      thumb.style.height = `${height}px`;
      thumb.style.transform = `translateY(${top}px)`;
    };
    const reveal = (): void => {
      track.setAttribute("data-active", "");
      window.clearTimeout(hide);
      hide = window.setTimeout(() => track.removeAttribute("data-active"), 900);
    };
    // A scroll within a second of your own input is yours; the follower's writes are not.
    let input = Number.NEGATIVE_INFINITY;
    const onInput = (): void => {
      input = performance.now();
      reveal();
    };
    const onScroll = (): void => {
      place();
      if (performance.now() - input < 1000) reveal();
    };
    let drag: { readonly y: number; readonly top: number; readonly scale: number } | null = null;
    const onDown = (event: PointerEvent): void => {
      event.preventDefault();
      thumb.setPointerCapture(event.pointerId);
      // The lab may scale the window to fit; pointer moves are in screen px.
      const zoom = track.getBoundingClientRect().height / Math.max(1, track.offsetHeight);
      const travel = Math.max(1, track.clientHeight - thumb.offsetHeight);
      const range = scroller.scrollHeight - scroller.clientHeight;
      drag = { y: event.clientY, top: scroller.scrollTop, scale: range / travel / zoom };
      track.setAttribute("data-drag", "");
    };
    const onMove = (event: PointerEvent): void => {
      if (drag) scroller.scrollTop = drag.top + (event.clientY - drag.y) * drag.scale;
    };
    const onUp = (event: PointerEvent): void => {
      if (!drag) return;
      drag = null;
      if (thumb.hasPointerCapture(event.pointerId)) thumb.releasePointerCapture(event.pointerId);
      track.removeAttribute("data-drag");
    };
    place();
    scroller.addEventListener("scroll", onScroll, { passive: true });
    scroller.addEventListener("wheel", onInput, { passive: true });
    scroller.addEventListener("touchmove", onInput, { passive: true });
    scroller.addEventListener("keydown", onInput);
    thumb.addEventListener("pointerdown", onDown);
    thumb.addEventListener("pointermove", onMove);
    thumb.addEventListener("pointerup", onUp);
    thumb.addEventListener("pointercancel", onUp);
    const observer = typeof ResizeObserver === "function" ? new ResizeObserver(place) : null;
    observer?.observe(scroller);
    if (scroller.firstElementChild) observer?.observe(scroller.firstElementChild);
    // The track ends above the dock, whose height arrives as a style change (see useFollow).
    const dock = new MutationObserver(place);
    if (scroller.parentElement) {
      dock.observe(scroller.parentElement, { attributes: true, attributeFilter: ["style"] });
    }
    return () => {
      window.clearTimeout(hide);
      observer?.disconnect();
      dock.disconnect();
      scroller.removeEventListener("scroll", onScroll);
      scroller.removeEventListener("wheel", onInput);
      scroller.removeEventListener("touchmove", onInput);
      scroller.removeEventListener("keydown", onInput);
      thumb.removeEventListener("pointerdown", onDown);
      thumb.removeEventListener("pointermove", onMove);
      thumb.removeEventListener("pointerup", onUp);
      thumb.removeEventListener("pointercancel", onUp);
    };
  }, [scrollRef]);
  return (
    <div ref={trackRef} className="halo-scrollbar" aria-hidden="true">
      <div ref={thumbRef} className="halo-scrollbar-thumb" />
    </div>
  );
}

export function Transcript({
  model,
  activity,
  workspace,
  player,
  permissions,
  halt,
  footer,
}: {
  readonly model: TranscriptModel;
  /** What the agent does now, after a stop or an answer given here in the lab. */
  readonly activity: OrbState;
  readonly workspace: Workspace;
  readonly player: Player;
  readonly permissions: PermissionInteraction;
  /** A stop pressed in the lab, for the last turn. */
  readonly halt: Halt;
  /** The status slab and composer, floating over the bottom of the transcript. */
  readonly footer: ReactNode;
}) {
  const reduced = usePrefersReducedMotion();
  const { scrollRef, contentRef, away, jumpToLatest } = useFollow(player.playing && !reduced);
  const lastIndex = model.turns.length - 1;
  return (
    <>
      <div ref={scrollRef} className="halo-scroll">
        <div ref={contentRef} className="halo-column">
          {model.turns.map((turn, index) => (
            <TurnView
              // The user's message names the turn from the start; the agent renames it later.
              key={turn.user?.id ?? turn.id}
              turn={turn}
              live={index === lastIndex}
              activity={index === lastIndex ? activity : "done"}
              halt={index === lastIndex ? halt : null}
              sessionModel={model.session.model}
              sessionEffort={model.session.effort}
              workspace={workspace}
              permissions={permissions}
            />
          ))}
        </div>
      </div>
      <OverlayScrollbar scrollRef={scrollRef} />
      <div className="halo-dock-area">
        {away ? (
          <button type="button" className="halo-latest" onClick={jumpToLatest}>
            <ArrowDown size={14} aria-hidden="true" />
            Latest
          </button>
        ) : null}
        {footer}
      </div>
    </>
  );
}

const TurnView = memo(function TurnView({
  turn,
  live,
  activity,
  halt,
  sessionModel,
  sessionEffort,
  workspace,
  permissions,
}: {
  readonly turn: Turn;
  readonly live: boolean;
  readonly activity: OrbState;
  readonly halt: Halt;
  readonly sessionModel: string | null;
  readonly sessionEffort: string | null;
  readonly workspace: Workspace;
  readonly permissions: PermissionInteraction;
}) {
  const agentId = turn.agent ?? "generic";
  const modelId = turn.footer?.model ?? (live ? sessionModel : null);
  const effortId = turn.footer?.effort ?? (live ? sessionEffort : null);
  const modelText = [
    modelLabel(workspace, agentId, modelId),
    effortLabel(workspace, agentId, modelId, effortId),
  ]
    .filter(Boolean)
    .join(" · ");
  const working = turn.status === "running" || turn.status === "waiting";
  // The agent's card arrives when the agent starts its turn (its turn_started moves the start
  // past the message), so it settles in as the agent picks up, not the instant you press Send.
  const started =
    turn.user === null ||
    turn.startedAt > turn.user.at ||
    turn.items.length > 0 ||
    turn.footer !== null ||
    !working;
  const showCard = started && (turn.items.length > 0 || working || turn.footer !== null);
  return (
    // The turn wears its own agent's colours, whoever the composer points at now.
    <section className="halo-turn" style={brandCssVariables(getBrand(agentId)) as CSSProperties}>
      {turn.user ? <UserBubble text={turn.user.text} /> : null}
      {showCard ? (
        <AgentCard
          turn={turn}
          live={live}
          activity={activity}
          halt={halt}
          modelText={modelText}
          autonomy={workspace.composer.autonomy}
          permissions={permissions}
        />
      ) : null}
    </section>
  );
});

/** The user's message: a bubble tinted with the agent it went to, sent up from the composer. */
function UserBubble({ text }: { readonly text: string }) {
  const enter = useEntrance();
  const reduced = usePrefersReducedMotion();
  return (
    <motion.div
      className="halo-user"
      initial={enter ? (reduced ? { opacity: 0 } : { opacity: 0, y: 48, scale: 0.94 }) : false}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={reduced ? { duration: 0.16 } : { type: "spring", stiffness: 260, damping: 24 }}
    >
      <div className="halo-user-actions">
        <IconButton icon={Pencil} label="Edit and resend" size={14} />
        <CopyButton text={text} label="Copy message" />
      </div>
      <div className="halo-user-bubble" data-halo-light="">
        <Glass material="bubble" radius={RADIUS.bubble} />
        <p className="halo-user-text">{text}</p>
      </div>
    </motion.div>
  );
}

const STOP_NOTES: Readonly<Partial<Record<StopReason, string>>> = {
  max_tokens: "Stopped at the token limit",
  max_turn_requests: "Stopped at the request limit",
  refusal: "The agent declined",
  cancelled: "Stopped",
  error: "Failed",
};

/**
 * Hides a run that waits on a permission when the permission card right after it shows the same
 * call: the card (or its answer) already says what will run.
 */
function visibleItems(items: readonly TurnItem[]): TurnItem[] {
  return items.filter((item, index) => {
    if (item.kind !== "tools" || item.status !== "waiting") return true;
    const next = items[index + 1];
    if (next?.kind !== "permission") return true;
    const { permission } = next;
    // Pending, or answered here before the agent has moved on: the card says what runs.
    const open = permission.status === "pending";
    return !(open && item.calls.some((call) => call.id === permission.toolCallId));
  });
}

/**
 * The agent's turn: one slab of glass that settles into place with a spring, headed by the
 * agent's live orb, holding its thoughts, tools and answer, and closed by the turn's numbers. What
 * the agent is doing right now is on the status line under the transcript, which never scrolls
 * away; the card's orb wears a lit ring while the turn runs.
 */
function AgentCard({
  turn,
  live,
  activity,
  halt,
  modelText,
  autonomy,
  permissions,
}: {
  readonly turn: Turn;
  readonly live: boolean;
  readonly activity: OrbState;
  readonly halt: Halt;
  readonly modelText: string;
  readonly autonomy: Autonomy;
  readonly permissions: PermissionInteraction;
}) {
  const enter = useEntrance();
  const reduced = usePrefersReducedMotion();
  const brand = getBrand(turn.agent ?? "generic");
  const stopped = halt === "stopped";
  const working = (turn.status === "running" || turn.status === "waiting") && !stopped;
  // The live turn follows what the agent does now (an answer given here moves it on).
  const waiting = (live ? activity === "waiting" : turn.status === "waiting") && !stopped;
  const orbState: OrbState = stopped
    ? "idle"
    : live
      ? activity
      : turn.status === "failed"
        ? "error"
        : turn.status === "cancelled"
          ? "idle"
          : "done";
  const items = visibleItems(turn.items);
  const answer = useMemo(
    () =>
      turn.items
        .filter((item) => item.kind === "message")
        .map((item) => (item.kind === "message" ? item.text : ""))
        .join("\n\n"),
    [turn.items],
  );

  return (
    <motion.article
      className="halo-card"
      data-halo-light=""
      data-waiting={waiting ? "" : undefined}
      initial={enter ? (reduced ? { opacity: 0 } : { opacity: 0, y: 28, scale: 0.965 }) : false}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={
        reduced ? { duration: 0.2 } : { type: "spring", stiffness: 210, damping: 20, mass: 1 }
      }
      style={{ transformOrigin: "50% 0%" }}
    >
      <Glass material="card" radius={RADIUS.card} />
      <header className="halo-card-head">
        <span
          className="halo-card-orb"
          data-live={live && working ? "" : undefined}
          data-waiting={live && waiting ? "" : undefined}
        >
          <span className="halo-card-ring" aria-hidden="true" />
          <Orb brand={brand} state={orbState} size={30} />
        </span>
        <div className="halo-card-who">
          <span className="halo-agent-name">{brand.name}</span>
          {modelText ? <span className="halo-model-tag">{modelText}</span> : null}
        </div>
      </header>
      {items.length > 0 ? (
        <div className="halo-card-body">
          {items.map((item) => (
            <ItemSlot
              key={`${item.kind}:${item.id}`}
              item={item}
              agent={brand.name}
              autonomy={autonomy}
              cancelled={live && halt !== null}
              permissions={permissions}
            />
          ))}
        </div>
      ) : null}
      {stopped && turn.footer === null ? (
        <p className="halo-stopped">
          <Square size={10} fill="currentColor" strokeWidth={0} aria-hidden="true" />
          Stopped
        </p>
      ) : null}
      {turn.footer ? <TurnFooterView footer={turn.footer} answer={answer} /> : null}
    </motion.article>
  );
}

/**
 * One block of the turn. Blocks that arrive while you watch open their space with a spring and
 * rise into it; a message only rises, since its text keeps growing as it streams.
 */
const ItemSlot = memo(function ItemSlot({
  item,
  agent,
  autonomy,
  cancelled,
  permissions,
}: {
  readonly item: TurnItem;
  readonly agent: string;
  readonly autonomy: Autonomy;
  readonly cancelled: boolean;
  readonly permissions: PermissionInteraction;
}) {
  const enter = useEntrance();
  const reduced = usePrefersReducedMotion();
  const content = (
    <div className="halo-slot-inner">
      <ItemView
        item={item}
        agent={agent}
        autonomy={autonomy}
        cancelled={cancelled}
        permissions={permissions}
      />
    </div>
  );
  if (!enter) {
    return (
      <div className="halo-slot" data-kind={item.kind}>
        {content}
      </div>
    );
  }
  if (item.kind === "message" || reduced) {
    return (
      <div className="halo-slot halo-enter" data-kind={item.kind}>
        {content}
      </div>
    );
  }
  return (
    <motion.div
      className="halo-slot"
      data-kind={item.kind}
      initial={{ height: 0, opacity: 0, y: 12, overflow: "clip" }}
      animate={{ height: "auto", opacity: 1, y: 0, transitionEnd: { overflow: "visible" } }}
      transition={{ height: springs.smooth, y: springs.smooth, opacity: { duration: 0.26 } }}
    >
      {content}
    </motion.div>
  );
});

function ItemView({
  item,
  agent,
  autonomy,
  cancelled,
  permissions,
}: {
  readonly item: TurnItem;
  readonly agent: string;
  readonly autonomy: Autonomy;
  readonly cancelled: boolean;
  readonly permissions: PermissionInteraction;
}) {
  switch (item.kind) {
    case "thought":
      return <ThoughtBlock item={item} />;
    case "message":
      return (
        <div className="halo-message">
          <Markdown text={item.text} streaming={item.streaming} />
        </div>
      );
    case "tools":
      return <ToolRunBlock item={item} />;
    case "permission":
      return (
        <PermissionCard
          permission={item.permission}
          agent={agent}
          autonomy={autonomy}
          answer={permissions.answers[item.permission.requestId] ?? null}
          pressed={permissions.pressed}
          cancelled={cancelled}
          onAnswer={(optionId) => permissions.answer(item.permission.requestId, optionId)}
        />
      );
  }
}

/** Duration, tokens, cost, and the files the turn changed; actions for the whole answer. */
function TurnFooterView({
  footer,
  answer,
}: {
  readonly footer: TurnFooter;
  readonly answer: string;
}) {
  const enter = useEntrance();
  const [filesOpen, setFilesOpen] = useState(false);
  const usage = footer.usage;
  const cost = footer.cost ? formatCost(footer.cost.amount, footer.cost.currency) : null;
  const note = STOP_NOTES[footer.stopReason];
  const metrics = [
    usage
      ? { key: "in", text: `${formatCount(usage.input_tokens)} in`, title: "Input tokens" }
      : null,
    usage
      ? { key: "out", text: `${formatCount(usage.output_tokens)} out`, title: "Output tokens" }
      : null,
    usage?.cached_input_tokens
      ? {
          key: "cached",
          text: `${formatCount(usage.cached_input_tokens)} cached`,
          title: "Cached input tokens",
        }
      : null,
    cost ? { key: "cost", text: cost, title: "Cost" } : null,
  ].filter((metric) => metric !== null);
  return (
    <footer className={cx("halo-foot", enter && "halo-enter")}>
      <div className="halo-foot-row">
        <span className="halo-metrics">
          <DurationMetric ms={footer.durationMs} />
          {metrics.map((metric) => (
            <span key={metric.key} className="halo-metric" title={metric.title}>
              {metric.text}
            </span>
          ))}
        </span>
        {note ? <span className="halo-metric halo-stop-note">{note}</span> : null}
        {footer.files.length > 0 ? (
          <button
            type="button"
            className="halo-files-chip"
            data-halo-light=""
            aria-expanded={filesOpen}
            onClick={() => setFilesOpen((value) => !value)}
          >
            <span>{plural(footer.files.length, "file")} changed</span>
            <LineCounts added={footer.added} removed={footer.removed} />
            <ChevronDown size={13} className="halo-chevron" aria-hidden="true" />
          </button>
        ) : null}
        <span className="halo-foot-spacer" />
        <span className="halo-foot-actions">
          <CopyButton text={answer} label="Copy answer" />
          <IconButton icon={RotateCcw} label="Retry" size={14} />
          <IconButton icon={GitFork} label="Fork into a new chat" size={14} />
        </span>
      </div>
      <Collapse open={filesOpen}>
        <ul className="halo-files">
          {footer.files.map((file) => (
            <li key={file.path} className="halo-files-row">
              <span className="halo-files-name">{basename(file.path)}</span>
              <span className="halo-files-dir">{dirname(file.path)}</span>
              {file.isNew ? <span className="halo-tag">New</span> : null}
              <LineCounts added={file.added} removed={file.removed} />
            </li>
          ))}
        </ul>
      </Collapse>
    </footer>
  );
}
