import {
  type AgentBrandId,
  easings,
  getBrand,
  Orb,
  type OrbState,
  springs,
  usePrefersReducedMotion,
} from "@ghost/ui";
import {
  ArrowUp,
  Check,
  ChevronDown,
  CornerDownLeft,
  FilePenLine,
  Hand,
  Plus,
  Square,
  Zap,
} from "lucide-react";
import { AnimatePresence, motion, useAnimate } from "motion/react";
import { type RefObject, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import {
  AUTONOMY_LABELS,
  type Autonomy,
  composerLabel,
  findAgent,
  type PermissionModel,
  type PermissionOption,
  type Workspace,
} from "../../kit";
import { Glass } from "./Glass";
import { type Handoff, HandoffOrbs, HandoffView } from "./Handoff";
import { optionForKey } from "./Permission";
import { Picker, type Selection, type SelectionMemory } from "./Picker";
import type { TranscriptKeys } from "./Transcript";
import { Collapse, cx, IconButton } from "./ui";

export const AUTONOMY_ICON: Readonly<Record<Autonomy, typeof Zap>> = {
  full_auto: Zap,
  auto_edit: FilePenLine,
  ask: Hand,
};

const AUTONOMY_HINT: Readonly<Record<Autonomy, string>> = {
  full_auto: "Edits and runs without asking",
  auto_edit: "Edits files, asks before commands",
  ask: "Asks before every change",
};

const AUTONOMY_ORDER: readonly Autonomy[] = ["full_auto", "auto_edit", "ask"];

/** The composer plus its two menus. Everything the next message is sent with lives here. */
export interface ComposerProps {
  readonly workspace: Workspace;
  readonly selection: Selection;
  /** The autonomy the next message goes with (what the selected agent can do). */
  readonly autonomy: Autonomy;
  /** True while a turn runs: Send becomes Stop, and typing queues. */
  readonly running: boolean;
  /** The pill orb's state: the running agent's activity, idle otherwise. */
  readonly orbState: OrbState;
  readonly pickerOpen: boolean;
  readonly onPickerOpen: (open: boolean) => void;
  readonly autonomyOpen: boolean;
  readonly onAutonomyOpen: (open: boolean) => void;
  readonly memory: SelectionMemory;
  readonly onCommit: (selection: Selection) => void;
  readonly onAutonomy: (autonomy: Autonomy) => void;
  readonly onPreview: (agent: AgentBrandId | null) => void;
  /** The request waiting on the user: its keys work from here (Enter allows, Esc rejects). */
  readonly pending: PermissionModel | null;
  readonly onAnswer: (option: PermissionOption) => void;
  /** Sends the text; while a turn runs, the app queues it instead. */
  readonly onSend: (text: string) => void;
  readonly onStop: () => void;
  /** Whether the box holds text (a waiting card's hints depend on it). */
  readonly onTyping: (typing: boolean) => void;
  /** The handoff the next message carries, when it goes to another agent than the last turn's. */
  readonly handoff: Handoff | null;
  /** Said when the selected agent cannot keep the autonomy the user chose. */
  readonly autonomyNote: string | null;
  /** Grows each time the app changed the autonomy for the user: the chip pulses once. */
  readonly pulse: number;
  /** PageUp, PageDown and Command-arrows scroll the transcript while the box is empty. */
  readonly keys: RefObject<TranscriptKeys | null>;
  /** The hero composer in a new chat: taller, with a send hint, menus below. */
  readonly hero?: boolean;
  /** Text to put in the box (a suggestion, or a queued message to edit); a new nonce applies it. */
  readonly prefill?: { readonly text: string; readonly nonce: number } | null;
  /** Takes focus when it mounts (the dock's composer, taking over from the hero's). */
  readonly autoFocus?: boolean;
}

export function Composer({
  workspace,
  selection,
  autonomy,
  running,
  orbState,
  pickerOpen,
  onPickerOpen,
  autonomyOpen,
  onAutonomyOpen,
  memory,
  onCommit,
  onAutonomy,
  onPreview,
  pending,
  onAnswer,
  onSend,
  onStop,
  onTyping,
  handoff,
  autonomyNote,
  pulse,
  keys,
  hero = false,
  prefill = null,
  autoFocus = false,
}: ComposerProps) {
  const [draft, setDraft] = useState(workspace.composer.draft);
  const [focused, setFocused] = useState(false);
  const [handoffOpen, setHandoffOpen] = useState(false);
  const areaRef = useRef<HTMLTextAreaElement>(null);
  const pillRef = useRef<HTMLButtonElement>(null);
  const chipRef = useRef<HTMLButtonElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const placement = hero ? "below" : "above";

  // A press anywhere outside the composer and its menus closes them.
  useEffect(() => {
    if (!pickerOpen && !autonomyOpen) return;
    const onPress = (event: PointerEvent): void => {
      const wrap = wrapRef.current;
      if (wrap && event.target instanceof Node && wrap.contains(event.target)) return;
      onPickerOpen(false);
      onAutonomyOpen(false);
    };
    window.addEventListener("pointerdown", onPress, true);
    return () => window.removeEventListener("pointerdown", onPress, true);
  }, [pickerOpen, autonomyOpen, onPickerOpen, onAutonomyOpen]);

  // Command-/ opens and closes the picker from anywhere (WebKit's Tab skips buttons by default).
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (!(event.metaKey || event.ctrlKey) || event.key !== "/") return;
      event.preventDefault();
      onAutonomyOpen(false);
      onPickerOpen(!pickerOpen);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pickerOpen, onPickerOpen, onAutonomyOpen]);

  // A menu that closes with focus inside it hands focus back to the control that opened it.
  const wasOpen = useRef({ picker: pickerOpen, autonomy: autonomyOpen });
  useLayoutEffect(() => {
    const before = wasOpen.current;
    wasOpen.current = { picker: pickerOpen, autonomy: autonomyOpen };
    const active = document.activeElement;
    const inside = (selector: string): boolean =>
      active instanceof Node && Boolean(wrapRef.current?.querySelector(selector)?.contains(active));
    if (before.picker && !pickerOpen && inside(".au-picker")) {
      pillRef.current?.focus({ preventScroll: true });
    }
    if (before.autonomy && !autonomyOpen && inside(".au-menu")) {
      chipRef.current?.focus({ preventScroll: true });
    }
  }, [pickerOpen, autonomyOpen]);

  const [chipLeft, setChipLeft] = useState(0);
  // The autonomy menu opens over its chip, wherever the pill's label left it.
  useLayoutEffect(() => {
    if (autonomyOpen && chipRef.current) setChipLeft(chipRef.current.offsetLeft);
  }, [autonomyOpen]);

  const inputId = useId();
  const agent = findAgent(workspace, selection.agent);
  const agentName = agent?.name ?? getBrand(selection.agent).name;
  const label = composerLabel(workspace, { ...workspace.composer, ...selection, autonomy });
  const typing = draft.trim() !== "";

  useEffect(() => {
    onTyping(typing);
  }, [typing, onTyping]);

  // Only text offered after this composer mounted: a composer that takes over (the dock after the
  // hero) must not pick up a suggestion that was already sent.
  const seenPrefill = useRef(prefill?.nonce ?? 0);
  const focusOnMount = useRef(autoFocus);
  useEffect(() => {
    if (focusOnMount.current) areaRef.current?.focus({ preventScroll: true });
  }, []);
  useEffect(() => {
    if (!prefill || prefill.nonce <= seenPrefill.current) return;
    seenPrefill.current = prefill.nonce;
    setDraft(prefill.text);
    const area = areaRef.current;
    if (area) {
      area.focus();
      area.setSelectionRange(prefill.text.length, prefill.text.length);
    }
  }, [prefill]);

  // Grows with the text, up to eight lines; then it scrolls.
  // biome-ignore lint/correctness/useExhaustiveDependencies: it measures again whenever the text changes.
  useLayoutEffect(() => {
    const area = areaRef.current;
    if (!area) return;
    area.style.height = "0px";
    area.style.height = `${Math.min(area.scrollHeight, 8 * 24)}px`;
  }, [draft]);

  // A new handoff starts folded.
  const handoffKey = handoff ? `${handoff.from}>${handoff.to}` : null;
  // biome-ignore lint/correctness/useExhaustiveDependencies: it resets whenever the handoff changes.
  useEffect(() => setHandoffOpen(false), [handoffKey]);

  const send = (): void => {
    if (!typing) return;
    onSend(draft.trim());
    setDraft("");
  };

  const placeholder = pending
    ? "Enter to allow, Esc to reject, or type a reply"
    : hero
      ? `Ask ${agentName} to build, fix or explain`
      : `Message ${agentName}`;

  return (
    <div ref={wrapRef} className={cx("au-composer-wrap", hero && "au-composer-hero")}>
      <AnimatePresence>
        {pickerOpen ? (
          <Picker
            key="picker"
            workspace={workspace}
            selection={selection}
            memory={memory}
            onCommit={onCommit}
            onClose={() => onPickerOpen(false)}
            onPreview={onPreview}
            placement={placement}
          />
        ) : null}
        {autonomyOpen ? (
          <AutonomyMenu
            key="autonomy"
            left={Math.max(0, chipLeft - 10)}
            levels={agent?.autonomy ?? ["full_auto"]}
            value={autonomy}
            placement={placement}
            onPick={(level) => {
              onAutonomy(level);
              onAutonomyOpen(false);
            }}
            onClose={() => onAutonomyOpen(false)}
          />
        ) : null}
      </AnimatePresence>
      <Glass
        level={3}
        radius={16}
        frost
        className="au-composer"
        data-focus={focused ? "" : undefined}
      >
        <AnimatePresence initial={false}>
          {handoff || autonomyNote ? (
            <motion.div
              key="notes"
              className="au-composer-notes"
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={{ height: springs.smooth, opacity: { duration: 0.18 } }}
            >
              {handoff ? (
                <>
                  <div className="au-strip">
                    <HandoffOrbs from={handoff.from} to={handoff.to} />
                    <span className="au-strip-text">
                      Next message goes to <strong>{getBrand(handoff.to).name}</strong> with a
                      handoff
                    </span>
                    <button
                      type="button"
                      className="au-strip-btn"
                      aria-expanded={handoffOpen}
                      onClick={() => setHandoffOpen(!handoffOpen)}
                    >
                      Preview
                      <ChevronDown
                        className="au-chevron"
                        data-open={handoffOpen ? "" : undefined}
                        size={13}
                      />
                    </button>
                  </div>
                  <Collapse open={handoffOpen}>
                    <HandoffView handoff={handoff} />
                  </Collapse>
                </>
              ) : null}
              {autonomyNote ? (
                <p className="au-autonomy-note">
                  <Zap size={13} strokeWidth={2} />
                  {autonomyNote}
                </p>
              ) : null}
            </motion.div>
          ) : null}
        </AnimatePresence>
        <label className="au-sr" htmlFor={inputId}>
          Message {agentName}
        </label>
        <textarea
          ref={areaRef}
          id={inputId}
          className="au-input"
          rows={1}
          placeholder={placeholder}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return;
            const key = event.key;
            if (pending) {
              // The waiting card's keys, from where the cursor is. With text in the box, Enter
              // sends it (queued), so allowing takes Command too.
              if (key === "Enter" && !event.shiftKey && (!typing || event.metaKey)) {
                const option = optionForKey(pending.options, event.altKey ? "always" : "allow");
                if (option) {
                  event.preventDefault();
                  onAnswer(option);
                  return;
                }
              }
              if (key === "Escape") {
                const option = optionForKey(pending.options, "reject");
                if (option) {
                  event.preventDefault();
                  onAnswer(option);
                  return;
                }
              }
            }
            if (key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              send();
              return;
            }
            if (key === "Escape" && running) {
              event.preventDefault();
              onStop();
              return;
            }
            const transcript = keys.current;
            if (typing || !transcript) return;
            if (key === "PageUp" || key === "PageDown") {
              event.preventDefault();
              transcript.page(key === "PageUp" ? -1 : 1);
            } else if (event.metaKey && (key === "ArrowUp" || key === "ArrowDown")) {
              event.preventDefault();
              transcript.edge(key === "ArrowUp" ? -1 : 1);
            }
          }}
        />
        <div className="au-bar">
          <IconButton label="Attach files" tip="top" className="au-attach">
            <Plus size={17} strokeWidth={1.75} />
          </IconButton>
          <button
            ref={pillRef}
            type="button"
            className="au-pill"
            aria-haspopup="dialog"
            aria-expanded={pickerOpen}
            aria-keyshortcuts="Meta+/"
            data-tip="Agent, model and effort  ⌘/"
            data-tip-side="top"
            onClick={() => {
              onAutonomyOpen(false);
              onPickerOpen(!pickerOpen);
            }}
          >
            <Orb brand={selection.agent} state={orbState} size={18} label={agentName} />
            <span className="au-pill-label">{label}</span>
            <ChevronDown className="au-chevron" data-open={pickerOpen ? "" : undefined} size={14} />
          </button>
          <AutonomyChip
            buttonRef={chipRef}
            autonomy={autonomy}
            open={autonomyOpen}
            pulse={pulse}
            onClick={() => {
              onPickerOpen(false);
              onAutonomyOpen(!autonomyOpen);
            }}
          />
          <span className="au-bar-spacer" />
          {hero ? (
            <span className="au-send-hint">
              <CornerDownLeft size={12} strokeWidth={2} /> to send
            </span>
          ) : null}
          <SendStop running={running} canSend={typing} onSend={send} onStop={onStop} />
        </div>
      </Glass>
    </div>
  );
}

function AutonomyChip({
  buttonRef,
  autonomy,
  open,
  pulse,
  onClick,
}: {
  readonly buttonRef: RefObject<HTMLButtonElement | null>;
  readonly autonomy: Autonomy;
  readonly open: boolean;
  readonly pulse: number;
  readonly onClick: () => void;
}) {
  const Icon = AUTONOMY_ICON[autonomy];
  const reduced = usePrefersReducedMotion();
  const [scope, animate] = useAnimate<HTMLSpanElement>();
  const seen = useRef(pulse);
  // One pulse when the app changed the level for the user (switching to an agent that cannot keep
  // it), so the change never goes unseen.
  useEffect(() => {
    if (pulse === seen.current) return;
    seen.current = pulse;
    if (reduced || !scope.current) return;
    void animate(
      scope.current,
      { scale: [1, 1.08, 1] },
      { duration: 0.46, ease: easings.standard },
    );
  }, [pulse, reduced, animate, scope]);
  return (
    <span ref={scope} className="au-autonomy-wrap">
      <button
        ref={buttonRef}
        type="button"
        className="au-autonomy"
        data-level={autonomy}
        aria-haspopup="menu"
        aria-expanded={open}
        data-tip={AUTONOMY_HINT[autonomy]}
        data-tip-side="top"
        onClick={onClick}
      >
        <Icon size={12} strokeWidth={2} />
        {AUTONOMY_LABELS[autonomy]}
      </button>
    </span>
  );
}

function AutonomyMenu({
  left,
  levels,
  value,
  placement,
  onPick,
  onClose,
}: {
  /** Distance from the composer's left edge, so the menu sits over its chip. */
  readonly left: number;
  readonly levels: readonly Autonomy[];
  readonly value: Autonomy;
  readonly placement: "above" | "below";
  readonly onPick: (level: Autonomy) => void;
  readonly onClose: () => void;
}) {
  const menuRef = useRef<HTMLElement>(null);
  // Focus starts on the chosen level; arrows move between the levels this agent can use.
  useLayoutEffect(() => {
    menuRef.current
      ?.querySelector<HTMLElement>('[aria-checked="true"]')
      ?.focus({ preventScroll: true });
  }, []);
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.stopPropagation();
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      const items = [
        ...(menuRef.current?.querySelectorAll<HTMLButtonElement>(".au-menu-item:not(:disabled)") ??
          []),
      ];
      if (items.length === 0) return;
      event.stopPropagation();
      event.preventDefault();
      const index = items.indexOf(document.activeElement as HTMLButtonElement);
      const next =
        items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length];
      next?.focus({ preventScroll: true });
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);
  const below = placement === "below";
  return (
    <motion.div
      className="au-menu-pop"
      data-placement={placement}
      style={{ left }}
      initial={{ opacity: 0, y: below ? -8 : 8, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: below ? -4 : 4, scale: 0.98, transition: { duration: 0.12 } }}
      transition={springs.snappy}
    >
      <Glass
        ref={menuRef}
        level={3}
        radius={16}
        frost
        className="au-menu"
        role="menu"
        aria-label="Autonomy"
      >
        {AUTONOMY_ORDER.map((level) => {
          const Icon = AUTONOMY_ICON[level];
          const supported = levels.includes(level);
          return (
            <button
              key={level}
              type="button"
              role="menuitemradio"
              aria-checked={level === value}
              className="au-menu-item"
              disabled={!supported}
              onClick={() => onPick(level)}
            >
              <span className="au-menu-icon" data-level={level}>
                <Icon size={14} strokeWidth={1.75} />
              </span>
              <span className="au-option-text">
                <span className="au-option-label">{AUTONOMY_LABELS[level]}</span>
                <span className="au-option-sub">
                  {supported ? AUTONOMY_HINT[level] : "This agent never asks"}
                </span>
              </span>
              <Check
                className="au-option-check"
                data-off={level === value ? undefined : ""}
                size={14}
                strokeWidth={2.4}
                aria-hidden="true"
              />
            </button>
          );
        })}
      </Glass>
    </motion.div>
  );
}

/** Send, which becomes Stop while the agent works (a ring circles it as it does). */
function SendStop({
  running,
  canSend,
  onSend,
  onStop,
}: {
  readonly running: boolean;
  readonly canSend: boolean;
  readonly onSend: () => void;
  readonly onStop: () => void;
}) {
  const pop = {
    initial: { opacity: 0, scale: 0.6 },
    animate: { opacity: 1, scale: 1 },
    exit: { opacity: 0, scale: 0.6 },
    transition: springs.snappy,
  };
  return (
    <div className="au-send-slot">
      <AnimatePresence initial={false} mode="popLayout">
        {running && canSend ? (
          <motion.button
            key="queue"
            type="button"
            className="au-queue"
            onClick={onSend}
            data-tip="Sends when this turn ends"
            data-tip-side="top"
            {...pop}
          >
            Queue
          </motion.button>
        ) : null}
        {running ? (
          <motion.button
            key="stop"
            type="button"
            className="au-send"
            data-kind="stop"
            aria-label="Stop"
            data-tip="Stop  esc"
            data-tip-side="top"
            onClick={onStop}
            {...pop}
          >
            <span className="au-stop-ring" aria-hidden="true" />
            <Square size={10} fill="currentColor" strokeWidth={0} />
          </motion.button>
        ) : (
          <motion.button
            key="send"
            type="button"
            className="au-send"
            data-kind="send"
            aria-label="Send"
            disabled={!canSend}
            onClick={onSend}
            {...pop}
          >
            <ArrowUp size={17} strokeWidth={2} />
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}
