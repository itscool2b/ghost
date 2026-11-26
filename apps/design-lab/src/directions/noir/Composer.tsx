import { GlassPanel, getBrand, Orb, type OrbState } from "@ghost/ui";
import {
  ArrowRightLeft,
  ArrowUp,
  AtSign,
  Check,
  ChevronDown,
  Hand,
  PencilLine,
  Plus,
  Square,
  X,
  Zap,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type KeyboardEvent, useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  AUTONOMY_LABELS,
  type Autonomy,
  effortLabel,
  findAgent,
  modelLabel,
  type PermissionModel,
  plural,
  type Workspace,
} from "../../kit";
import type { Handoff } from "./handoff";
import { Picker } from "./Picker";
import type { ComposerState, ComposerUpdate } from "./state";
import { IconButton, Kbd, MiniOrb, popIn, reveal, useDismiss, useReduced } from "./ui";

const AUTONOMY_ICON = { full_auto: Zap, auto_edit: PencilLine, ask: Hand } as const;

const AUTONOMY_HINT: Readonly<Record<Autonomy, string>> = {
  full_auto: "Works without asking. Every turn is checkpointed.",
  auto_edit: "Edits files on its own, asks before commands.",
  ask: "Asks before every tool.",
};

interface ComposerProps {
  readonly workspace: Workspace;
  readonly state: ComposerState;
  readonly update: ComposerUpdate;
  /** The orb's state: the open chat's activity when it is this agent's chat. */
  readonly orbState: OrbState;
  /** A turn is running or waiting: Send becomes Stop, and new messages queue. */
  readonly busy: boolean;
  readonly onStop: () => void;
  /** Whether Esc stops the turn (not while an approval card has the key). */
  readonly escStops: boolean;
  /** A request waiting on the user: what is typed now answers it, as "do this instead". */
  readonly pending: PermissionModel | null;
  readonly onInstead: (text: string) => void;
  /** The context a switch of agent sends along, while the next message would carry it. */
  readonly handoff: Handoff | null;
  /** Switches back to the chat's own agent. */
  readonly onKeepAgent: () => void;
  /** The new chat's composer, centred in the window: it takes focus as soon as it shows. */
  readonly hero?: boolean;
}

/**
 * The composer: one auto-growing field over a quiet toolbar. The orb at the left of the agent
 * pill is the room's light: it carries the agent's state, and the pill opens the agent > model >
 * effort picker. Cmd+L brings focus here from anywhere.
 */
export function Composer({
  workspace,
  state,
  update,
  orbState,
  busy,
  onStop,
  escStops,
  pending,
  onInstead,
  handoff,
  onKeepAgent,
  hero,
}: ComposerProps) {
  const reduced = useReduced();
  const fieldRef = useRef<HTMLTextAreaElement>(null);
  const [focused, setFocused] = useState(false);
  const [autonomyOpen, setAutonomyOpen] = useState(false);
  const [previewOpen, setPreviewOpen] = useState(false);
  const [menuLeft, setMenuLeft] = useState(0);
  const wrapRef = useRef<HTMLDivElement>(null);
  const chipRef = useRef<HTMLButtonElement>(null);
  // Whether the open picker came from the pill (rather than the lab's picker state).
  const [pickerFromPill, setPickerFromPill] = useState(false);
  const agent = findAgent(workspace, state.agent);
  const name = agent?.name ?? getBrand(state.agent).name;
  const model = modelLabel(workspace, state.agent, state.model);
  const effort = effortLabel(workspace, state.agent, state.model, state.effort);
  const canSend = state.draft.trim() !== "";
  const AutonomyIcon = AUTONOMY_ICON[state.autonomy];

  // The field grows with its text, up to eight lines, then scrolls. Empty, it is one line.
  const draft = state.draft;
  useLayoutEffect(() => {
    const field = fieldRef.current;
    if (!field) return;
    if (draft === "") {
      field.style.height = "";
      return;
    }
    field.style.height = "0px";
    field.style.height = `${Math.min(field.scrollHeight, 8 * 24)}px`;
  }, [draft]);

  // A new chat is for typing: its composer takes focus when it shows.
  useEffect(() => {
    if (hero) fieldRef.current?.focus({ preventScroll: true });
  }, [hero]);

  // Cmd+L (Ctrl+L) puts the cursor in the composer from anywhere in the window.
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (!(event.metaKey || event.ctrlKey) || event.altKey || event.key.toLowerCase() !== "l") {
        return;
      }
      event.preventDefault();
      fieldRef.current?.focus({ preventScroll: true });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const send = (): void => {
    const text = state.draft.trim();
    if (text === "") return;
    if (pending) {
      onInstead(text);
    } else {
      const message = { text, handoffFrom: handoff ? handoff.fromName : null };
      update({ draft: "", queue: [...state.queue, message].slice(-3) });
    }
    setPreviewOpen(false);
    fieldRef.current?.focus({ preventScroll: true });
  };

  const onFieldKey = (event: KeyboardEvent<HTMLTextAreaElement>): void => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      // With nothing typed, Cmd+Enter belongs to a waiting request (it allows once).
      if (!canSend && (event.metaKey || event.ctrlKey)) return;
      event.preventDefault();
      send();
    } else if (event.key === "Escape" && escStops) {
      event.preventDefault();
      onStop();
    }
  };

  const placeholder = pending
    ? `Or tell ${name} what to do instead`
    : busy
      ? `Queue a follow-up for ${name}`
      : `Message ${name}`;
  const caret = focused && state.draft === "";

  return (
    <div className="n-composer-wrap" data-hero={hero ? "" : undefined} ref={wrapRef}>
      <AnimatePresence>
        {autonomyOpen ? (
          <AutonomyMenu
            key="autonomy"
            left={menuLeft}
            value={state.autonomy}
            allowed={agent?.autonomy ?? ["full_auto"]}
            onChoose={(autonomy) => {
              update({ autonomy });
              setAutonomyOpen(false);
              fieldRef.current?.focus({ preventScroll: true });
            }}
            onClose={() => setAutonomyOpen(false)}
          />
        ) : null}
      </AnimatePresence>
      <AnimatePresence>
        {previewOpen && handoff ? (
          <HandoffPreview key="handoff" handoff={handoff} onClose={() => setPreviewOpen(false)} />
        ) : null}
      </AnimatePresence>
      <AnimatePresence>
        {state.pickerOpen ? (
          <Picker
            key="picker"
            workspace={workspace}
            state={state}
            update={update}
            onClose={() => update({ pickerOpen: false })}
            fromPill={pickerFromPill}
          />
        ) : null}
      </AnimatePresence>
      <GlassPanel
        elevation={2}
        blur
        squircle
        radius={22}
        className="n-composer"
        data-focused={focused ? "" : undefined}
      >
        <span className="n-composer-light" data-activity={orbState} aria-hidden="true" />
        <AnimatePresence initial={false}>
          {handoff ? (
            <motion.div key="handoff" className="n-handoff-wrap" {...reveal(reduced)}>
              <div className="n-handoff">
                <MiniOrb agent={state.agent} status="live" size={14} className="n-handoff-orb" />
                <span className="n-handoff-text">
                  <span className="n-handoff-who">
                    {handoff.toName} takes over from {handoff.fromName}
                  </span>
                  <span className="n-handoff-what">
                    with a handoff: {plural(handoff.prompts, "prompt")},{" "}
                    {plural(handoff.answers, "answer")}
                    {handoff.files > 0 ? `, ${plural(handoff.files, "changed file")}` : ""}
                  </span>
                </span>
                <button
                  type="button"
                  className="n-text-btn n-handoff-preview"
                  aria-haspopup="dialog"
                  aria-expanded={previewOpen}
                  onClick={() => {
                    setAutonomyOpen(false);
                    update({ pickerOpen: false });
                    setPreviewOpen((value) => !value);
                  }}
                >
                  Preview
                </button>
                <IconButton
                  icon={X}
                  label={`Keep ${handoff.fromName}`}
                  size="sm"
                  iconSize={12}
                  onClick={() => {
                    setPreviewOpen(false);
                    onKeepAgent();
                  }}
                />
              </div>
            </motion.div>
          ) : null}
        </AnimatePresence>
        <AnimatePresence initial={false}>
          {state.queue.length > 0 ? (
            <motion.ul key="queue" className="n-queue" {...reveal(reduced)}>
              {state.queue.map((message, index) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: the queue is short and ordered.
                <li key={index} className="n-queued">
                  <span className="n-queued-label">Queued</span>
                  <span className="n-queued-text">{message.text}</span>
                  {message.handoffFrom ? (
                    <span
                      className="n-queued-tag"
                      title={`Carries a handoff from ${message.handoffFrom}`}
                    >
                      <ArrowRightLeft size={12} strokeWidth={1.7} aria-hidden="true" />
                      Handoff
                    </span>
                  ) : null}
                  <IconButton
                    icon={X}
                    label="Remove from queue"
                    size="sm"
                    iconSize={12}
                    onClick={() => update({ queue: state.queue.filter((_, i) => i !== index) })}
                  />
                </li>
              ))}
            </motion.ul>
          ) : null}
        </AnimatePresence>
        <div className="n-field-wrap">
          {caret ? <span className="n-caret-idle" aria-hidden="true" /> : null}
          <textarea
            ref={fieldRef}
            className="n-field"
            data-composer-field=""
            data-caret={caret ? "" : undefined}
            rows={1}
            value={state.draft}
            placeholder={placeholder}
            aria-label={`Message ${name}`}
            aria-keyshortcuts="Meta+L"
            spellCheck={false}
            onChange={(event) => update({ draft: event.target.value })}
            onKeyDown={onFieldKey}
            onFocus={() => setFocused(true)}
            onBlur={() => setFocused(false)}
          />
        </div>
        <div className="n-bar">
          <IconButton icon={Plus} label="Attach images or files" />
          <IconButton icon={AtSign} label="Mention a file" />
          <span className="n-bar-sep" aria-hidden="true" />
          <button
            type="button"
            className="n-pill"
            aria-haspopup="dialog"
            aria-expanded={state.pickerOpen}
            onClick={() => {
              setAutonomyOpen(false);
              setPreviewOpen(false);
              setPickerFromPill(!state.pickerOpen);
              update({ pickerOpen: !state.pickerOpen });
            }}
          >
            <Orb brand={state.agent} state={orbState} size={18} />
            <span className="n-pill-agent">{name}</span>
            {model ? <span className="n-pill-rest">{model}</span> : null}
            {effort ? <span className="n-pill-rest">{effort}</span> : null}
            <ChevronDown className="n-pill-chev" size={14} strokeWidth={1.6} aria-hidden="true" />
          </button>
          <div className="n-autonomy">
            <button
              ref={chipRef}
              type="button"
              className="n-chip n-autonomy-chip"
              data-mode={state.autonomy}
              aria-haspopup="menu"
              aria-expanded={autonomyOpen}
              onClick={() => {
                update({ pickerOpen: false });
                setPreviewOpen(false);
                // The menu opens above the composer, starting where the chip does.
                setMenuLeft(offsetWithin(chipRef.current, wrapRef.current) - 6);
                setAutonomyOpen((value) => !value);
              }}
            >
              <AutonomyIcon
                className="n-autonomy-icon"
                size={12}
                strokeWidth={1.8}
                aria-hidden="true"
              />
              {AUTONOMY_LABELS[state.autonomy]}
            </button>
          </div>
          <span className="n-bar-fill" />
          {escStops ? (
            <span className="n-bar-hint">
              <Kbd>Esc</Kbd> to stop
            </span>
          ) : null}
          {busy && !canSend ? (
            <button
              type="button"
              className="n-send"
              data-mode="stop"
              aria-label="Stop"
              title="Stop"
              onClick={onStop}
            >
              <Square size={10} strokeWidth={0} fill="currentColor" aria-hidden="true" />
            </button>
          ) : (
            <button
              type="button"
              className="n-send"
              data-mode="send"
              aria-label={pending ? "Send instead" : busy ? "Queue message" : "Send"}
              title={pending ? "Send instead" : busy ? "Queue message" : "Send"}
              disabled={!canSend}
              onClick={send}
            >
              <ArrowUp size={16} strokeWidth={2} aria-hidden="true" />
            </button>
          )}
        </div>
      </GlassPanel>
    </div>
  );
}

/**
 * Where `element` starts inside `ancestor`, in the window's own CSS px: offsets ignore the
 * transform that scales the lab window, where bounding boxes would not.
 */
function offsetWithin(element: HTMLElement | null, ancestor: HTMLElement | null): number {
  let left = 0;
  let node: HTMLElement | null = element;
  while (node && node !== ancestor) {
    left += node.offsetLeft;
    node = node.offsetParent instanceof HTMLElement ? node.offsetParent : null;
  }
  return node === ancestor ? left : 0;
}

function AutonomyMenu({
  left,
  value,
  allowed,
  onChoose,
  onClose,
}: {
  /** Distance from the composer's left edge, so the menu starts where its chip does. */
  readonly left: number;
  readonly value: Autonomy;
  readonly allowed: readonly Autonomy[];
  readonly onChoose: (autonomy: Autonomy) => void;
  readonly onClose: () => void;
}) {
  const reduced = useReduced();
  const menuRef = useRef<HTMLDivElement>(null);
  useDismiss(menuRef, onClose);
  const motionProps = popIn(reduced);
  return (
    <motion.div
      ref={menuRef}
      className="n-menu n-autonomy-menu"
      role="menu"
      aria-label="Autonomy"
      style={{ left }}
      initial={motionProps.initial}
      animate={motionProps.animate}
      exit={motionProps.exit}
      transition={motionProps.transition}
    >
      <GlassPanel elevation={3} blur squircle radius={16} className="n-overlay-panel n-menu-panel">
        {(["full_auto", "auto_edit", "ask"] as const).map((mode) => {
          const Icon = AUTONOMY_ICON[mode];
          const disabled = !allowed.includes(mode);
          return (
            <button
              key={mode}
              type="button"
              role="menuitemradio"
              aria-checked={mode === value}
              className="n-menu-item"
              disabled={disabled}
              onClick={() => onChoose(mode)}
            >
              <Icon className="n-menu-icon" size={14} strokeWidth={1.6} aria-hidden="true" />
              <span className="n-menu-text">
                <span className="n-menu-label">{AUTONOMY_LABELS[mode]}</span>
                <span className="n-menu-hint">
                  {disabled ? "This agent never asks." : AUTONOMY_HINT[mode]}
                </span>
              </span>
              {mode === value ? (
                <Check className="n-menu-check" size={14} strokeWidth={1.8} aria-hidden="true" />
              ) : null}
            </button>
          );
        })}
      </GlassPanel>
    </motion.div>
  );
}

/**
 * The handoff block exactly as the next prompt will carry it, so nothing is added out of sight.
 */
function HandoffPreview({
  handoff,
  onClose,
}: {
  readonly handoff: Handoff;
  readonly onClose: () => void;
}) {
  const reduced = useReduced();
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, onClose);
  const motionProps = popIn(reduced);
  return (
    <motion.div
      ref={ref}
      className="n-menu n-handoff-pop"
      role="dialog"
      aria-label={`Handoff to ${handoff.toName}`}
      initial={motionProps.initial}
      animate={motionProps.animate}
      exit={motionProps.exit}
      transition={motionProps.transition}
    >
      <GlassPanel elevation={3} blur squircle radius={16} className="n-overlay-panel">
        <div className="n-handoff-pop-head">
          <span className="n-handoff-pop-title">Handoff to {handoff.toName}</span>
          <span className="n-handoff-pop-note">
            Sent before your message. Nothing else is added.
          </span>
        </div>
        <pre className="n-handoff-block">{handoff.block}</pre>
      </GlassPanel>
    </motion.div>
  );
}
