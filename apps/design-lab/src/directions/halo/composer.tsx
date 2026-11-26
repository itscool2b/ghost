import {
  getBrand,
  monogram,
  Orb,
  type OrbState,
  orbPosterBackground,
  springs,
  usePrefersReducedMotion,
} from "@ghost/ui";
import {
  ArrowUp,
  Check,
  ChevronDown,
  Download,
  FilePen,
  Hand,
  type LucideIcon,
  Plus,
  Square,
  Zap,
} from "lucide-react";
import { AnimatePresence, animate, motion } from "motion/react";
import {
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import {
  type AgentInfo,
  AUTONOMY_LABELS,
  type Autonomy,
  type Composer as ComposerModel,
  composerLabel,
  findAgent,
  findModel,
  type Workspace,
} from "../../kit";
import { focusKind } from "./keys";
import { cx, Glass, Kbd, RADIUS, trapTab } from "./primitives";

/** Who the next message goes to, and how: the agent, model, effort and autonomy picked here. */
export type Selection = Pick<ComposerModel, "agent" | "model" | "effort" | "autonomy">;

const AUTONOMY_ICONS: Readonly<Record<Autonomy, LucideIcon>> = {
  full_auto: Zap,
  auto_edit: FilePen,
  ask: Hand,
};

/** What an effort level means, in a few words, under the effort column. */
const EFFORT_NOTES: Readonly<Record<string, string>> = {
  off: "Answers without reasoning first",
  minimal: "Barely reasons, answers fast",
  low: "Light reasoning, quick answers",
  medium: "Balanced speed and depth",
  high: "Thinks longer before it acts",
  xhigh: "Deep reasoning for hard problems",
  max: "Deepest reasoning, slowest and costliest",
};

const AUTONOMY_LINES: Readonly<Record<Autonomy, string>> = {
  full_auto: "Edits and runs commands without asking",
  auto_edit: "Edits files, asks before commands",
  ask: "Asks before every change",
};

/** "Opus" for "Opus 5.5": the pill names the model family, the picker the exact model. */
function shortModel(label: string): string {
  return label.replace(/\s+\d+(?:\.\d+)*$/, "") || label;
}

/** The pill's words: "Claude Code · Opus · High". */
function pillLabel(workspace: Workspace, selection: Selection): string {
  const agent = findAgent(workspace, selection.agent);
  const model = findModel(agent, selection.model);
  const effort =
    model?.efforts.find((candidate) => candidate.id === selection.effort)?.label ??
    selection.effort ??
    "";
  return [agent?.name ?? selection.agent, model ? shortModel(model.label) : selection.model, effort]
    .filter((part) => part !== "")
    .join(" · ");
}

type SendMode = "send" | "queue" | "stop";

const SEND_COPY: Readonly<Record<SendMode, { readonly label: string; readonly title: string }>> = {
  send: { label: "Send", title: "Send (⏎)" },
  queue: { label: "Queue message", title: "Queue (⏎)" },
  stop: { label: "Stop", title: "Stop (esc)" },
};

/**
 * The agent's orb as the send control, wrapped in its halo: a ring of the agent's light with a
 * soft bloom. It turns while the agent works, holds still and breathes amber while the agent waits
 * on you, and rests lit when idle. While working it is Stop, or Queue once you type.
 */
function SendOrb({
  agent,
  state,
  mode,
  working,
  disabled,
  onPress,
}: {
  readonly agent: string;
  readonly state: OrbState;
  readonly mode: SendMode;
  readonly working: boolean;
  readonly disabled: boolean;
  readonly onPress: () => void;
}) {
  const copy = SEND_COPY[mode];
  return (
    <button
      type="button"
      className="halo-send"
      data-mode={mode}
      data-state={state}
      data-working={working ? "" : undefined}
      aria-label={copy.label}
      title={copy.title}
      disabled={disabled}
      onClick={onPress}
    >
      <span className="halo-send-glow" aria-hidden="true" />
      <span className="halo-send-bloom" aria-hidden="true" />
      <span className="halo-send-ring" aria-hidden="true" />
      <Orb brand={agent} state={state} size={44} className="halo-send-orb" />
      <span className="halo-send-glyph" aria-hidden="true">
        {mode === "stop" ? (
          <Square size={12} fill="currentColor" strokeWidth={0} />
        ) : (
          <ArrowUp size={18} strokeWidth={2.4} />
        )}
      </span>
    </button>
  );
}

/**
 * A floating menu that opens upward from its trigger with a spring. It holds focus while open (Tab
 * cycles inside it). The blurred glass itself fades, not a wrapper around it: a fading parent
 * would cut the glass off from what is behind and the blur would snap in at the end.
 */
function Popover({
  className,
  label,
  role,
  radius = RADIUS.popover,
  origin = "bottom right",
  innerRef,
  onKeyDown,
  children,
}: {
  readonly className: string;
  readonly label: string;
  readonly role?: "dialog";
  readonly radius?: number;
  readonly origin?: string;
  readonly innerRef: RefObject<HTMLDivElement | null>;
  readonly onKeyDown?: (event: KeyboardEvent<HTMLDivElement>) => void;
  readonly children: ReactNode;
}) {
  const reduced = usePrefersReducedMotion();
  return (
    <motion.div
      className={cx("halo-popover", className)}
      initial={reduced ? false : { y: 10, scale: 0.96 }}
      animate={{ y: 0, scale: 1 }}
      exit={reduced ? { y: 0 } : { y: 6, scale: 0.98 }}
      transition={reduced ? { duration: 0 } : springs.snappy}
      style={{ transformOrigin: origin }}
    >
      <motion.div
        ref={innerRef}
        className="halo-popover-inner"
        role={role}
        aria-label={role ? label : undefined}
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        transition={{ duration: reduced ? 0.16 : 0.18, ease: [0.2, 0, 0, 1] }}
        onKeyDown={(event) => {
          if (innerRef.current) trapTab(event, innerRef.current);
          onKeyDown?.(event);
        }}
      >
        <Glass material="popover" radius={radius} />
        {children}
      </motion.div>
    </motion.div>
  );
}

/** Moves focus to the next or previous enabled row in a list, wrapping at the ends. */
function moveFocus(rows: readonly HTMLElement[], from: Element | null, step: number): void {
  if (rows.length === 0) return;
  const index = rows.indexOf(from as HTMLElement);
  const next =
    index === -1 ? (step > 0 ? 0 : rows.length - 1) : (index + step + rows.length) % rows.length;
  rows[next]?.focus();
}

/**
 * While a menu is open but focus is elsewhere (it opened on its own), the arrow keys bring focus
 * into it at its chosen row.
 */
function useArrowsEnter(menuRef: RefObject<HTMLDivElement | null>, chosen: string): void {
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (!["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"].includes(event.key)) return;
      const menu = menuRef.current;
      const active = document.activeElement;
      if (!menu || (active && menu.contains(active))) return;
      const kind = focusKind(active);
      if (kind === "composer-text" || kind === "field") return;
      // The pill's own ArrowUp opens the picker; here it only moves focus in.
      event.preventDefault();
      menu.querySelector<HTMLElement>(chosen)?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [menuRef, chosen]);
}

function AutonomyMenu({
  agent,
  value,
  autoFocus,
  onChange,
}: {
  readonly agent: AgentInfo | null;
  readonly value: Autonomy;
  readonly autoFocus: boolean;
  readonly onChange: (value: Autonomy) => void;
}) {
  const levels: readonly Autonomy[] = ["full_auto", "auto_edit", "ask"];
  const supported = agent?.autonomy ?? levels;
  const innerRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const checked = "[role='menuitemradio'][aria-checked='true']";
  useLayoutEffect(() => {
    if (autoFocus) innerRef.current?.querySelector<HTMLElement>(checked)?.focus();
  }, [autoFocus]);
  useArrowsEnter(innerRef, checked);
  return (
    <Popover
      className="halo-autonomy-menu"
      label="Autonomy"
      origin="bottom center"
      innerRef={innerRef}
      onKeyDown={(event) => {
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
        event.preventDefault();
        const rows = [
          ...event.currentTarget.querySelectorAll<HTMLElement>(
            "[role='menuitemradio']:not(:disabled)",
          ),
        ];
        moveFocus(rows, document.activeElement, event.key === "ArrowDown" ? 1 : -1);
      }}
    >
      <p id={titleId} className="halo-popover-title">
        Autonomy
      </p>
      <div className="halo-menu-list" role="menu" aria-labelledby={titleId}>
        {levels.map((level) => {
          const Icon = AUTONOMY_ICONS[level];
          const available = supported.includes(level);
          return (
            <button
              key={level}
              type="button"
              role="menuitemradio"
              className="halo-menu-row"
              data-halo-light=""
              aria-checked={level === value}
              tabIndex={level === value ? 0 : -1}
              disabled={!available}
              onClick={() => onChange(level)}
            >
              <span className="halo-menu-icon" data-level={level}>
                <Icon size={14} aria-hidden="true" />
              </span>
              <span className="halo-menu-text">
                <span className="halo-menu-label">{AUTONOMY_LABELS[level]}</span>
                <span className="halo-menu-sub">
                  {available ? AUTONOMY_LINES[level] : `${agent?.name ?? "This agent"} never asks`}
                </span>
              </span>
              {level === value ? (
                <Check size={15} className="halo-menu-check" aria-hidden="true" />
              ) : null}
            </button>
          );
        })}
      </div>
    </Popover>
  );
}

/** Effort as a little level meter: one bar per step, lit up to this one. */
function EffortBars({ level, of }: { readonly level: number; readonly of: number }) {
  return (
    <span className="halo-effort-bars" aria-hidden="true">
      {Array.from({ length: Math.max(of, 1) }, (_, index) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: bars are positional.
          key={index}
          data-on={index <= level ? "" : undefined}
          style={{ height: 4 + Math.round((index / Math.max(of - 1, 1)) * 8) }}
        />
      ))}
    </span>
  );
}

const OPTION = "[role='option']";

/** Arrow keys inside the picker: up and down within a column, left and right across columns. */
function onPickerKeys(event: KeyboardEvent<HTMLDivElement>): void {
  const target = event.target instanceof HTMLElement ? event.target : null;
  const list = target?.closest<HTMLElement>("[role='listbox']");
  if (!target || !list) return;
  const rows = [...list.querySelectorAll<HTMLElement>(OPTION)];
  switch (event.key) {
    case "ArrowDown":
    case "ArrowUp":
      event.preventDefault();
      moveFocus(rows, target, event.key === "ArrowDown" ? 1 : -1);
      return;
    case "Home":
      event.preventDefault();
      rows[0]?.focus();
      return;
    case "End":
      event.preventDefault();
      rows[rows.length - 1]?.focus();
      return;
    case "ArrowLeft":
    case "ArrowRight": {
      event.preventDefault();
      const lists = [...event.currentTarget.querySelectorAll<HTMLElement>("[role='listbox']")];
      const next = lists[lists.indexOf(list) + (event.key === "ArrowRight" ? 1 : -1)];
      const row =
        next?.querySelector<HTMLElement>(`${OPTION}[aria-selected='true']`) ??
        next?.querySelector<HTMLElement>(OPTION);
      row?.focus();
      return;
    }
    default:
  }
}

/** One picker column: a titled list of options where one is chosen. */
function PickerColumn({
  title,
  children,
  after,
}: {
  readonly title: string;
  readonly children: ReactNode;
  readonly after?: ReactNode;
}) {
  const titleId = useId();
  return (
    <div className="halo-picker-col">
      <p id={titleId} className="halo-popover-title">
        {title}
      </p>
      <div className="halo-picker-list" role="listbox" aria-labelledby={titleId}>
        {children}
      </div>
      {after}
    </div>
  );
}

/**
 * Agent > model > effort, side by side: every agent with its live orb, the chosen agent's models,
 * the chosen model's efforts. Arrow keys move, Enter chooses: choosing is never the same as moving,
 * since picking an agent cross-fades the whole window into its colour.
 */
function Picker({
  workspace,
  selection,
  autoFocus,
  onSelect,
  onClose,
}: {
  readonly workspace: Workspace;
  readonly selection: Selection;
  readonly autoFocus: boolean;
  readonly onSelect: (next: Selection) => void;
  readonly onClose: () => void;
}) {
  const innerRef = useRef<HTMLDivElement>(null);
  const agent = findAgent(workspace, selection.agent);
  const model = findModel(agent, selection.model);
  const efforts = model?.efforts ?? [];
  const effortNote =
    selection.effort && Object.hasOwn(EFFORT_NOTES, selection.effort)
      ? EFFORT_NOTES[selection.effort]
      : undefined;
  const chosenAgent = `.halo-pick-agent${OPTION}[aria-selected='true']`;
  useLayoutEffect(() => {
    if (autoFocus) innerRef.current?.querySelector<HTMLElement>(chosenAgent)?.focus();
  }, [autoFocus, chosenAgent]);
  useArrowsEnter(innerRef, chosenAgent);

  const pickAgent = (next: AgentInfo): void => {
    if (!next.installed || next.id === selection.agent) return;
    const nextModel = findModel(next, next.defaultModel) ?? next.models[0] ?? null;
    onSelect({
      agent: next.id,
      model: nextModel?.id ?? "",
      effort: nextModel?.defaultEffort ?? null,
      autonomy: next.autonomy.includes(selection.autonomy)
        ? selection.autonomy
        : (next.autonomy[0] ?? "full_auto"),
    });
  };

  return (
    <Popover
      className="halo-picker"
      label="Agent, model and effort"
      role="dialog"
      radius={RADIUS.capsule}
      innerRef={innerRef}
      onKeyDown={onPickerKeys}
    >
      <div className="halo-picker-cols">
        <PickerColumn title="Agent">
          {workspace.agents.map((item) => {
            const chosen = item.id === selection.agent;
            return (
              <button
                key={item.id}
                type="button"
                role="option"
                className="halo-pick-row halo-pick-agent"
                data-halo-light=""
                aria-selected={chosen}
                aria-disabled={item.installed ? undefined : true}
                tabIndex={chosen ? 0 : -1}
                data-missing={item.installed ? undefined : ""}
                onClick={() => pickAgent(item)}
              >
                {item.installed ? (
                  <Orb brand={item.id} state="idle" size={26} className="halo-pick-orb" />
                ) : (
                  <span className="halo-pick-ghost" aria-hidden="true">
                    {monogram(item.name)}
                  </span>
                )}
                <span className="halo-pick-text">
                  <span className="halo-pick-label">{item.name}</span>
                  <span className="halo-pick-sub">
                    {item.installed ? item.version : "Not installed"}
                  </span>
                </span>
                {item.update ? <span className="halo-pick-badge">Update</span> : null}
                {!item.installed ? (
                  <span className="halo-pick-install">
                    <Download size={12} aria-hidden="true" />
                    Install
                  </span>
                ) : null}
                {chosen ? <Check size={15} className="halo-menu-check" aria-hidden="true" /> : null}
              </button>
            );
          })}
        </PickerColumn>
        <PickerColumn title="Model">
          {agent?.models.map((item) => {
            const chosen = item.id === selection.model;
            return (
              <button
                key={item.id}
                type="button"
                role="option"
                className="halo-pick-row halo-pick-model"
                data-halo-light=""
                aria-selected={chosen}
                tabIndex={chosen ? 0 : -1}
                onClick={() =>
                  onSelect({
                    ...selection,
                    model: item.id,
                    effort: item.efforts.some((effort) => effort.id === selection.effort)
                      ? selection.effort
                      : item.defaultEffort,
                  })
                }
              >
                <span className="halo-pick-text">
                  <span className="halo-pick-label">{item.label}</span>
                  <span className="halo-pick-sub">{item.description}</span>
                </span>
                {chosen ? <Check size={15} className="halo-menu-check" aria-hidden="true" /> : null}
              </button>
            );
          })}
        </PickerColumn>
        <PickerColumn
          title="Effort"
          after={
            efforts.length === 0 ? (
              <p className="halo-pick-empty">This model has no effort setting.</p>
            ) : effortNote ? (
              <p className="halo-pick-note">{effortNote}</p>
            ) : null
          }
        >
          {efforts.map((item, index) => {
            const chosen = item.id === selection.effort;
            return (
              <button
                key={item.id}
                type="button"
                role="option"
                className="halo-pick-row halo-pick-effort"
                data-halo-light=""
                aria-selected={chosen}
                tabIndex={chosen || (selection.effort === null && index === 0) ? 0 : -1}
                onClick={() => {
                  onSelect({ ...selection, effort: item.id });
                  onClose();
                }}
              >
                <EffortBars level={index} of={efforts.length} />
                <span className="halo-pick-label">{item.label}</span>
                {chosen ? <Check size={15} className="halo-menu-check" aria-hidden="true" /> : null}
              </button>
            );
          })}
        </PickerColumn>
      </div>
      <footer className="halo-picker-foot" aria-hidden="true">
        <span>
          <Kbd>↑</Kbd>
          <Kbd>↓</Kbd> move
        </span>
        <span>
          <Kbd>←</Kbd>
          <Kbd>→</Kbd> column
        </span>
        <span>
          <Kbd>⏎</Kbd> choose
        </span>
        <span>
          <Kbd>esc</Kbd> close
        </span>
      </footer>
    </Popover>
  );
}

/**
 * Moves focus back to a trigger when its menu closes with focus inside (Escape, or a choice made),
 * so the keyboard never lands on the page body.
 */
function useReturnFocus(
  open: boolean,
  menuSelector: string,
  trigger: RefObject<HTMLButtonElement | null>,
): void {
  const wasOpen = useRef(open);
  useEffect(() => {
    if (wasOpen.current && !open) {
      const active = document.activeElement;
      if (active instanceof HTMLElement && active.closest(menuSelector)) {
        trigger.current?.focus({ preventScroll: true });
      }
    }
    wasOpen.current = open;
  }, [open, menuSelector, trigger]);
}

/**
 * Animates the capsule's height when it changes between one row and two, from the height it had
 * to the height it now needs, so the controls glide down instead of jumping. Content sits at the
 * bottom while it grows, so the text slides up into its own row.
 */
function useCapsuleFlip(capsuleRef: RefObject<HTMLDivElement | null>, multiline: boolean): void {
  const reduced = usePrefersReducedMotion();
  const lastHeight = useRef<number | null>(null);
  const flipping = useRef(false);

  // The capsule's height as last laid out, kept current outside of a flip.
  useLayoutEffect(() => {
    const capsule = capsuleRef.current;
    if (!capsule) return;
    lastHeight.current = capsule.offsetHeight;
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver(() => {
      if (!flipping.current) lastHeight.current = capsule.offsetHeight;
    });
    observer.observe(capsule);
    return () => observer.disconnect();
  }, [capsuleRef]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the layout flips between one row and two.
  useLayoutEffect(() => {
    const capsule = capsuleRef.current;
    const from = lastHeight.current;
    if (!capsule || from === null) return;
    const to = capsule.offsetHeight;
    if (reduced || Math.abs(to - from) < 1) {
      lastHeight.current = to;
      return;
    }
    flipping.current = true;
    capsule.setAttribute("data-flipping", "");
    const controls = animate(capsule, { height: [`${from}px`, `${to}px`] }, springs.smooth);
    const done = (): void => {
      flipping.current = false;
      capsule.removeAttribute("data-flipping");
      capsule.style.height = "";
      lastHeight.current = capsule.offsetHeight;
    };
    controls.then(done, done);
    return () => {
      controls.stop();
      done();
    };
  }, [multiline]);
}

/**
 * The composer: a capsule floating nearest of everything, holding the message, the agent pill
 * (agent > model > effort), the autonomy chip, and the agent's orb as Send. While the agent works,
 * Send becomes Stop (Queue once you type) and the halo turns; while it waits on a permission, what
 * you type replaces the request.
 */
export function Composer({
  workspace,
  selection,
  onSelect,
  activity,
  working,
  waitingFor,
  pickerOpen,
  onPickerOpen,
  menuOpen,
  onMenuOpen,
  hero = false,
  onStop,
  onQueue,
  onReplace,
}: {
  readonly workspace: Workspace;
  readonly selection: Selection;
  readonly onSelect: (next: Selection) => void;
  readonly activity: OrbState;
  readonly working: boolean;
  /** The agent whose permission request is pending, if one is. */
  readonly waitingFor: string | null;
  readonly pickerOpen: boolean;
  readonly onPickerOpen: (open: boolean) => void;
  readonly menuOpen: boolean;
  readonly onMenuOpen: (open: boolean) => void;
  readonly hero?: boolean;
  readonly onStop: () => void;
  /** A message typed while the agent works: it waits until the turn ends. */
  readonly onQueue: (text: string) => void;
  /** A message typed while a permission waits: it rejects the request and goes instead. */
  readonly onReplace: (text: string) => void;
}) {
  const agent = findAgent(workspace, selection.agent);
  const [draft, setDraft] = useState(workspace.composer.draft);
  // Whether a menu was opened by you (click or keys): then focus moves into it.
  const [pickerFocus, setPickerFocus] = useState(false);
  const [menuFocus, setMenuFocus] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const capsuleRef = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);
  const pillRef = useRef<HTMLButtonElement>(null);
  const chipRef = useRef<HTMLButtonElement>(null);
  const AutonomyIcon = AUTONOMY_ICONS[selection.autonomy];
  const typing = draft.trim() !== "";
  const mode: SendMode = typing
    ? waitingFor || !working
      ? "send"
      : "queue"
    : working
      ? "stop"
      : "send";
  const name = agent?.name ?? "the agent";

  /*
   * Grows with the text, up to six lines. Once the text wraps in the capsule, the composer opens
   * into two rows (text above, controls below) and stays open until the draft is empty again, so
   * it never flickers between the two at the wrap point.
   */
  const [multiline, setMultiline] = useState(false);
  // biome-ignore lint/correctness/useExhaustiveDependencies: measured again whenever the text or the layout changes.
  useLayoutEffect(() => {
    const area = textRef.current;
    if (!area) return;
    area.style.height = "0px";
    const height = area.scrollHeight;
    area.style.height = `${Math.min(height, 156)}px`;
    if (draft === "") setMultiline(false);
    else if (draft.includes("\n") || height > 30) setMultiline(true);
  }, [draft, multiline]);
  useCapsuleFlip(capsuleRef, multiline);

  useReturnFocus(pickerOpen, ".halo-picker", pillRef);
  useReturnFocus(menuOpen, ".halo-autonomy-menu", chipRef);
  useEffect(() => {
    if (!pickerOpen) setPickerFocus(false);
  }, [pickerOpen]);
  useEffect(() => {
    if (!menuOpen) setMenuFocus(false);
  }, [menuOpen]);

  // A press anywhere outside closes the open menu.
  useEffect(() => {
    if (!pickerOpen && !menuOpen) return;
    const onPointerDown = (event: PointerEvent): void => {
      if (rootRef.current?.contains(event.target as Node)) return;
      onPickerOpen(false);
      onMenuOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [pickerOpen, menuOpen, onPickerOpen, onMenuOpen]);

  const openPicker = (): void => {
    setPickerFocus(true);
    onMenuOpen(false);
    onPickerOpen(true);
  };

  const submit = (): void => {
    const text = draft.trim();
    if (text === "") return;
    if (waitingFor) onReplace(text);
    else if (working) onQueue(text);
    // A new message: in the lab it only clears the field.
    setDraft("");
  };

  return (
    <div
      ref={rootRef}
      className="halo-composer-wrap"
      data-hero={hero ? "" : undefined}
      style={{ "--composer-accent": getBrand(selection.agent).accent } as CSSProperties}
    >
      <AnimatePresence>
        {pickerOpen ? (
          <Picker
            key="picker"
            workspace={workspace}
            selection={selection}
            autoFocus={pickerFocus}
            onSelect={onSelect}
            onClose={() => onPickerOpen(false)}
          />
        ) : null}
        {menuOpen ? (
          <AutonomyMenu
            key="autonomy"
            agent={agent}
            value={selection.autonomy}
            autoFocus={menuFocus}
            onChange={(autonomy) => {
              onSelect({ ...selection, autonomy });
              onMenuOpen(false);
            }}
          />
        ) : null}
      </AnimatePresence>
      <div
        ref={capsuleRef}
        className="halo-composer"
        data-halo-light=""
        data-working={working ? "" : undefined}
        data-multiline={multiline ? "" : undefined}
      >
        <Glass material="capsule" radius={RADIUS.capsule} />
        <button type="button" className="halo-attach" aria-label="Attach files" title="Attach">
          <Plus size={18} aria-hidden="true" />
        </button>
        <textarea
          ref={textRef}
          className="halo-input"
          rows={1}
          value={draft}
          aria-label="Message"
          placeholder={
            waitingFor
              ? `Or tell ${waitingFor} what to do instead`
              : working
                ? "Queue a follow-up"
                : `Message ${name}`
          }
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault();
              submit();
            }
          }}
        />
        <div className="halo-composer-tools">
          <button
            ref={pillRef}
            type="button"
            className="halo-agent-pill"
            data-halo-light=""
            aria-haspopup="dialog"
            aria-expanded={pickerOpen}
            title={composerLabel(workspace, { ...workspace.composer, ...selection })}
            onClick={() => (pickerOpen ? onPickerOpen(false) : openPicker())}
            onKeyDown={(event) => {
              if (event.key === "ArrowUp" && !pickerOpen) {
                event.preventDefault();
                openPicker();
              }
            }}
          >
            <span
              className="halo-pill-orb"
              style={
                { "--pill-orb": orbPosterBackground(getBrand(selection.agent)) } as CSSProperties
              }
              aria-hidden="true"
            />
            <span className="halo-pill-label">{pillLabel(workspace, selection)}</span>
            <ChevronDown size={14} className="halo-chevron" aria-hidden="true" />
          </button>
          <button
            ref={chipRef}
            type="button"
            className="halo-autonomy"
            data-halo-light=""
            data-level={selection.autonomy}
            data-compact={typing ? "" : undefined}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            title={`${AUTONOMY_LABELS[selection.autonomy]}: ${AUTONOMY_LINES[selection.autonomy]}`}
            onClick={() => {
              if (menuOpen) {
                onMenuOpen(false);
                return;
              }
              setMenuFocus(true);
              onPickerOpen(false);
              onMenuOpen(true);
            }}
          >
            <AutonomyIcon
              size={13}
              aria-hidden="true"
              fill={selection.autonomy === "full_auto" ? "currentColor" : "none"}
            />
            <span className="halo-autonomy-label">{AUTONOMY_LABELS[selection.autonomy]}</span>
          </button>
        </div>
        <SendOrb
          agent={selection.agent}
          state={activity}
          mode={mode}
          working={working}
          disabled={mode === "send" && !typing}
          onPress={mode === "stop" ? onStop : submit}
        />
      </div>
    </div>
  );
}
