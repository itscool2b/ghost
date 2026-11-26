import { type ReactNode, useEffect, useRef } from "react";
import { useInView, usePageVisible, useReducedMotion } from "../motion";

/**
 * The field a stage stands on: rows of contour lines over low hills, far rows close together and
 * faint, near rows apart and brighter. One function draws it twice: as an SVG when the page is
 * built, so it is whole without scripts and holds still where motion is not welcome, and on a
 * canvas that lets the hills drift once the page can move. Monochrome, and no light is painted:
 * the lines are the page's ink at a low strength.
 */

const ROWS = 34;
const TAU = Math.PI * 2;

/** A row's place down the stage, 0 at the far edge and 1 at the near one. */
const depth = (row: number) => row / (ROWS - 1);

/** How high the ground stands at `u` across the stage (0 to 1) on a row, at time `t` in seconds. */
function height(u: number, d: number, t: number): number {
  const broad =
    Math.sin(TAU * (u * 1.1 + d * 0.9 + t * 0.011)) *
    Math.sin(TAU * (u * 0.37 - d * 0.5 + 0.21 + t * 0.007));
  const mid = Math.sin(TAU * (u * 2.3 - d * 1.7 + t * 0.017) + 1.7);
  const fine = Math.sin(TAU * (u * 5.1 + d * 3.1 - t * 0.023));
  const h = 0.6 * broad + 0.28 * mid + 0.12 * fine;
  // Hills rise more than valleys fall, so the rows read as ground and not as waves.
  return h > 0 ? h ** 1.35 : h * 0.4;
}

/** A row's line as points across a stage `w` wide and `h` tall. */
function rowY(row: number, u: number, h: number, t: number): number {
  const d = depth(row);
  const base = h * (0.05 + 0.99 * d ** 1.35);
  const lift = h * (0.035 + 0.13 * d);
  return base - lift * height(u, d, t);
}

/** How strongly a row is drawn: faint far away, firmer close. */
const strength = (row: number) => 0.11 + 0.2 * depth(row) ** 1.2;

const VIEW_W = 1200;
const VIEW_H = 640;
const STEPS = 60;

/** The still field, drawn once when the page is built. */
const STILL: readonly { d: string; opacity: number }[] = Array.from({ length: ROWS }, (_, row) => {
  let d = "";
  for (let i = 0; i <= STEPS; i++) {
    const u = i / STEPS;
    d += `${i === 0 ? "M" : "L"}${Math.round(u * VIEW_W)} ${rowY(row, u, VIEW_H, 0).toFixed(1)}`;
  }
  return { d, opacity: Number(strength(row).toFixed(3)) };
});

/**
 * A stage: the field with grain over it and whatever stands on it. The field is decoration, hidden
 * from assistive technology. `quiet` draws it fainter, for the stages further down the page.
 */
export function Stage({
  children,
  className = "",
  quiet = false,
}: {
  children?: ReactNode;
  className?: string;
  quiet?: boolean;
}) {
  const stage = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const reduced = useReducedMotion();
  const visible = usePageVisible();
  const inView = useInView(stage, { rootMargin: "120px" });
  const live = !reduced && visible && inView;

  useEffect(() => {
    const node = canvas.current;
    const holder = stage.current;
    const context = node?.getContext("2d");
    if (!node || !holder || !context) return;
    if (!live) {
      // Reduced motion returns to the still drawing; offscreen, the last frame simply waits.
      if (reduced) holder.removeAttribute("data-live");
      return;
    }
    let frame = 0;
    let width = 0;
    let tall = 0;
    const fit = () => {
      const box = node.getBoundingClientRect();
      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      width = Math.max(1, Math.round(box.width));
      tall = Math.max(1, Math.round(box.height));
      node.width = Math.round(width * ratio);
      node.height = Math.round(tall * ratio);
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
    };
    fit();
    const sizes = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(fit);
    sizes?.observe(node);
    const started = performance.now();
    const draw = (now: number) => {
      const t = (now - started) / 1000;
      context.clearRect(0, 0, width, tall);
      context.lineWidth = 1;
      context.lineJoin = "round";
      const step = Math.max(6, Math.round(width / 160));
      for (let row = 0; row < ROWS; row++) {
        context.strokeStyle = `rgba(242, 242, 242, ${strength(row)})`;
        context.beginPath();
        for (let x = 0; x <= width + step; x += step) {
          const y = rowY(row, x / width, tall, t);
          if (x === 0) context.moveTo(x, y);
          else context.lineTo(x, y);
        }
        context.stroke();
      }
      holder.setAttribute("data-live", "");
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => {
      cancelAnimationFrame(frame);
      sizes?.disconnect();
    };
  }, [live, reduced]);

  return (
    <div
      ref={stage}
      className={`stage${quiet ? " stage-quiet" : ""}${className ? ` ${className}` : ""}`}
    >
      <svg
        className="stage-field"
        viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
        preserveAspectRatio="none"
        aria-hidden="true"
        focusable="false"
      >
        {STILL.map((line, row) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: a fixed drawing; the row is identity.
          <path key={row} d={line.d} strokeOpacity={line.opacity} />
        ))}
      </svg>
      {/* biome-ignore lint/a11y/noAriaHiddenOnFocusable: a canvas with no tabindex takes no focus, and the field is decoration. */}
      <canvas ref={canvas} className="stage-canvas" aria-hidden="true" />
      <div className="stage-grain" aria-hidden="true" />
      {children}
    </div>
  );
}
