import { springs, usePrefersReducedMotion } from "@ghost/ui";
import { AnimatePresence, motion } from "motion/react";
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type { Player } from "../../kit";

/** Joins class names, skipping the falsy ones. */
export function cx(...parts: readonly (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

/**
 * False while the window renders its first frame, true afterwards. Anything that mounts once it is
 * true arrived live (a new block, a new tool call) and animates in; what was already there when the
 * window opened (a frozen `?t=` frame, a finished chat) just appears, as history does in a real app.
 */
export const LiveContext = createContext(false);

/** Whether this component mounted after the window did. Fixed for the component's lifetime. */
export function useBornLive(): boolean {
  const live = useContext(LiveContext);
  return useRef(live).current;
}

const NO_SUBSCRIPTION = (): (() => void) => () => {};

/**
 * Whether the playhead has reached `at` (ms on the timeline); false for null. Re-renders only when
 * the answer flips, so a component can wait for a moment on the timeline without re-rendering every
 * frame, and a frozen `?t=` frame always gets the same answer.
 */
export function usePlayheadPast(player: Player, at: number | null): boolean {
  const read = useCallback(() => at !== null && player.getTime() >= at, [player, at]);
  return useSyncExternalStore(at === null ? NO_SUBSCRIPTION : player.subscribe, read, read);
}

/**
 * Height reveal for disclosures: springs open, fades closed; instant under reduced motion. Content
 * is clipped only while the height moves, so shadows, focus rings and hover fills that reach past
 * the edges show once it rests.
 */
export function Collapse({
  open,
  children,
  className,
  id,
  appear = false,
}: {
  readonly open: boolean;
  readonly children: ReactNode;
  readonly className?: string;
  readonly id?: string;
  /** Also reveal when it mounts open (something that arrived live), not only when `open` flips. */
  readonly appear?: boolean;
}) {
  const reduced = usePrefersReducedMotion();
  const [moving, setMoving] = useState(false);
  return (
    <AnimatePresence initial={appear}>
      {open ? (
        <motion.div
          key="collapse"
          id={id}
          className={cx("au-collapse", className)}
          data-moving={moving ? "" : undefined}
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={
            reduced
              ? { duration: 0 }
              : { height: springs.smooth, opacity: { duration: 0.18, ease: "easeOut" } }
          }
          onAnimationStart={() => setMoving(true)}
          onAnimationComplete={() => setMoving(false)}
        >
          {children}
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}

/** A thin rotating arc in the accent colour (static under reduced motion). */
export function Spinner({ size = 14, tone }: { readonly size?: number; readonly tone?: "amber" }) {
  return (
    <span
      className="au-spinner"
      data-tone={tone}
      style={{ width: size, height: size }}
      aria-hidden="true"
    />
  );
}

export function Kbd({ children }: { readonly children: ReactNode }) {
  return <kbd className="au-kbd">{children}</kbd>;
}

/** "+19 −5", green and red. A side with nothing to count is left out ("+38" for a new file). */
export function DiffStat({ added, removed }: { readonly added: number; readonly removed: number }) {
  const both = added === 0 && removed === 0;
  return (
    <span className="au-diffstat">
      {added > 0 || both ? <span className="au-add">+{added}</span> : null}
      {removed > 0 ? <span className="au-del">−{removed}</span> : null}
    </span>
  );
}

export function IconButton({
  label,
  children,
  onClick,
  tip = "bottom",
  className,
  pressed,
  expanded,
}: {
  readonly label: string;
  readonly children: ReactNode;
  readonly onClick?: () => void;
  /** Where the tooltip shows. */
  readonly tip?: "bottom" | "top" | "right" | "left";
  readonly className?: string;
  readonly pressed?: boolean;
  readonly expanded?: boolean;
}) {
  return (
    <button
      type="button"
      className={cx("au-icon-btn", className)}
      aria-label={label}
      aria-pressed={pressed}
      aria-expanded={expanded}
      data-tip={label}
      data-tip-side={tip}
      onClick={onClick}
    >
      {children}
    </button>
  );
}
