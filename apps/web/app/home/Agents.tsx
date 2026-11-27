import { AGENT_BRANDS } from "@ghost/ui";
import { type ReactNode, type RefObject, useEffect, useRef, useState } from "react";
import { useInView, usePageVisible, useReducedMotion } from "../motion";
import { AGENTS, SpriteMark } from "./Sprite";

function Glyph({ children }: { children: ReactNode }) {
  return (
    <svg
      className="row-glyph"
      width="18"
      height="18"
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.25"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

const ROWS: readonly { text: string; glyph: ReactNode }[] = [
  {
    text: "Nine agents, and any that speaks ACP",
    glyph: (
      <>
        <rect x="3" y="3" width="5.5" height="5.5" rx="1.5" />
        <rect x="11.5" y="3" width="5.5" height="5.5" rx="1.5" />
        <rect x="3" y="11.5" width="5.5" height="5.5" rx="1.5" />
        <path d="M14.25 11.5v5.5M11.5 14.25H17" />
      </>
    ),
  },
  {
    text: "Your own sign-ins and plans",
    glyph: (
      <>
        <circle cx="7.5" cy="10" r="3.5" />
        <path d="M11 10h6.5M15 10v2.5M17.5 10v2" />
      </>
    ),
  },
  {
    text: "Hand a chat to another at any turn",
    glyph: (
      <>
        <path d="M3.5 7h11l-3-3" />
        <path d="M16.5 13h-11l3 3" />
      </>
    ),
  },
];

/** How long each agent stays chosen while the picker runs through them. */
const BEAT = 1600;

/**
 * Which agent the picker holds: it moves down the list one at a time while the section is on
 * screen, the tab is visible and motion is welcome; otherwise the first stays chosen.
 */
function useChosen(target: RefObject<Element | null>, count: number): number {
  const [index, setIndex] = useState(0);
  const reduced = useReducedMotion();
  const visible = usePageVisible();
  const inView = useInView(target, { threshold: 0.3 });
  const live = !reduced && visible && inView;
  useEffect(() => {
    if (!live) return;
    const timer = window.setInterval(() => setIndex((at) => (at + 1) % count), BEAT);
    return () => window.clearInterval(timer);
  }, [live, count]);
  useEffect(() => {
    if (reduced) setIndex(0);
  }, [reduced]);
  return index;
}

/**
 * Every agent, as X Money sets its first part: at the left the heading, a grey paragraph and three
 * plain facts between hairlines at the column's foot; at the right a tall panel holding New chat's
 * agent menu, each agent's own mark beside its name, running through them one at a time. The list
 * is the page's list of the agents it runs; the marks are decorative.
 */
export function Agents() {
  const panel = useRef<HTMLDivElement>(null);
  const chosen = useChosen(panel, AGENTS.length);
  return (
    <section className="section split agents" id="agents" aria-labelledby="agents-title">
      <div className="wrap split-grid">
        <div className="split-text">
          <h2 id="agents-title" className="title">
            Your agents. <span className="tone">Each one as it is.</span>
          </h2>
          <p className="lead">
            Every agent runs through its own harness and your own sign-in. Ghost never reads, stores
            or proxies its credentials, so what you pay for and what you are signed in to stay
            between you and it.
          </p>
          <ul className="split-rows">
            {ROWS.map((row) => (
              <li key={row.text}>
                <Glyph>{row.glyph}</Glyph>
                {row.text}
              </li>
            ))}
          </ul>
        </div>
        <div className="panel split-panel pick-panel" ref={panel} data-zone="">
          <div className="pick">
            <p className="pick-search" aria-hidden="true">
              <svg
                width="14"
                height="14"
                viewBox="0 0 16 16"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.4"
                strokeLinecap="round"
                aria-hidden="true"
                focusable="false"
              >
                <circle cx="7" cy="7" r="4.5" />
                <path d="m10.5 10.5 3 3" />
              </svg>
              Search agents
            </p>
            <ul className="pick-list" aria-label="Agents Ghost runs">
              {AGENTS.map((agent, index) => (
                <li
                  key={agent}
                  className="pick-row"
                  data-chosen={index === chosen ? "" : undefined}
                >
                  <SpriteMark agent={agent} size={22} />
                  <span className="pick-name">{AGENT_BRANDS[agent].name}</span>
                  <span className="pick-state" aria-hidden="true">
                    {index === chosen ? (
                      <svg
                        width="14"
                        height="14"
                        viewBox="0 0 16 16"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="1.6"
                        strokeLinecap="round"
                        strokeLinejoin="round"
                        aria-hidden="true"
                        focusable="false"
                      >
                        <path d="m3.5 8.5 3 3 6-7" />
                      </svg>
                    ) : (
                      "Ready"
                    )}
                  </span>
                </li>
              ))}
              <li className="pick-row pick-more">
                <span className="pick-plus" aria-hidden="true">
                  +
                </span>
                <span className="pick-name">Any agent that speaks ACP</span>
              </li>
            </ul>
          </div>
        </div>
      </div>
    </section>
  );
}
