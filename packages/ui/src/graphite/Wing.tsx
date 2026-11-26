import { type RefObject, useEffect, useId, useRef } from "react";
import { LOGO_EYE_PATHS, LOGO_OUTLINE_PATH, LOGO_SMALL_EYE_PATHS, LOGO_VIEWBOX } from "./logoPaths";

const REDUCED = "(prefers-reduced-motion: reduce)";

/**
 * Points the wing's eyes at the pointer: sets `--look-x` and `--look-y` (each from -1 to 1) on the
 * element, eased so the eyes follow instead of snapping. Does nothing for touch, for
 * prefers-reduced-motion, or while the element is offscreen.
 */
function useLook(ref: RefObject<SVGSVGElement | null>, enabled: boolean) {
  useEffect(() => {
    const node = ref.current;
    if (!enabled || !node || matchMedia(REDUCED).matches || !matchMedia("(hover: hover)").matches)
      return;
    let frame = 0;
    let visible = true;
    const target = { x: 0, y: 0 };
    const now = { x: 0, y: 0 };
    const step = () => {
      frame = 0;
      now.x += (target.x - now.x) * 0.14;
      now.y += (target.y - now.y) * 0.14;
      node.style.setProperty("--look-x", now.x.toFixed(3));
      node.style.setProperty("--look-y", now.y.toFixed(3));
      if (Math.abs(target.x - now.x) + Math.abs(target.y - now.y) > 0.004)
        frame = requestAnimationFrame(step);
    };
    const onMove = (event: PointerEvent) => {
      if (!visible || event.pointerType === "touch") return;
      const box = node.getBoundingClientRect();
      // A full look at about a third of the window away; further than that looks the same.
      const reach = Math.max(240, innerWidth / 3);
      const clamp = (value: number) => Math.max(-1, Math.min(1, value));
      target.x = clamp((event.clientX - (box.left + box.width / 2)) / reach);
      target.y = clamp((event.clientY - (box.top + box.height / 2)) / reach);
      if (!frame) frame = requestAnimationFrame(step);
    };
    const observer = new IntersectionObserver((entries) => {
      visible = entries[entries.length - 1]?.isIntersecting ?? true;
    });
    observer.observe(node);
    addEventListener("pointermove", onMove, { passive: true });
    return () => {
      removeEventListener("pointermove", onMove);
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [ref, enabled]);
}

export interface WingProps {
  readonly className?: string;
  /** Drawn in one hairline instead of filled: the website's closing picture. */
  readonly line?: boolean;
  /** The eyes follow the pointer. */
  readonly look?: boolean;
  /**
   * The small logo's larger eyes, which stay open at icon sizes: for a wing that stands where
   * the small logo was a moment ago (the desktop's tour lifts off from the sidebar's logo).
   */
  readonly small?: boolean;
}

/**
 * The Ghost logo as something alive: the same wing, whose eyes blink now and then and can look
 * toward the pointer, or wherever its owner sets `--look-x` and `--look-y` (each from -1 to 1).
 * Filled, the eyes are holes cut by a mask, so they show whatever is behind the wing; in line,
 * the outline and the eyes are stroked. Always decorative: whatever shows it names the app in
 * words. The component draws and looks; each app's own stylesheet says how it moves and where
 * motion is welcome (the website's styles/home.css, the desktop's tour.css), under the class
 * names `wing`, `wing-body`, `wing-look` and `wing-eye`.
 */
export function Wing({ className, line = false, look = false, small = false }: WingProps) {
  const ref = useRef<SVGSVGElement>(null);
  const mask = useId();
  useLook(ref, look);
  const [width, height] = LOGO_VIEWBOX;
  const eyes = (
    <g className="wing-look">
      {(small ? LOGO_SMALL_EYE_PATHS : LOGO_EYE_PATHS).map((d) => (
        <path key={d} className="wing-eye" d={d} pathLength={line ? 1 : undefined} />
      ))}
    </g>
  );
  return (
    <svg
      ref={ref}
      className={["wing", line ? "wing-line" : "", className ?? ""].filter(Boolean).join(" ")}
      viewBox={`0 0 ${width} ${height}`}
      aria-hidden="true"
      focusable="false"
    >
      {line ? (
        <>
          <path className="wing-body" d={LOGO_OUTLINE_PATH} pathLength={1} />
          {eyes}
        </>
      ) : (
        <>
          <mask id={mask} maskUnits="userSpaceOnUse" x="0" y="0" width={width} height={height}>
            <rect width={width} height={height} fill="#fff" />
            <g fill="#000">{eyes}</g>
          </mask>
          <path className="wing-body" d={LOGO_OUTLINE_PATH} mask={`url(#${mask})`} />
        </>
      )}
    </svg>
  );
}
