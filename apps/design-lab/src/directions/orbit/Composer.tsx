import type { AgentBrandId } from "@ghost/ui";
import { ArrowUp, Check, ChevronDown, Pencil, Plus, Square, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import {
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  AUTONOMY_LABELS,
  type Autonomy,
  effortLabel,
  findAgent,
  type LimitWindow,
  modelLabel,
  type PlanModel,
  type Player,
  type Workspace,
} from "../../kit";
import type { MarkTimeline } from "./geometry";
import { submitAction } from "./keys";
import { OrbitMark } from "./OrbitMark";
import { Picker } from "./Picker";
import { PlanCard, PlanSegment } from "./Plan";
import type { ComposerState, HandoffSummary, QueuedMessage } from "./state";
import { formatClock, resetTime } from "./text";
import { DiffCounts, Keycap, LevelGlyph, popover, useOutsideClose, useReduced } from "./ui";

/*
 * The composer (visual-language.md 4.9): one object that grows upward in segments (Grok's single
 * material, Claude's fused bars). Above the input, at most two 36px segments: plan, queue,
 * handoff. Below it, Aside's control row: attach, the agent pill, the autonomy chip, the context
 * meter, and a send button that changes with the state, the way X's primary does. In a narrow
 * window the pill drops its model and effort and the meter its number before anything clips.
 */

const AUTONOMY_CONSEQUENCE: Readonly<Record<Autonomy, string>> = {
  full_auto: "Edits files and runs commands without asking. Every turn is checkpointed.",
  auto_edit: "Edits files without asking. Asks before running commands.",
  ask: "Asks before every edit and command.",
};

const AUTONOMY_ORDER: readonly Autonomy[] = ["full_auto", "auto_edit", "ask"];

/** A segment's enter and exit: height and opacity over --o-dock. */
function segmentMotion(reduced: boolean) {
  return {
    initial: { height: 0, opacity: 0 },
    animate: { height: "auto", opacity: 1 },
    exit: { height: 0, opacity: 0 },
    transition: { duration: reduced ? 0.12 : 0.26, ease: [0.23, 1, 0.32, 1] as const },
  };
}

/** Aside's autonomy menu: three levels with their consequences, and Final confirm. */
export function AutonomyMenu({
  value,
  allowed,
  agentName,
  finalConfirm,
  left,
  onChoose,
  onFinalConfirm,
  onClose,
  anchor,
}: {
  readonly value: Autonomy;
  readonly allowed: readonly Autonomy[];
  readonly agentName: string;
  readonly finalConfirm: boolean;
  readonly left: number;
  readonly onChoose: (autonomy: Autonomy) => void;
  readonly onFinalConfirm: (on: boolean) => void;
  readonly onClose: () => void;
  readonly anchor: { readonly current: HTMLElement | null };
}) {
  const reduced = useReduced();
  const ref = useRef<HTMLDivElement>(null);
  // Pi never asks: the levels that ask and Final confirm do not apply to it.
  const canAsk = allowed.includes("ask") || allowed.includes("auto_edit");
  useOutsideClose(ref, onClose, anchor);
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const index = ["1", "2", "3"].indexOf(event.key);
      const level = AUTONOMY_ORDER[index];
      if (level && allowed.includes(level)) {
        event.preventDefault();
        onChoose(level);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [allowed, onChoose]);
  return (
    <motion.div
      ref={ref}
      className="o-menu o-autonomy-menu"
      role="menu"
      aria-label="Autonomy"
      data-orbit-menu=""
      style={{ left }}
      {...popover(reduced)}
    >
      {AUTONOMY_ORDER.map((level, index) => {
        const disabled = !allowed.includes(level);
        const selected = level === value;
        return (
          <button
            key={level}
            type="button"
            role="menuitemradio"
            aria-checked={selected}
            className="o-autonomy-row"
            data-selected={selected ? "" : undefined}
            disabled={disabled}
            onClick={() => onChoose(level)}
          >
            <LevelGlyph autonomy={level} />
            <span className="o-autonomy-text">
              <span className="o-t-ui o-w-strong">{AUTONOMY_LABELS[level]}</span>
              <span className="o-t-meta o-c3">
                {disabled ? `${agentName} never asks before acting.` : AUTONOMY_CONSEQUENCE[level]}
              </span>
            </span>
            {selected ? (
              <span className="o-filled-check" aria-hidden="true">
                <Check size={9} strokeWidth={3} />
              </span>
            ) : (
              <Keycap>{index + 1}</Keycap>
            )}
          </button>
        );
      })}
      <div className="o-menu-rule" />
      <button
        type="button"
        role="menuitemcheckbox"
        aria-checked={canAsk && finalConfirm}
        className="o-switch-row"
        disabled={!canAsk}
        onClick={() => onFinalConfirm(!finalConfirm)}
      >
        <span className="o-autonomy-text">
          <span className="o-t-ui o-w-strong">Final confirm</span>
          <span className="o-t-meta o-c3">
            {canAsk
              ? "Always ask before rm, force pushes and writes outside the project."
              : `${agentName} cannot ask, so nothing waits for a confirm.`}
          </span>
        </span>
        <span
          className="o-switch"
          data-on={canAsk && finalConfirm ? "" : undefined}
          aria-hidden="true"
        />
      </button>
      <p className="o-menu-foot o-t-mono o-mono-11 o-c3">
        <Keycap label="Command">{"⌘"}</Keycap>
        <Keycap label="Shift">{"⇧"}</Keycap>
        <Keycap>A</Keycap> cycles autonomy
      </p>
    </motion.div>
  );
}

/** "Next message goes to Codex with a handoff · 412 tokens", and a preview of what is handed over. */
function HandoffSegment({
  agent,
  name,
  tokens,
  previewOpen,
  onPreview,
  onKeep,
}: {
  readonly agent: string;
  readonly name: string;
  readonly tokens: number;
  readonly previewOpen: boolean;
  readonly onPreview: () => void;
  readonly onKeep: () => void;
}) {
  return (
    <div className="o-segment o-handoff">
      <OrbitMark agent={agent} size={14} surface="var(--o-raised-2)" label={name} />
      <span className="o-segment-text o-t-meta o-c2">
        Next message goes to <span className="o-c1 o-w-strong">{name}</span> with a handoff ·{" "}
        <span className="o-num">{tokens}</span> tokens
      </span>
      <button
        type="button"
        className="o-text-btn o-t-meta"
        aria-expanded={previewOpen}
        onClick={onPreview}
      >
        {previewOpen ? "Hide" : "Preview"}
      </button>
      <button
        type="button"
        className="o-seg-close"
        aria-label="Keep the chat's agent"
        onClick={onKeep}
      >
        <X size={12} strokeWidth={1.75} aria-hidden="true" />
      </button>
    </div>
  );
}

/**
 * What a handoff carries, as X Money's receipt: the chat, its turns, the files it changed and the
 * last answer, so you know what the next agent starts from before you send.
 */
function HandoffCard({
  agent,
  name,
  summary,
  onClose,
}: {
  readonly agent: string;
  readonly name: string;
  readonly summary: HandoffSummary;
  readonly onClose: () => void;
}) {
  const latest = useRef(onClose);
  latest.current = onClose;
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      latest.current();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
  return (
    <div className="o-plan-card o-handoff-card">
      <div className="o-plan-card-head">
        <OrbitMark agent={agent} size={16} surface="var(--o-raised)" label={name} />
        <span className="o-t-ui o-w-strong">Handoff to {name}</span>
        <span className="o-t-mono o-mono-11 o-c3 o-num">{summary.tokens} tokens</span>
        <button type="button" className="o-ghost o-t-meta o-plan-hide" onClick={onClose}>
          Hide
        </button>
      </div>
      <div className="o-tear" aria-hidden="true" />
      <dl className="o-sheet-rows o-t-ui">
        <div className="o-perm-row">
          <dt className="o-c3">Chat</dt>
          <dd className="o-c1">{summary.title}</dd>
        </div>
        <div className="o-perm-row">
          <dt className="o-c3">Turns</dt>
          <dd className="o-c1 o-num">{summary.turns}</dd>
        </div>
        <div className="o-perm-row">
          <dt className="o-c3">Files</dt>
          <dd className="o-c1 o-handoff-files">
            {summary.files === 0 ? (
              "None changed"
            ) : (
              <>
                <span className="o-num">
                  {summary.files} {summary.files === 1 ? "file" : "files"}
                </span>
                <DiffCounts added={summary.added} removed={summary.removed} size={11} />
              </>
            )}
          </dd>
        </div>
        {summary.lastAnswer ? (
          <div className="o-perm-row">
            <dt className="o-c3">Last answer</dt>
            <dd className="o-c2 o-handoff-answer">{summary.lastAnswer}</dd>
          </div>
        ) : null}
      </dl>
      <p className="o-plan-card-foot o-t-meta o-c3">
        {name} reads this summary, not the other agent's session.
      </p>
    </div>
  );
}

/**
 * A blocking notice (section 4.8): the fact that stops sending, as a composer segment with its
 * one way out (Claude's docked banners).
 */
function BlockingSegment({
  window,
  agentName,
  onSwitch,
}: {
  readonly window: LimitWindow;
  readonly agentName: string;
  readonly onSwitch: () => void;
}) {
  const reset = window.resets_at !== null ? resetTime(window.resets_at) : null;
  return (
    <div className="o-segment o-blocking" role="status">
      <span className="o-diamond" aria-hidden="true" />
      <span className="o-segment-text o-t-meta o-c2">
        {agentName} {window.label.toLowerCase()} limit reached
        {reset ? (
          <>
            {" · resets "}
            <span className="o-num">{reset}</span>
          </>
        ) : null}
      </span>
      <button type="button" className="o-text-btn o-t-meta" onClick={onSwitch}>
        Switch agent
      </button>
    </div>
  );
}

function QueueSegment({
  queue,
  paused,
  onEdit,
  onRemove,
  onSendNext,
}: {
  readonly queue: readonly QueuedMessage[];
  /** A stop paused the queue: nothing goes out until you send the next one yourself. */
  readonly paused: boolean;
  readonly onEdit: (message: QueuedMessage) => void;
  readonly onRemove: (message: QueuedMessage) => void;
  readonly onSendNext: () => void;
}) {
  const next = queue[0];
  if (!next) return null;
  return (
    <div className="o-segment o-queue" data-paused={paused ? "" : undefined}>
      <span className="o-t-caps o-c3">Queued</span>
      <span className="o-t-mono o-c2 o-num">{queue.length}</span>
      {paused ? <span className="o-t-meta o-c3 o-queue-paused">· paused</span> : null}
      <span className="o-segment-text o-t-ui o-c2">{next.text}</span>
      {paused ? (
        <button type="button" className="o-text-btn o-t-meta" onClick={onSendNext}>
          Send next
        </button>
      ) : null}
      <button type="button" className="o-text-btn o-t-meta" onClick={() => onEdit(next)}>
        Edit
      </button>
      <button
        type="button"
        className="o-seg-close"
        aria-label="Remove from the queue"
        onClick={() => onRemove(next)}
      >
        <X size={12} strokeWidth={1.75} aria-hidden="true" />
      </button>
    </div>
  );
}

/** Grok Build's status bar meter: a 28 x 4px track, orange from 85%. */
function ContextMeter({ used, size }: { readonly used: number; readonly size: number }) {
  const share = size > 0 ? Math.min(1, used / size) : 0;
  const percent = Math.round(share * 100);
  const total = size >= 1_000_000 ? `${size / 1_000_000}M` : `${Math.round(size / 1000)}k`;
  return (
    <span className="o-meter" title={`${percent}% of ${total} context used`}>
      <span className="o-meter-track" aria-hidden="true">
        <span
          className="o-meter-fill"
          data-high={share >= 0.85 ? "" : undefined}
          style={{ width: `${Math.max(share > 0 ? 8 : 0, share * 100)}%` }}
        />
      </span>
      <span className="o-t-mono o-mono-11 o-c3 o-num o-meter-pct">{percent}%</span>
      <span className="o-sr">of the context window used</span>
    </span>
  );
}

export interface ComposerProps {
  readonly workspace: Workspace;
  readonly state: ComposerState;
  readonly onChange: (change: Partial<ComposerState>) => void;
  /** The picker's choice of agent, model and effort (the window clamps the autonomy to the agent). */
  readonly onCommitChoice: (choice: {
    agent: AgentBrandId;
    model: string;
    effort: string | null;
  }) => void;
  readonly player: Player;
  /** The live activity of the chat's agent, for the pill's mark while it is that agent. */
  readonly timeline: MarkTimeline | null;
  /** The chat's agent (the one that ran the last turn), or null in a new chat. */
  readonly chatAgent: string | null;
  /** The agent works: send becomes stop or queue, and the hint line shows. */
  readonly running: boolean;
  readonly stopping: boolean;
  readonly queue: readonly QueuedMessage[];
  /** A stop paused the queue (Aside: a stop never sends the next message by itself). */
  readonly queuePaused: boolean;
  readonly onSendNext: () => void;
  /** What a handoff to another agent would carry, for the segment's Preview. */
  readonly handoffSummary: HandoffSummary;
  readonly plan: PlanModel | null;
  readonly planUpdated: string;
  readonly context: { readonly used: number; readonly size: number } | null;
  /** A spent limit of the chat's agent: sending to it waits, and the composer says so. */
  readonly blocking: LimitWindow | null;
  readonly handoffTokens: number;
  readonly autonomyOpen: boolean;
  readonly onAutonomyOpen: (open: boolean) => void;
  readonly onAutonomy: (autonomy: Autonomy) => void;
  readonly onSend: (text: string) => void;
  readonly onQueue: (text: string) => void;
  /** Cmd Enter while the agent works: the message goes into the running turn now. */
  readonly onSteer: (text: string) => void;
  readonly onStop: () => void;
  readonly onEditQueued: (message: QueuedMessage) => void;
  readonly onRemoveQueued: (message: QueuedMessage) => void;
  readonly onKeepAgent: () => void;
  /** Opens the picker, from the blocking notice's "Switch agent". */
  readonly onSwitchAgent: () => void;
  /** The empty state's composer takes focus when it shows. */
  readonly autoFocus?: boolean;
  readonly onSoon: (what: string) => void;
  readonly onCopy: (text: string, done: string) => void;
  readonly children?: ReactNode;
}

/** The composer and everything it opens: the picker, the autonomy menu, the plan card. */
export function Composer(props: ComposerProps) {
  const { workspace, state, onChange, player, running, stopping, queue, plan, autonomyOpen } =
    props;
  const reduced = useReduced();
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const chipRef = useRef<HTMLButtonElement>(null);
  const pillRef = useRef<HTMLButtonElement>(null);
  const [planOpen, setPlanOpen] = useState(false);
  const [handoffOpen, setHandoffOpen] = useState(false);
  const [menuLeft, setMenuLeft] = useState(0);
  const agent = findAgent(workspace, state.agent);
  const name = agent?.name ?? state.agent;
  const model = modelLabel(workspace, state.agent, state.model);
  const effort = effortLabel(workspace, state.agent, state.model, state.effort);
  const text = state.draft;
  const typed = text.trim() !== "";
  const sameAgent = props.chatAgent === null || props.chatAgent === state.agent;
  const live = sameAgent && props.timeline ? { timeline: props.timeline, player } : undefined;
  const showPlan = plan !== null && running && !plan.done;
  const handoff = !sameAgent && props.chatAgent !== null;
  // A spent limit blocks only the agent it belongs to: switching agents gets past it.
  const blocked = props.blocking !== null && sameAgent;
  const segments: { readonly key: string; readonly node: ReactNode }[] = [];
  if (blocked && props.blocking) {
    segments.push({
      key: "blocking",
      node: (
        <BlockingSegment window={props.blocking} agentName={name} onSwitch={props.onSwitchAgent} />
      ),
    });
  }
  if (showPlan && plan) {
    segments.push({
      key: "plan",
      node: (
        <PlanSegment plan={plan} open={planOpen} onToggle={() => setPlanOpen((value) => !value)} />
      ),
    });
  }
  if (queue.length > 0) {
    segments.push({
      key: "queue",
      node: (
        <QueueSegment
          queue={queue}
          paused={props.queuePaused}
          onEdit={props.onEditQueued}
          onRemove={props.onRemoveQueued}
          onSendNext={props.onSendNext}
        />
      ),
    });
  }
  if (handoff) {
    segments.push({
      key: "handoff",
      node: (
        <HandoffSegment
          agent={state.agent}
          name={name}
          tokens={props.handoffTokens}
          previewOpen={handoffOpen}
          onPreview={() => setHandoffOpen((value) => !value)}
          onKeep={() => {
            setHandoffOpen(false);
            props.onKeepAgent();
          }}
        />
      ),
    });
  }
  const visible = segments.slice(0, 2);
  const overflow = segments.length - visible.length;

  // The field grows with its text to 12 lines (22px each), then scrolls.
  useLayoutEffect(() => {
    const field = fieldRef.current;
    if (!field) return;
    if (text === "") {
      field.style.height = "";
      return;
    }
    field.style.height = "0px";
    field.style.height = `${Math.min(Math.max(field.scrollHeight, 22), 12 * 22)}px`;
  }, [text]);

  useEffect(() => {
    if (props.autoFocus) fieldRef.current?.focus({ preventScroll: true });
  }, [props.autoFocus]);

  // Esc folds the plan card first (capture phase), so it never stops the running turn.
  useEffect(() => {
    if (!planOpen) return;
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setPlanOpen(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [planOpen]);

  const submit = (meta: boolean): void => {
    if (blocked && !running) return;
    const action = submitAction({ text, running, meta, canSteer: true });
    if (action === "send") props.onSend(text.trim());
    else if (action === "queue") props.onQueue(text.trim());
    else if (action === "steer") props.onSteer(text.trim());
  };

  const onKey = (event: KeyboardEvent<HTMLTextAreaElement>): void => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      submit(event.metaKey || event.ctrlKey);
    }
  };

  const placeholder = running
    ? `Queue a message for ${name}`
    : blocked
      ? `${name} is paused until its limit resets`
      : `Message ${name}`;

  return (
    <div className="o-composer-wrap" data-running={running ? "" : undefined}>
      <AnimatePresence>
        {state.pickerOpen ? (
          <Picker
            key="picker"
            workspace={workspace}
            state={state}
            onCommit={(choice) => {
              props.onCommitChoice(choice);
              fieldRef.current?.focus({ preventScroll: true });
            }}
            onClose={() => onChange({ pickerOpen: false })}
            anchor={pillRef}
            onCopy={props.onCopy}
          />
        ) : null}
      </AnimatePresence>
      <AnimatePresence>
        {autonomyOpen ? (
          <AutonomyMenu
            key="autonomy"
            value={state.autonomy}
            allowed={agent?.autonomy ?? ["full_auto"]}
            agentName={name}
            finalConfirm={state.finalConfirm}
            left={menuLeft}
            anchor={chipRef}
            onChoose={(level) => {
              props.onAutonomy(level);
              props.onAutonomyOpen(false);
            }}
            onFinalConfirm={(on) => onChange({ finalConfirm: on })}
            onClose={() => props.onAutonomyOpen(false)}
          />
        ) : null}
      </AnimatePresence>
      <AnimatePresence>
        {planOpen && showPlan && plan ? (
          <motion.div
            key="plan-card"
            className="o-plan-pop"
            data-orbit-menu=""
            {...popover(reduced)}
          >
            <PlanCard
              plan={plan}
              agentName={name}
              updated={props.planUpdated}
              onHide={() => setPlanOpen(false)}
            />
          </motion.div>
        ) : null}
        {handoffOpen && handoff ? (
          <motion.div
            key="handoff-card"
            className="o-plan-pop"
            data-orbit-menu=""
            {...popover(reduced)}
          >
            <HandoffCard
              agent={state.agent}
              name={name}
              summary={props.handoffSummary}
              onClose={() => setHandoffOpen(false)}
            />
          </motion.div>
        ) : null}
      </AnimatePresence>
      {queue.length > 1 ? (
        <div className="o-peek" aria-hidden="true">
          {queue.slice(1, 3).map((message, index) => (
            <span
              key={message.id}
              className="o-peek-sliver"
              style={{ marginInline: 12 * (index + 1) }}
            />
          ))}
        </div>
      ) : null}
      <div className="o-composer" data-has-segments={visible.length > 0 ? "" : undefined}>
        <AnimatePresence initial={false}>
          {visible.map((segment, index) => (
            <motion.div key={segment.key} className="o-segment-wrap" {...segmentMotion(reduced)}>
              {segment.node}
              {index === 0 && overflow > 0 ? (
                <span className="o-seg-more o-t-meta o-c3">+{overflow}</span>
              ) : null}
            </motion.div>
          ))}
        </AnimatePresence>
        <div className="o-input-wrap">
          <textarea
            ref={fieldRef}
            className="o-input o-t-body"
            data-composer=""
            rows={1}
            value={text}
            placeholder={placeholder}
            aria-label={`Message ${name}`}
            spellCheck={false}
            onChange={(event) => onChange({ draft: event.target.value })}
            onKeyDown={onKey}
          />
        </div>
        <div className="o-controls">
          <button
            type="button"
            className="o-attach"
            aria-label="Attach files"
            title="Attach files"
            onClick={() => props.onSoon("Attaching files")}
          >
            <Plus size={16} strokeWidth={1.5} aria-hidden="true" />
          </button>
          <button
            ref={pillRef}
            type="button"
            className="o-pill"
            aria-haspopup="dialog"
            aria-expanded={state.pickerOpen}
            data-open={state.pickerOpen ? "" : undefined}
            onClick={() => {
              props.onAutonomyOpen(false);
              onChange({ pickerOpen: !state.pickerOpen });
            }}
          >
            <OrbitMark
              agent={state.agent}
              size={14}
              surface="var(--o-raised)"
              live={live}
              label={name}
            />
            <span className="o-t-ui o-w-strong o-c1 o-pill-name">{name}</span>
            {model ? <span className="o-t-ui o-c2 o-pill-model"> {model}</span> : null}
            {effort ? <span className="o-t-ui o-c3 o-pill-effort"> {effort}</span> : null}
            <ChevronDown className="o-c3" size={12} strokeWidth={1.75} aria-hidden="true" />
          </button>
          <button
            ref={chipRef}
            type="button"
            className="o-chip"
            aria-haspopup="menu"
            aria-expanded={autonomyOpen}
            data-open={autonomyOpen ? "" : undefined}
            onClick={() => {
              onChange({ pickerOpen: false });
              setMenuLeft(chipRef.current?.offsetLeft ?? 0);
              props.onAutonomyOpen(!autonomyOpen);
            }}
          >
            <LevelGlyph autonomy={state.autonomy} />
            <span className="o-t-ui o-c2 o-chip-label">{AUTONOMY_LABELS[state.autonomy]}</span>
            <ChevronDown className="o-c3" size={12} strokeWidth={1.75} aria-hidden="true" />
          </button>
          <span className="o-controls-fill" />
          {props.context ? (
            <ContextMeter used={props.context.used} size={props.context.size} />
          ) : null}
          <SendButton
            running={running}
            typed={typed && !(blocked && !running)}
            onSend={() => submit(false)}
            onSteer={() => submit(true)}
            onStop={props.onStop}
          />
        </div>
      </div>
      <p className="o-hint o-t-mono o-mono-11 o-c3" aria-hidden={!(running || queue.length > 0)}>
        {running
          ? stopping
            ? "esc again to force stop"
            : "esc to stop · enter to queue · ⌘ enter to steer"
          : queue.length > 0
            ? props.queuePaused
              ? "enter to send · queue paused"
              : "enter to send"
            : " "}
      </p>
      {props.children}
    </div>
  );
}

/** X's primary that changes with the state: send, stop, or Aside's Queue / Steer split. */
function SendButton({
  running,
  typed,
  onSend,
  onSteer,
  onStop,
}: {
  readonly running: boolean;
  readonly typed: boolean;
  readonly onSend: () => void;
  readonly onSteer: () => void;
  readonly onStop: () => void;
}) {
  if (running && typed) {
    return (
      <span className="o-split">
        <button type="button" className="o-split-main o-t-ui" onClick={onSend}>
          Queue
          <Keycap onInk label="Enter">
            {"↵"}
          </Keycap>
        </button>
        <button
          type="button"
          className="o-split-chev"
          aria-label="Steer instead"
          title="Steer instead (Command Enter)"
          onClick={onSteer}
        >
          <ChevronDown size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </span>
    );
  }
  if (running) {
    return (
      <button
        type="button"
        className="o-send"
        data-mode="stop"
        aria-label="Stop"
        title="Stop (Esc)"
        onClick={onStop}
      >
        <Square size={10} fill="currentColor" strokeWidth={0} aria-hidden="true" />
      </button>
    );
  }
  return (
    <button
      type="button"
      className="o-send"
      data-mode={typed ? "ready" : "empty"}
      aria-label="Send"
      title="Send (Enter)"
      disabled={!typed}
      onClick={onSend}
    >
      <ArrowUp size={16} strokeWidth={2} aria-hidden="true" />
    </button>
  );
}

/** The wait on a permission request as a clock, read from the playhead once a second. */
export function WaitClock({ player, since }: { readonly player: Player; readonly since: number }) {
  const [now, setNow] = useState(() => player.getTime());
  useEffect(() => {
    const update = (): void => setNow(player.getTime());
    return player.subscribe(update);
  }, [player]);
  return <span className="o-num">{formatClock(Math.max(0, now - since))}</span>;
}

/** The "Tell Codex what to do instead" field, shared with the permission card. */
export function InsteadField({
  agentName,
  value,
  onChange,
  onSubmit,
}: {
  readonly agentName: string;
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly onSubmit: () => void;
}) {
  return (
    <label className="o-instead">
      <Pencil className="o-c3" size={14} strokeWidth={1.5} aria-hidden="true" />
      <input
        className="o-instead-input o-t-ui"
        value={value}
        placeholder={`Tell ${agentName} what to do instead`}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && value.trim() !== "") {
            event.preventDefault();
            onSubmit();
          }
          if (event.key === "Escape") (event.target as HTMLInputElement).blur();
        }}
      />
    </label>
  );
}
