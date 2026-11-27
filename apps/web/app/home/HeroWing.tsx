import { LOGO_EYE_PATHS, LOGO_OUTLINE_PATH, LOGO_VIEWBOX } from "@ghost/ui/graphite";
import { type CSSProperties, type RefObject, useEffect, useId, useRef } from "react";

/** The slab's depth: this many copies of the wing, 2px apart, the face at the front. */
const LAYERS = 12;

const REDUCED = "(prefers-reduced-motion: reduce)";

/**
 * Tilts the wing toward the pointer: sets `--tilt-x` and `--tilt-y` (each from -1 to 1) on the
 * element, eased so it turns instead of snapping. Nothing for touch, for reduced motion, or while
 * the wing is offscreen.
 */
function useTilt(ref: RefObject<HTMLDivElement | null>) {
  useEffect(() => {
    const node = ref.current;
    if (!node || matchMedia(REDUCED).matches || !matchMedia("(hover: hover)").matches) return;
    let frame = 0;
    let visible = true;
    const target = { x: 0, y: 0 };
    const now = { x: 0, y: 0 };
    const step = () => {
      frame = 0;
      now.x += (target.x - now.x) * 0.08;
      now.y += (target.y - now.y) * 0.08;
      node.style.setProperty("--tilt-x", now.x.toFixed(3));
      node.style.setProperty("--tilt-y", now.y.toFixed(3));
      if (Math.abs(target.x - now.x) + Math.abs(target.y - now.y) > 0.002)
        frame = requestAnimationFrame(step);
    };
    const onMove = (event: PointerEvent) => {
      if (!visible || event.pointerType === "touch") return;
      const box = node.getBoundingClientRect();
      // A full turn at about half the window away; further than that turns no more.
      const reach = Math.max(320, innerWidth / 2);
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
  }, [ref]);
}

/**
 * The opening's object, as X Money opens on its card: the Ghost wing as a solid slab in three
 * dimensions, drawn in CSS alone. Twelve copies of the wing stand 2px apart in depth, the face in
 * the page's ink and the ones behind it a step darker each, so its edge reads as a bevel; its eyes
 * are holes through all of them, and blink. It flies in from far off and pitched back, banks into
 * place, then rides the air and turns a little toward the pointer. Without scripts it still flies
 * in and rides; where motion is not welcome it simply stands at rest, still a slab. Decorative: the
 * heading over it names the page.
 */
export function HeroWing() {
  const root = useRef<HTMLDivElement>(null);
  const id = useId().replace(/[^a-zA-Z0-9-]/g, "");
  const mask = `hw3-eyes-${id}`;
  const face = `hw3-face-${id}`;
  const [width, height] = LOGO_VIEWBOX;
  useTilt(root);
  return (
    <div className="hw3" ref={root} aria-hidden="true">
      <svg className="sprite" width="0" height="0" aria-hidden="true" focusable="false">
        <defs>
          {/* The eyes, cut through every layer at once, so they blink as one. */}
          <mask id={mask} maskUnits="userSpaceOnUse" x="0" y="0" width={width} height={height}>
            <rect width={width} height={height} fill="#fff" />
            <g fill="#000">
              {LOGO_EYE_PATHS.map((d) => (
                <path key={d} className="hw3-eye" d={d} />
              ))}
            </g>
          </mask>
          {/* The face catches more of the light toward its nose. */}
          <linearGradient id={face} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor="#ffffff" />
            <stop offset="1" stopColor="#d6d6d6" />
          </linearGradient>
        </defs>
      </svg>
      <div className="hw3-air">
        <div className="hw3-idle">
          <div className="hw3-tilt">
            <div className="hw3-body">
              {Array.from({ length: LAYERS }, (_, index) => LAYERS - 1 - index).map((depth) => (
                <svg
                  key={depth}
                  className="hw3-layer"
                  viewBox={`0 0 ${width} ${height}`}
                  aria-hidden="true"
                  focusable="false"
                  data-face={depth === 0 ? "" : undefined}
                  style={{ "--z": depth } as CSSProperties}
                >
                  <path
                    d={LOGO_OUTLINE_PATH}
                    mask={`url(#${mask})`}
                    fill={depth === 0 ? `url(#${face})` : undefined}
                  />
                </svg>
              ))}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
