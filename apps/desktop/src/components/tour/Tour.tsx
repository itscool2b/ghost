import { usePrefersReducedMotion } from "@o7/ui";
import { Wing } from "@o7/ui/graphite";
import {
  type CSSProperties,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { OVER_BROWSER } from "../../lib/browserOcclusion";
import {
  type Box,
  FLIGHT_MS,
  flightPose,
  glide,
  type Point,
  type Pose,
  place,
  poseTransform,
  WING_RATIO,
} from "../../lib/tour/flight";
import { offerWords, type TourOffer, type TourStop } from "../../lib/tour/stops";
import "./tour.css";

/** What the tour is doing: offering a flight from the sidebar's foot, or flying one. */
export type TourState =
  | {
      readonly phase: "offer";
      readonly offer: TourOffer;
      /** Setup just ended: the wing flies in to its place before it asks. */
      readonly arrive?: boolean;
    }
  | { readonly phase: "flight"; readonly stops: readonly TourStop[] };

/** How long a stop's control may take to show (a pane opening, a chat loading) before it is passed. */
const SEEK_MS = 900;
/** How often a settled stop checks that its control is still where the ring is. */
const WATCH_MS = 400;
/** The wing's width at rest, when the sidebar's own logo cannot be measured (the rail). */
const HOME_SIZE = 30;

/** The sidebar's own logo: where the wing lives between flights. */
const HOME = ".chat-sidebar-footer-logo";

function homePose(): Pose {
  const box = document.querySelector(HOME)?.getBoundingClientRect();
  if (box && box.width > 0)
    return {
      x: box.left + box.width / 2,
      y: box.top + box.height / 2,
      angle: 0,
      bank: 0,
      size: box.width,
    };
  return { x: 12 + HOME_SIZE / 2, y: innerHeight - 30, angle: 0, bank: 0, size: HOME_SIZE };
}

/**
 * The stop's control: the first element carrying its name that is actually on screen. A name set
 * on the glyph inside a menu's button (the bell, More) stands for the whole button.
 */
function findTarget(id: string): HTMLElement | null {
  for (const node of document.querySelectorAll<HTMLElement>(`[data-tour="${id}"]`)) {
    const control = node.closest<HTMLElement>("summary") ?? node;
    const box = control.getBoundingClientRect();
    if (box.width > 0 && box.height > 0) return control;
  }
  return null;
}

interface Shown {
  readonly stop: string;
  readonly card: Point;
  readonly ring: Box & { readonly radius: number };
}

function ringOf(node: HTMLElement): Shown["ring"] {
  const box = node.getBoundingClientRect();
  const corner = Number.parseFloat(getComputedStyle(node).borderTopLeftRadius) || 0;
  const pad = 4;
  return {
    left: box.left - pad,
    top: box.top - pad,
    width: box.width + pad * 2,
    height: box.height + pad * 2,
    radius: Math.min(corner, box.height / 2) + pad,
  };
}

/**
 * The tour: o7's wing lifts off from the logo at the sidebar's foot, flies to one control after
 * another and settles beside each, nose toward it, while a card says what it is. Before a flight
 * it sits on the logo and asks. Nothing dims and nothing is covered but the card's own box: the
 * window stays live under it, and the tour ends at Escape, at its last stop, or when it is told
 * to stop.
 *
 * `onPrepare` brings a stop's place into view (New chat, an open chat) before the wing looks
 * for its control; a control that never shows is passed. `onEnd` gets how the flight ended and
 * the stops it reached or passed.
 */
export function Tour({
  state,
  onTake,
  onDecline,
  onPrepare,
  onEnd,
}: {
  readonly state: TourState;
  readonly onTake: () => void;
  readonly onDecline: () => void;
  readonly onPrepare: (stop: TourStop) => void;
  readonly onEnd: (outcome: "finished" | "stopped", visited: readonly string[]) => void;
}) {
  const reduced = usePrefersReducedMotion();
  const wing = useRef<HTMLDivElement>(null);
  const card = useRef<HTMLElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const pose = useRef<Pose | null>(null);
  const frame = useRef(0);
  const titleId = useId();
  const bodyId = useId();
  const [index, setIndex] = useState(0);
  const [shown, setShown] = useState<Shown | null>(null);
  const [settled, setSettled] = useState(false);
  const [asking, setAsking] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const direction = useRef<1 | -1>(1);
  const visited = useRef<string[]>([]);
  // The latest callbacks, so a parent's re-render never restarts a flight.
  const calls = useRef({ onPrepare, onEnd });
  calls.current = { onPrepare, onEnd };
  const reducedNow = useRef(reduced);
  reducedNow.current = reduced;

  const draw = useCallback((next: Pose) => {
    pose.current = next;
    const node = wing.current;
    if (!node) return;
    node.style.width = `${next.size.toFixed(2)}px`;
    node.style.transform = poseTransform(next);
  }, []);
  const fly = useCallback(
    (to: Pose, ms: number, done?: () => void) => {
      cancelAnimationFrame(frame.current);
      const from = pose.current ?? homePose();
      if (ms <= 0 || reducedNow.current) {
        draw(to);
        done?.();
        return;
      }
      const started = performance.now();
      const step = (now: number) => {
        const t = Math.min(1, (now - started) / ms);
        draw(flightPose(from, to, glide(t)));
        if (t < 1) frame.current = requestAnimationFrame(step);
        else done?.();
      };
      frame.current = requestAnimationFrame(step);
    },
    [draw],
  );
  useEffect(() => () => cancelAnimationFrame(frame.current), []);

  // Before the first paint the wing stands exactly on the sidebar's logo, which hides while the
  // wing is out; arriving from setup, it starts large and high and flies down to it.
  const arrive = state.phase === "offer" && state.arrive === true;
  // biome-ignore lint/correctness/useExhaustiveDependencies: Placed once, when the tour mounts.
  useLayoutEffect(() => {
    if (arrive && !reducedNow.current) {
      draw({ x: innerWidth * 0.5, y: innerHeight * 0.28, angle: 205, bank: 0, size: 96 });
      fly(homePose(), FLIGHT_MS * 1.5, () => setAsking(true));
    } else {
      draw(homePose());
      setAsking(true);
    }
  }, []);

  const phase = state.phase;
  const stops = state.phase === "flight" ? state.stops : null;
  const stop = stops?.[index] ?? null;

  const end = useCallback(
    (outcome: "finished" | "stopped") => {
      setShown(null);
      setSettled(false);
      setLeaving(true);
      fly(homePose(), FLIGHT_MS, () => calls.current.onEnd(outcome, visited.current));
    },
    [fly],
  );
  const move = useCallback(
    (by: 1 | -1) => {
      if (!stops) return;
      const next = index + by;
      if (next < 0) return;
      direction.current = by;
      if (next >= stops.length) end("finished");
      else setIndex(next);
    },
    [stops, index, end],
  );

  /** Puts the ring, the card and the wing at `node`; `glideThere` flies, otherwise it jumps. */
  const settle = useCallback(
    (at: TourStop, node: HTMLElement, glideThere: boolean) => {
      const box = node.getBoundingClientRect();
      const size = card.current
        ? { width: card.current.offsetWidth, height: card.current.offsetHeight }
        : { width: 320, height: 160 };
      const placed = place(box, at.side, size, { width: innerWidth, height: innerHeight });
      setShown({ stop: at.id, card: placed.card, ring: ringOf(node) });
      if (glideThere) {
        setSettled(false);
        fly(placed.wing, FLIGHT_MS, () => setSettled(true));
      } else {
        cancelAnimationFrame(frame.current);
        draw(placed.wing);
        setSettled(true);
      }
    },
    [draw, fly],
  );

  // A stop: bring its place into view, wait for its control, then fly to it. A control that
  // never shows is passed, in the direction the person was going.
  // biome-ignore lint/correctness/useExhaustiveDependencies: A flight advances by its index alone.
  useEffect(() => {
    if (phase !== "flight" || !stop || leaving) return;
    let live = true;
    let seeking = 0;
    setShown(null);
    calls.current.onPrepare(stop);
    const started = performance.now();
    const seek = () => {
      if (!live) return;
      const node = findTarget(stop.id);
      if (node) {
        node.scrollIntoView({ block: "nearest", inline: "nearest" });
        if (!visited.current.includes(stop.id)) visited.current.push(stop.id);
        settle(stop, node, true);
        return;
      }
      if (performance.now() - started < SEEK_MS) {
        seeking = requestAnimationFrame(seek);
        return;
      }
      if (!visited.current.includes(stop.id)) visited.current.push(stop.id);
      const last = (stops?.length ?? 0) - 1;
      if (direction.current === -1 && index > 0) setIndex(index - 1);
      else if (index < last) {
        direction.current = 1;
        setIndex(index + 1);
      } else end("finished");
    };
    // One frame first, for the place just asked for to render.
    seeking = requestAnimationFrame(seek);
    return () => {
      live = false;
      cancelAnimationFrame(seeking);
    };
  }, [phase, index, leaving]);

  // Settled: the ring and the card follow their control when the window or a pane changes.
  useEffect(() => {
    if (phase !== "flight" || !stop || !settled || shown?.stop !== stop.id) return;
    const follow = () => {
      const node = findTarget(stop.id);
      if (!node) return;
      const ring = ringOf(node);
      if (
        Math.abs(ring.left - shown.ring.left) > 1 ||
        Math.abs(ring.top - shown.ring.top) > 1 ||
        Math.abs(ring.width - shown.ring.width) > 1 ||
        Math.abs(ring.height - shown.ring.height) > 1
      )
        settle(stop, node, false);
    };
    const timer = setInterval(follow, WATCH_MS);
    addEventListener("resize", follow);
    return () => {
      clearInterval(timer);
      removeEventListener("resize", follow);
    };
  }, [phase, stop, settled, shown, settle]);

  // Waiting on the logo, the wing stays on it when the window changes size.
  useEffect(() => {
    if (phase !== "offer" || !asking) return;
    const home = () => draw(homePose());
    addEventListener("resize", home);
    return () => removeEventListener("resize", home);
  }, [phase, asking, draw]);

  // Each stop is read out: its heading takes focus as its card shows. Focus is never held.
  const placed = shown?.stop;
  useEffect(() => {
    if (placed) heading.current?.focus({ preventScroll: true });
  }, [placed]);

  const keys = (event: KeyboardEvent<HTMLElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const onButton = event.target instanceof HTMLElement && event.target.closest("button") !== null;
    if (event.key === "Escape") end("stopped");
    else if (event.key === "ArrowRight" || (event.key === "Enter" && !onButton)) move(1);
    else if (event.key === "ArrowLeft") move(-1);
    else return;
    event.preventDefault();
    event.stopPropagation();
  };

  const over = { [OVER_BROWSER]: "" };
  const home = pose.current ?? homePose();
  const words = state.phase === "offer" ? offerWords(state.offer) : null;
  const last = stops !== null && index === stops.length - 1;
  return (
    <div className="tour" data-phase={phase} data-testid="tour">
      {shown && stop && shown.stop === stop.id ? (
        <div
          key={`ring-${stop.id}`}
          className="tour-ring"
          aria-hidden="true"
          style={
            {
              left: shown.ring.left,
              top: shown.ring.top,
              width: shown.ring.width,
              height: shown.ring.height,
              "--tour-radius": `${shown.ring.radius}px`,
            } as CSSProperties
          }
        />
      ) : null}
      <div
        ref={wing}
        className="tour-wing"
        data-settled={settled || (phase === "offer" && asking) ? "" : undefined}
        data-testid="tour-wing"
        aria-hidden="true"
        {...over}
      >
        <span className="tour-wing-float">
          <Wing small />
        </span>
      </div>
      {words && asking && !leaving ? (
        <section
          className="tour-card tour-offer"
          aria-labelledby={titleId}
          aria-describedby={bodyId}
          data-testid="tour-offer"
          data-placed=""
          style={{
            left: Math.max(12, home.x - home.size / 2),
            bottom: innerHeight - (home.y - (home.size * WING_RATIO) / 2) + 12,
          }}
          {...over}
        >
          <h2 id={titleId} className="tour-title">
            {words.title}
          </h2>
          <p id={bodyId} className="tour-body">
            {words.body}
          </p>
          <div className="tour-actions">
            <button
              type="button"
              className="g-button g-button-primary g-button-28"
              onClick={onTake}
            >
              {words.take}
            </button>
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              onClick={onDecline}
            >
              Not now
            </button>
          </div>
        </section>
      ) : null}
      {stop && stops && !leaving ? (
        <section
          key={`card-${stop.id}`}
          ref={card}
          className="tour-card"
          role="dialog"
          aria-modal="false"
          aria-labelledby={titleId}
          aria-describedby={bodyId}
          data-testid="tour-card"
          data-stop={stop.id}
          data-placed={shown?.stop === stop.id ? "" : undefined}
          style={shown?.stop === stop.id ? { left: shown.card.x, top: shown.card.y } : undefined}
          onKeyDown={keys}
          {...over}
        >
          <h2 id={titleId} ref={heading} tabIndex={-1} className="tour-title">
            {stop.title}
          </h2>
          <p id={bodyId} className="tour-body">
            {stop.body}
          </p>
          <div className="tour-actions">
            <span className="tour-count">
              {index + 1} of {stops.length}
            </span>
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              onClick={() => end("stopped")}
            >
              End tour
            </button>
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              disabled={index === 0}
              onClick={() => move(-1)}
            >
              Back
            </button>
            <button
              type="button"
              className="g-button g-button-primary g-button-28"
              data-testid="tour-next"
              onClick={() => move(1)}
            >
              {last ? "Done" : "Next"}
            </button>
          </div>
        </section>
      ) : null}
    </div>
  );
}
