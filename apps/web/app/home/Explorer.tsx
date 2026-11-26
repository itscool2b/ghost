import { type ReactNode, useEffect, useRef, useState } from "react";
import { useInView, usePageVisible, useReducedMotion } from "../motion";
import {
  BrowserView,
  ChangesView,
  DashboardView,
  HandoffView,
  QueueView,
  TerminalView,
} from "./Scenes";
import { Stage } from "./Stage";

const FEATURES: readonly { id: string; title: string; text: string; view: ReactNode }[] = [
  {
    id: "handoff",
    title: "Switch agents mid-chat",
    text: "Hand the chat to another agent at any turn. The handoff stays in view, so you always know who did what.",
    view: <HandoffView />,
  },
  {
    id: "queue",
    title: "Queue and steer",
    text: "Type while a turn runs. Your next message waits in a queue, or steers the turn already under way.",
    view: <QueueView />,
  },
  {
    id: "changes",
    title: "Every change in view",
    text: "See each file a turn changed, comment on any line, and take back what you do not want.",
    view: <ChangesView />,
  },
  {
    id: "terminal",
    title: "Chat or terminal",
    text: "Open any chat in its agent's own interface, right inside Ghost. Back in Chat, the turns are already there.",
    view: <TerminalView />,
  },
  {
    id: "browser",
    title: "A browser beside the chat",
    text: "Open what you are building next to the conversation, point at the part that should change, and send it to the agent.",
    view: <BrowserView />,
  },
  {
    id: "dashboard",
    title: "A dashboard for every chat",
    text: "One board shows which agents are working, which need you and which have finished.",
    view: <DashboardView />,
  },
];

/** How long each view holds before the next one shows by itself, in ms. */
const HOLD = 7000;

/**
 * What Ghost does, one part at a time: the list at the left names six parts of the app, each in two
 * lines, and the stage beside it shows the chosen one. In view the list moves on by itself, a
 * hairline under the chosen row filling as its time runs; a press, the keyboard or a pointer
 * resting on the list holds it. The rows are plain toggle buttons whose words are always shown,
 * so the section reads whole without scripts, where the first picture stands. The pictures are
 * authored illustrations of supported states, hidden from assistive technology.
 */
export function Explorer() {
  const [active, setActive] = useState(0);
  const [held, setHeld] = useState(false);
  const grid = useRef<HTMLDivElement>(null);
  const reduced = useReducedMotion();
  const visible = usePageVisible();
  const inView = useInView(grid, { threshold: 0.35 });
  const auto = !reduced && visible && inView && !held;

  useEffect(() => {
    if (!auto) return;
    const timer = window.setTimeout(() => setActive((active + 1) % FEATURES.length), HOLD);
    return () => window.clearTimeout(timer);
  }, [auto, active]);

  return (
    <section className="section explorer" id="features" aria-labelledby="explorer-title">
      <div className="wrap">
        <div className="chapter-head">
          <h2 id="explorer-title" className="title">
            One window. <span className="tone">Everything an agent does, in view.</span>
          </h2>
          <p className="lead">
            Long turns are where agents earn their keep, and where you lose track of them. Ghost
            keeps the work in front of you: who is doing what, what changed, and what is waiting on
            you.
          </p>
        </div>
        <div className="explorer-grid" ref={grid} data-auto={auto ? "" : undefined} data-zone="">
          <ul
            className="explorer-list"
            aria-label="What Ghost does"
            onPointerEnter={() => setHeld(true)}
            onPointerLeave={() => setHeld(false)}
            onFocus={() => setHeld(true)}
            onBlur={() => setHeld(false)}
          >
            {FEATURES.map((feature, i) => (
              <li key={feature.id}>
                <button
                  type="button"
                  className="explorer-item"
                  aria-pressed={i === active}
                  onClick={() => setActive(i)}
                >
                  <span className="explorer-item-title">{feature.title}</span>
                  <span className="explorer-item-text">{feature.text}</span>
                  <span className="explorer-item-bar" aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
          <Stage quiet className="explorer-stage">
            {FEATURES.map((feature, i) => (
              <div
                key={feature.id}
                className="explorer-view work-view"
                data-card={feature.id}
                data-active={i === active ? "" : undefined}
                aria-hidden="true"
              >
                {feature.view}
              </div>
            ))}
          </Stage>
        </div>
      </div>
    </section>
  );
}
