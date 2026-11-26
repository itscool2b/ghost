import {
  easings,
  getBrand,
  type OrbState,
  orbPosterBackground,
  springs,
  usePrefersReducedMotion,
} from "@ghost/ui";
import type { LucideIcon } from "lucide-react";
import { AnimatePresence, motion, type TargetAndTransition, type Transition } from "motion/react";
import {
  type ButtonHTMLAttributes,
  type CSSProperties,
  createContext,
  type ReactNode,
  type RefObject,
  useContext,
  useEffect,
  useRef,
  useState,
} from "react";
import {
  type AgentInfo,
  type ChatStatus,
  formatElapsed,
  type Player,
  usePlayhead,
} from "../../kit";

/* ------------------------------------------------------------------------------------ motion */

/*
 * Noir's motion is crisp and short: opacity in 160 ms, a 6 px rise on the snappy spring, and
 * height changes as a quick ease-out tween (a spring on height would overshoot). Under reduced
 * motion nothing moves: rises become plain fades and height changes are instant.
 */
const FADE = { duration: 0.16, ease: easings.out };
const EXIT = { duration: 0.12, ease: easings.in };
const HEIGHT = { duration: 0.22, ease: easings.out };

/**
 * For motion that carries what the reader is reading (the transcript following new output, the
 * dock giving back the plan's room): a soft start and a soft landing, so a 140 px move never takes
 * more than about 16 px in one frame. An ease-out would put a third of it in the first frame.
 */
export const GLIDE = { duration: 0.32, ease: [0.3, 0, 0.2, 1] as [number, number, number, number] };

export interface Presence {
  readonly initial: TargetAndTransition | false;
  readonly animate: TargetAndTransition;
  readonly exit: TargetAndTransition;
  readonly transition: Transition;
}

/** A block entering the transcript: fades in while it rises 6 px into place. */
export function riseIn(reduced: boolean, enter = true): Presence {
  return {
    initial: enter ? (reduced ? { opacity: 0 } : { opacity: 0, y: 6 }) : false,
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0 },
    transition: reduced ? FADE : { opacity: FADE, y: springs.snappy },
  };
}

/** A region that opens and closes in place (tool details, the thought, the plan list). */
export function reveal(reduced: boolean, enter = true): Presence {
  return {
    initial: enter ? { height: 0, opacity: 0 } : false,
    animate: { height: "auto", opacity: 1 },
    exit: { height: 0, opacity: 0 },
    transition: reduced ? { duration: 0 } : { height: HEIGHT, opacity: FADE },
  };
}

/** A line of text replaced in place (a step's label): the old one fades out as the new one fades in. */
export function swap(reduced: boolean): Presence {
  const fade = reduced ? { duration: 0 } : { duration: 0.12, ease: easings.out };
  return {
    initial: { opacity: 0 },
    animate: { opacity: 1 },
    exit: { opacity: 0 },
    transition: fade,
  };
}

/** An overlay (menu, picker) appearing over the content: a fade and a 4 px settle. */
export function popIn(reduced: boolean): Presence {
  return {
    initial: reduced ? { opacity: 0 } : { opacity: 0, y: 4, scale: 0.985 },
    animate: { opacity: 1, y: 0, scale: 1 },
    // Leaving is quicker than arriving, and a tween: a spring's tail would hold it on screen.
    exit: reduced
      ? { opacity: 0, transition: EXIT }
      : { opacity: 0, y: 3, scale: 0.99, transition: EXIT },
    transition: reduced ? FADE : { opacity: FADE, y: springs.snappy, scale: springs.snappy },
  };
}

export { usePrefersReducedMotion as useReduced };

/*
 * Blocks already on screen when the transcript first renders appear without an entrance, so a
 * chat opened mid-way does not cascade in; only blocks that arrive afterwards fade and rise.
 */
const EntranceContext = createContext(false);
export const EntranceProvider = EntranceContext.Provider;

/** Whether this block should play its entrance: decided once, when it first renders. */
export function useEntrance(): boolean {
  const live = useContext(EntranceContext);
  const [enter] = useState(live);
  return enter;
}

/* --------------------------------------------------------------------------------- mini orbs */

const STATUS_ORB: Readonly<Record<ChatStatus, OrbState>> = {
  running: "thinking",
  needs_approval: "waiting",
  done_unseen: "idle",
  error: "error",
  idle: "idle",
};

/**
 * The CSS stand-in for an orb (the poster @ghost/ui draws before WebGL), for places too small
 * for a live one: sidebar rows, the dock, turn headers. Status adds a ring: a turning arc while
 * running, a breathing amber halo while it needs approval, a lit rim when done and unseen.
 */
export function MiniOrb({
  agent,
  status = "idle",
  size = 8,
  ghost = false,
  className,
}: {
  readonly agent: string;
  readonly status?: ChatStatus | "live";
  readonly size?: number;
  /** An empty ring: an agent that is not installed. */
  readonly ghost?: boolean;
  readonly className?: string;
}) {
  const brand = getBrand(agent);
  const orbState: OrbState = status === "live" ? "idle" : STATUS_ORB[status];
  return (
    <span
      aria-hidden="true"
      className={className ? `n-mini ${className}` : "n-mini"}
      data-status={status}
      data-ghost={ghost ? "" : undefined}
      style={
        {
          "--n-mini-size": `${size}px`,
          "--n-mini-glow": brand.glow,
          background: ghost ? undefined : orbPosterBackground(brand, orbState),
        } as CSSProperties
      }
    />
  );
}

/* ---------------------------------------------------------------------------- small controls */

export function Kbd({
  children,
  className,
}: {
  readonly children: ReactNode;
  readonly className?: string;
}) {
  return <kbd className={className ? `n-kbd ${className}` : "n-kbd"}>{children}</kbd>;
}

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  readonly icon: LucideIcon;
  readonly label: string;
  readonly size?: "sm" | "md";
  readonly iconSize?: number;
}

/** A quiet square button holding one icon. The label is its accessible name and tooltip. */
export function IconButton({
  icon: Icon,
  label,
  size = "md",
  iconSize,
  className,
  ...rest
}: IconButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={className ? `n-icon-btn ${className}` : "n-icon-btn"}
      data-size={size}
      {...rest}
    >
      <Icon size={iconSize ?? (size === "sm" ? 14 : 16)} strokeWidth={1.6} aria-hidden="true" />
    </button>
  );
}

/**
 * Update or Install, as a quiet chip of its own: a real button with its own Tab stop (in the dock
 * and the picker alike), with a small dot when an update waits.
 */
export function AgentAction({
  kind,
  agent,
}: {
  readonly kind: "update" | "install";
  readonly agent: AgentInfo;
}) {
  const label =
    kind === "update"
      ? `Update ${agent.name} to ${agent.update ?? "the latest version"}`
      : `Install ${agent.name}`;
  return (
    <button
      type="button"
      className="n-agent-action"
      data-kind={kind}
      aria-label={label}
      title={label}
    >
      {kind === "update" ? <span className="n-agent-action-dot" aria-hidden="true" /> : null}
      {kind === "update" ? "Update" : "Install"}
    </button>
  );
}

/** "+59 −23": lines added and removed, each in its colour, zeroes left out. */
export function DiffStat({ added, removed }: { readonly added: number; readonly removed: number }) {
  if (added === 0 && removed === 0) return null;
  return (
    <span className="n-diffstat">
      {added > 0 ? <span className="n-add">+{added}</span> : null}
      {removed > 0 ? <span className="n-del">{`−${removed}`}</span> : null}
    </span>
  );
}

/**
 * A live seconds counter from `since` (timeline ms). The playhead is read in quarter seconds, so
 * the counter turns within 250 ms of each whole second without re-rendering every frame.
 */
export function Elapsed({ player, since }: { readonly player: Player; readonly since: number }) {
  const now = usePlayhead(player, 250);
  return <span className="n-num">{formatElapsed(Math.max(0, now - since))}</span>;
}

/* ---------------------------------------------------------------------------------- overlays */

/**
 * Closes an overlay on Escape or a press outside it. Escape is caught in the capture phase and
 * marked handled, so a key meant for the overlay never also rejects a permission card below it.
 */
export function useDismiss(ref: RefObject<HTMLElement | null>, onClose: () => void): void {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const onPointer = (event: PointerEvent): void => {
      const element = ref.current;
      const target = event.target;
      if (!element || !(target instanceof Node) || element.contains(target)) return;
      // The control that opened the overlay toggles it itself.
      if (target instanceof Element && target.closest("[aria-haspopup]")) return;
      close.current();
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        close.current();
      }
    };
    window.addEventListener("pointerdown", onPointer, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("pointerdown", onPointer, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [ref]);
}

/* ------------------------------------------------------------------------------------ capture */

/**
 * True in Playwright's headless WebKit, which never paints `backdrop-filter`: its screenshots would
 * show sharp text through every blurred overlay, which no real WebKit does. Noir then gives its
 * overlays a denser fill there, and only there (Chromium shots, Safari and the app keep the glass).
 * A lab shim: it does not graduate into the app.
 */
export function captureWithoutBackdrop(): boolean {
  if (typeof navigator === "undefined" || !navigator.webdriver) return false;
  const agent = navigator.userAgent;
  return /AppleWebKit/.test(agent) && !/Chrome|Chromium|Edg/.test(agent);
}

/* ---------------------------------------------------------------------------------- expanding */

/*
 * The transcript's edges (noir.css, .n-scroll): content is fully clear in the title bar's band
 * (52 px) and in the 16 px above the dock, and fully opaque from 96 px down to 56 px above the dock.
 */
export const TITLE_BAR = 52;
const OPAQUE_TOP = 96;
const CLEAR_ABOVE_DOCK = 16;
const OPAQUE_ABOVE_DOCK = 56;
/** The dock floats this far above the window's bottom edge. */
const DOCK_GAP = 16;

interface TranscriptFrame {
  readonly scroller: HTMLElement;
  /** Window px per CSS px: the lab may scale the whole window, which bounding boxes include. */
  readonly scale: number;
  /** The readable band, in viewport px: under the title bar, above the dock. */
  readonly top: number;
  readonly bottom: number;
}

function frameOf(element: HTMLElement, clearTop: number, clearBottom: number) {
  const scroller = element.closest<HTMLElement>(".n-scroll");
  if (!scroller || scroller.offsetHeight === 0) return null;
  const box = scroller.getBoundingClientRect();
  const scale = box.height / scroller.offsetHeight || 1;
  const dock = Number.parseFloat(getComputedStyle(scroller).getPropertyValue("--n-dock-h")) || 0;
  const frame: TranscriptFrame = {
    scroller,
    scale,
    top: box.top + clearTop * scale,
    bottom: box.bottom - (dock + DOCK_GAP + clearBottom) * scale,
  };
  return frame;
}

/** How much of `element` (0..1) shows in the transcript, between its two clear bands. */
export function shareInView(element: HTMLElement): number {
  const frame = frameOf(element, TITLE_BAR, CLEAR_ABOVE_DOCK);
  if (!frame) return 1;
  const rect = element.getBoundingClientRect();
  if (rect.height <= 0) return 0;
  const shown = Math.min(rect.bottom, frame.bottom) - Math.max(rect.top, frame.top);
  return Math.max(0, Math.min(1, shown / rect.height));
}

/** Scrolls the transcript so `element` sits in the middle of its fully opaque band. */
export function centreInTranscript(element: HTMLElement, smooth: boolean): void {
  const frame = frameOf(element, OPAQUE_TOP, OPAQUE_ABOVE_DOCK);
  if (!frame) return;
  const rect = element.getBoundingClientRect();
  const delta = (rect.top + rect.bottom) / 2 - (frame.top + frame.bottom) / 2;
  frame.scroller.scrollTo({
    top: frame.scroller.scrollTop + delta / frame.scale,
    behavior: smooth ? "smooth" : "auto",
  });
}

/**
 * Scrolls the transcript just enough to show all of `element` between the title bar and the
 * composer dock, keeping its top in view if it is taller than the space. Measured in the window's
 * own px: the lab may scale the whole window, which bounding boxes include and scrollTop does not.
 */
export function revealInTranscript(element: HTMLElement, smooth: boolean): boolean {
  const frame = frameOf(element, OPAQUE_TOP, OPAQUE_ABOVE_DOCK);
  if (!frame) return false;
  const rect = element.getBoundingClientRect();
  let delta = rect.bottom > frame.bottom ? rect.bottom - frame.bottom : 0;
  if (rect.top - delta < frame.top) delta = rect.top - frame.top;
  if (Math.abs(delta) < 1) return false;
  frame.scroller.scrollTo({
    top: frame.scroller.scrollTop + delta / frame.scale,
    behavior: smooth ? "smooth" : "auto",
  });
  return true;
}

/**
 * Events an opening region sends up to the transcript: `hold` stops it following new output (the
 * reader is looking at something), `recheck` lets it decide again from where it is scrolled.
 */
export const TRANSCRIPT_HOLD = "noir-transcript-hold";
export const TRANSCRIPT_RECHECK = "noir-transcript-recheck";

/** True for the animation that opens an Expand (to its natural height), not the one closing it. */
function opening(definition: unknown): boolean {
  return (
    typeof definition === "object" &&
    definition !== null &&
    "height" in definition &&
    definition.height === "auto"
  );
}

/**
 * A region that opens and closes in place. When the reader opened it (`follow`), it brings
 * itself into view once open, so nothing they asked for unfolds out of sight under the composer.
 */
export function Expand({
  open,
  follow,
  className = "n-step-body",
  children,
}: {
  readonly open: boolean;
  readonly follow: boolean;
  readonly className?: string;
  readonly children: ReactNode;
}) {
  const reduced = usePrefersReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const motionProps = reveal(reduced);
  return (
    <AnimatePresence initial={false}>
      {open ? (
        <motion.div
          key="expand"
          ref={ref}
          className={className}
          initial={motionProps.initial}
          animate={motionProps.animate}
          exit={motionProps.exit}
          transition={motionProps.transition}
          onAnimationStart={(definition) => {
            if (follow && opening(definition)) {
              ref.current?.dispatchEvent(new Event(TRANSCRIPT_HOLD, { bubbles: true }));
            }
          }}
          onAnimationComplete={(definition) => {
            const element = ref.current;
            if (!follow || !element || !opening(definition)) return;
            if (!revealInTranscript(element, !reduced)) {
              element.dispatchEvent(new Event(TRANSCRIPT_RECHECK, { bubbles: true }));
            }
          }}
        >
          {children}
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}
