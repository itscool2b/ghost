/**
 * How the tour's wing flies: where it settles beside a control, and the curve it takes between
 * two places. Plain geometry, so it is tested without a window.
 *
 * The logo is a flying wing seen from above with its nose at the top, so an angle of 0 is nose
 * up and angles grow clockwise. In flight the nose follows the curve; settled, it points at the
 * control the stop is about.
 */
import type { TourSide } from "./stops";

export interface Point {
  readonly x: number;
  readonly y: number;
}

/** Where the wing is: its centre, its heading in degrees, its roll (0 to 1) and its width. */
export interface Pose extends Point {
  readonly angle: number;
  readonly bank: number;
  readonly size: number;
}

export interface Box {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

/** The wing's width beside a stop, and how its height follows from the logo's shape. */
export const WING_SIZE = 44;
export const WING_RATIO = 440 / 1044;
/** How long one flight takes, in ms (`--g-dur-flight`). */
export const FLIGHT_MS = 640;
/** Space between a control and the wing, and between the wing and the card. */
const GAP = 8;
/** How far the window's edge keeps the card and the wing away. */
const EDGE = 12;

const clamp = (value: number, low: number, high: number) =>
  Math.max(low, Math.min(Math.max(low, high), value));

/** The glide curve, `cubic-bezier(0.32, 0.72, 0, 1)`: fast away, long settle. */
export function glide(t: number): number {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  const curve = (a: number, b: number, u: number) =>
    3 * a * u * (1 - u) ** 2 + 3 * b * u * u * (1 - u) + u ** 3;
  // Solve x(u) = t by bisection; x is monotonic for these control points.
  let low = 0;
  let high = 1;
  for (let step = 0; step < 24; step += 1) {
    const middle = (low + high) / 2;
    if (curve(0.32, 0, middle) < t) low = middle;
    else high = middle;
  }
  return curve(0.72, 1, (low + high) / 2);
}

/** From heading `from` to `to` by the shorter way round, `k` of the way. */
export function turn(from: number, to: number, k: number): number {
  const delta = ((((to - from) % 360) + 540) % 360) - 180;
  return from + delta * k;
}

/** The heading, in degrees, of travel along (dx, dy) on a screen (y grows downward). */
export function heading(dx: number, dy: number): number {
  return (Math.atan2(dx, -dy) * 180) / Math.PI;
}

const OPPOSITE: Readonly<Record<TourSide, TourSide>> = {
  right: "left",
  left: "right",
  above: "below",
  below: "above",
};

export interface Placement {
  /** The side used: the one asked for, or its opposite when the card would not fit there. */
  readonly side: TourSide;
  readonly wing: Pose;
  /** The card's top left corner. */
  readonly card: Point;
}

/**
 * Where the wing and the card go for a control at `target`: the wing just off the control's
 * `side`, nose toward it, and the card beyond the wing, both kept inside the window.
 */
export function place(
  target: Box,
  side: TourSide,
  card: { readonly width: number; readonly height: number },
  window: { readonly width: number; readonly height: number },
): Placement {
  const depth = WING_SIZE * WING_RATIO;
  const reach = GAP + depth + GAP;
  const fits = (candidate: TourSide) => {
    if (candidate === "right")
      return target.left + target.width + reach + card.width + EDGE <= window.width;
    if (candidate === "left") return target.left - reach - card.width - EDGE >= 0;
    if (candidate === "below")
      return target.top + target.height + reach + card.height + EDGE <= window.height;
    return target.top - reach - card.height - EDGE >= 0;
  };
  const used = fits(side) || !fits(OPPOSITE[side]) ? side : OPPOSITE[side];
  const centre = { x: target.left + target.width / 2, y: target.top + target.height / 2 };
  const across = used === "right" || used === "left";
  // The wing rides the control's middle, kept where the card can still stand beside it.
  const along = across
    ? clamp(centre.y, EDGE + depth, window.height - EDGE - depth)
    : clamp(centre.x, EDGE + depth, window.width - EDGE - depth);
  const out = GAP + depth / 2;
  const wing: Pose = {
    x:
      used === "right"
        ? target.left + target.width + out
        : used === "left"
          ? target.left - out
          : along,
    y:
      used === "below"
        ? target.top + target.height + out
        : used === "above"
          ? target.top - out
          : along,
    angle: used === "right" ? -90 : used === "left" ? 90 : used === "below" ? 0 : 180,
    bank: 0,
    size: WING_SIZE,
  };
  const cardAt: Point = across
    ? {
        x: used === "right" ? target.left + target.width + reach : target.left - reach - card.width,
        y: clamp(along - 28, EDGE, window.height - card.height - EDGE),
      }
    : {
        x: clamp(along - card.width / 2, EDGE, window.width - card.width - EDGE),
        y: used === "below" ? target.top + target.height + reach : target.top - reach - card.height,
      };
  return { side: used, wing, card: cardAt };
}

/**
 * The wing `t` of the way (0 to 1, already eased) from `from` to `to`. It follows a shallow arc,
 * turns its nose along it, rolls into the turn through the middle, and takes up its resting
 * heading and width as it arrives. Two places a few pixels apart are joined by a straight slide.
 */
export function flightPose(from: Pose, to: Pose, t: number): Pose {
  const k = clamp(t, 0, 1);
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const distance = Math.hypot(dx, dy);
  const size = from.size + (to.size - from.size) * k;
  if (distance < 24)
    return {
      x: from.x + dx * k,
      y: from.y + dy * k,
      angle: turn(from.angle, to.angle, k),
      bank: 0,
      size,
    };
  // The arc bows to the left of travel, a fifth of the way out at most.
  const bow = Math.min(distance * 0.2, 96);
  const control = {
    x: from.x + dx / 2 + (dy / distance) * bow,
    y: from.y + dy / 2 - (dx / distance) * bow,
  };
  const x = (1 - k) ** 2 * from.x + 2 * (1 - k) * k * control.x + k * k * to.x;
  const y = (1 - k) ** 2 * from.y + 2 * (1 - k) * k * control.y + k * k * to.y;
  const tx = 2 * (1 - k) * (control.x - from.x) + 2 * k * (to.x - control.x);
  const ty = 2 * (1 - k) * (control.y - from.y) + 2 * k * (to.y - control.y);
  const along = heading(tx, ty);
  // Nose along the curve, turning out of the old heading at first and into the new one at last.
  const leaving = clamp(k / 0.2, 0, 1);
  const arriving = clamp((k - 0.62) / 0.38, 0, 1);
  const angle = turn(turn(from.angle, along, leaving), to.angle, arriving * arriving);
  return { x, y, angle, bank: Math.sin(Math.PI * k) * 0.22, size };
}

/** The CSS transform that draws a pose, for an element whose box is `size` wide. */
export function poseTransform(pose: Pose): string {
  const depth = pose.size * WING_RATIO;
  return `translate3d(${(pose.x - pose.size / 2).toFixed(2)}px, ${(pose.y - depth / 2).toFixed(2)}px, 0) rotate(${pose.angle.toFixed(2)}deg) scaleX(${(1 - pose.bank).toFixed(3)})`;
}
