import { springs, squirclePath, usePrefersReducedMotion } from "@ghost/ui";
import type { LucideIcon } from "lucide-react";
import {
  AnimatePresence,
  type MotionValue,
  motion,
  useMotionValue,
  useSpring,
  useTransform,
} from "motion/react";
import {
  createContext,
  type PointerEvent,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

/** Joins class names, skipping the falsy ones. */
export function cx(...parts: readonly (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ");
}

/*
 * Halo's radii, all squircles with the shared smoothing. Floating things round more than resting
 * ones, and nested shapes step down so corners stay concentric.
 */
export const RADIUS = {
  slab: 24,
  card: 26,
  bubble: 22,
  popover: 26,
  capsule: 32,
  well: 16,
  row: 14,
  chip: 10,
} as const;

/**
 * What a glass surface is made of. Each material is a fill, a bevel (inner light), and a rim of
 * light along the edge; the host element casts the shadow, since a shadow inside the clip would be
 * cut off. Planes nearer the eye are brighter and have a crisper top edge: `slab` (the sidebar,
 * resting), `card` (turns), `near` (the status slab tucked under the composer), `capsule` (the
 * composer), `popover` (menus, nearest).
 */
export type Material =
  | "slab"
  | "card"
  | "near"
  | "bubble"
  | "raised"
  | "well"
  | "capsule"
  | "popover"
  | "alert";

/**
 * The glass behind a Halo element: drop it as the first child of a positioned element with
 * `isolation: isolate`. It fills the host, sits behind its content, and is clipped to a squircle
 * that follows the host's size (WebKit has no `corner-shape`). The rim is an SVG stroke on the same
 * path, drawn inside the clip so the clip can never shave it off: bright where light lands on the
 * top-left edge, dim along the sides, and catching a little again at the bottom-right, the way a
 * thick pane of glass reads.
 */
export function Glass({
  material,
  radius,
  className,
}: {
  readonly material: Material;
  readonly radius: number;
  readonly className?: string;
}) {
  const surfaceRef = useRef<HTMLSpanElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const pathRef = useRef<SVGPathElement>(null);
  const gradientId = `halo-rim-${useId().replace(/[^\w-]/g, "")}`;

  // Written straight to the DOM on every resize, so a growing card costs no React render.
  useLayoutEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) return;
    let width = -1;
    let height = -1;
    const apply = (nextWidth: number, nextHeight: number): void => {
      if (Math.abs(nextWidth - width) < 0.25 && Math.abs(nextHeight - height) < 0.25) return;
      width = nextWidth;
      height = nextHeight;
      const path = squirclePath(width, height, radius);
      const clip = `path("${path}")`;
      surface.style.setProperty("-webkit-clip-path", clip);
      surface.style.setProperty("clip-path", clip);
      svgRef.current?.setAttribute("viewBox", `0 0 ${width} ${height}`);
      pathRef.current?.setAttribute("d", path);
    };
    apply(surface.offsetWidth, surface.offsetHeight);
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver((entries) => {
      const box = entries[entries.length - 1]?.borderBoxSize?.[0];
      if (box) apply(box.inlineSize, box.blockSize);
      else apply(surface.offsetWidth, surface.offsetHeight);
    });
    observer.observe(surface, { box: "border-box" });
    return () => observer.disconnect();
  }, [radius]);

  return (
    <span
      ref={surfaceRef}
      className={cx("halo-surface", className)}
      data-material={material}
      aria-hidden="true"
    >
      <svg
        ref={svgRef}
        className="halo-rim"
        preserveAspectRatio="none"
        focusable="false"
        aria-hidden="true"
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" className="halo-rim-a" />
            <stop offset="0.3" className="halo-rim-b" />
            <stop offset="0.7" className="halo-rim-c" />
            <stop offset="1" className="halo-rim-d" />
          </linearGradient>
        </defs>
        {/* 2px wide on the clip path: the outer half is clipped away, leaving a crisp 1px rim. */}
        <path ref={pathRef} fill="none" stroke={`url(#${gradientId})`} strokeWidth={2} />
      </svg>
    </span>
  );
}

/**
 * Opens and closes its content with a spring on height, fading it in step. Instant under reduced
 * motion. Content present on the first render does not animate.
 */
export function Collapse({
  open,
  children,
  className,
}: {
  readonly open: boolean;
  readonly children: ReactNode;
  readonly className?: string;
}) {
  const reduced = usePrefersReducedMotion();
  return (
    <AnimatePresence initial={false}>
      {open ? (
        <motion.div
          key="collapse"
          className={cx("halo-collapse", className)}
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={
            reduced
              ? { duration: 0 }
              : { height: springs.smooth, opacity: { duration: 0.22, ease: [0.2, 0, 0, 1] } }
          }
        >
          {children}
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}

/*
 * Entrances: only what arrives after the window first renders animates in. Opening a chat (or a
 * `?t=` frame) shows it at rest; a block streamed in a moment later fades and rises.
 */
const MountedContext = createContext<{ readonly current: boolean }>({ current: true });

export function EntranceProvider({ children }: { readonly children: ReactNode }) {
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
  }, []);
  return <MountedContext.Provider value={mounted}>{children}</MountedContext.Provider>;
}

/** True when the calling component mounted after the window's first render: it should animate in. */
export function useEntrance(): boolean {
  const mounted = useContext(MountedContext);
  const [enter] = useState(() => mounted.current);
  return enter;
}

/*
 * Parallax: the pointer's position over the window, springing toward it. Planes move by their
 * depth (px at the window's edge): far planes against the pointer, near ones with it. Offsets snap
 * to half pixels (whole device pixels at 2x), so a plane at rest never sits between pixels.
 */
interface ParallaxValues {
  readonly x: MotionValue<number>;
  readonly y: MotionValue<number>;
}

/*
 * The light under your pointer: glass marked `data-halo-light` catches a soft highlight where the
 * pointer is, the way visionOS lights what you look at. Only the element under the pointer updates
 * (two custom properties), and its highlight fades out when the pointer leaves.
 */
function followLight(event: PointerEvent<HTMLElement>): void {
  const target = event.target;
  if (!(target instanceof Element)) return;
  const lit = target.closest<HTMLElement>("[data-halo-light]");
  if (!lit) return;
  const rect = lit.getBoundingClientRect();
  // The window may be scaled to fit the screen; the highlight is placed in the element's own px.
  const scale = lit.offsetWidth > 0 ? rect.width / lit.offsetWidth : 1;
  lit.style.setProperty("--halo-lx", `${Math.round((event.clientX - rect.left) / scale)}px`);
  lit.style.setProperty("--halo-ly", `${Math.round((event.clientY - rect.top) / scale)}px`);
}

const ParallaxContext = createContext<ParallaxValues | null>(null);

export function useParallaxSource(enabled: boolean) {
  const rawX = useMotionValue(0);
  const rawY = useMotionValue(0);
  const x = useSpring(rawX, { stiffness: 90, damping: 18, mass: 0.8 });
  const y = useSpring(rawY, { stiffness: 90, damping: 18, mass: 0.8 });

  useEffect(() => {
    if (enabled) return;
    rawX.jump(0);
    rawY.jump(0);
    x.jump(0);
    y.jump(0);
  }, [enabled, rawX, rawY, x, y]);

  const onPointerMove = useCallback(
    (event: PointerEvent<HTMLElement>) => {
      if (event.pointerType === "touch") return;
      followLight(event);
      if (!enabled) return;
      // Planes hold still while you work in the dock or a menu is open, so nothing you aim at moves.
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest("[data-parallax-hold]")) return;
      if (event.currentTarget.querySelector(".halo-popover")) return;
      const rect = event.currentTarget.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return;
      rawX.set(((event.clientX - rect.left) / rect.width) * 2 - 1);
      rawY.set(((event.clientY - rect.top) / rect.height) * 2 - 1);
    },
    [enabled, rawX, rawY],
  );
  const onPointerLeave = useCallback(() => {
    rawX.set(0);
    rawY.set(0);
  }, [rawX, rawY]);

  const values = useMemo<ParallaxValues>(() => ({ x, y }), [x, y]);
  return { values, onPointerMove, onPointerLeave };
}

export function ParallaxProvider({
  value,
  children,
}: {
  readonly value: ParallaxValues;
  readonly children: ReactNode;
}) {
  return <ParallaxContext.Provider value={value}>{children}</ParallaxContext.Provider>;
}

const snap = (value: number): number => Math.round(value * 2) / 2;

/** Motion values for a plane at `depth`: spread them into a motion element's `style`. */
export function useDepth(depth: number) {
  const parallax = useContext(ParallaxContext);
  const still = useMotionValue(0);
  const x = useTransform(parallax?.x ?? still, (value) => snap(value * depth));
  const y = useTransform(parallax?.y ?? still, (value) => snap(value * depth * 0.6));
  return { x, y };
}

/** A round icon button with an accessible name and a tooltip. */
export function IconButton({
  icon: Icon,
  label,
  size = 16,
  className,
  onClick,
  pressed,
}: {
  readonly icon: LucideIcon;
  readonly label: string;
  readonly size?: number;
  readonly className?: string;
  readonly onClick?: () => void;
  readonly pressed?: boolean;
}) {
  return (
    <button
      type="button"
      className={cx("halo-icon-btn", className)}
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      onClick={onClick}
    >
      <Icon size={size} aria-hidden="true" />
    </button>
  );
}

/** A keycap hint. */
export function Kbd({ children }: { readonly children: ReactNode }) {
  return <kbd className="halo-kbd">{children}</kbd>;
}

/** Elements that take focus, in tab order, inside `root`. */
export function focusables(root: HTMLElement): HTMLElement[] {
  return [
    ...root.querySelectorAll<HTMLElement>(
      "button:not(:disabled), [href], input:not(:disabled), textarea:not(:disabled), [tabindex]",
    ),
  ].filter((element) => element.tabIndex >= 0 && element.getClientRects().length > 0);
}

/** Keeps Tab and Shift+Tab cycling inside `root`, for menus that hold focus while open. */
export function trapTab(
  event: { key: string; shiftKey: boolean; preventDefault(): void },
  root: HTMLElement,
): void {
  if (event.key !== "Tab") return;
  const items = focusables(root);
  const first = items[0];
  const last = items[items.length - 1];
  if (!first || !last) return;
  const active = document.activeElement;
  if (event.shiftKey && (active === first || !root.contains(active))) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && (active === last || !root.contains(active))) {
    event.preventDefault();
    first.focus();
  }
}

/** "+19 −5": lines added and removed, each left out when zero. */
export function LineCounts({
  added,
  removed,
  className,
}: {
  readonly added: number;
  readonly removed: number;
  readonly className?: string;
}) {
  if (added === 0 && removed === 0) return null;
  return (
    <span className={cx("halo-counts", className)}>
      {added > 0 ? <span className="halo-add">+{added}</span> : null}
      {removed > 0 ? <span className="halo-del">−{removed}</span> : null}
    </span>
  );
}

/** A small activity spinner: an arc of the accent turning around a faint track. */
export function Spinner({ className }: { readonly className?: string }) {
  return <span className={cx("halo-spinner", className)} aria-hidden="true" />;
}
