import type { ReactNode } from "react";
import {
  BrowserView,
  ChangesView,
  DashboardView,
  NewChatView,
  QueueView,
  TerminalView,
} from "./Scenes";

function Glyph({ children }: { children: ReactNode }) {
  return (
    <svg
      className="part-glyph"
      width="20"
      height="20"
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

/** What else ships in the app today, each a real part of it (docs/design/visual-language.md). */
const PARTS: readonly { id: string; glyph: ReactNode; title: string; text: string }[] = [
  {
    id: "roles",
    glyph: (
      <>
        <path d="M3.5 5.5h7M3.5 10h4M3.5 14.5h7" />
        <path d="m13 8 3 2-3 2" />
      </>
    ),
    title: "Plan with one, build with another",
    text: "Let one model write the plan and hand the accepted plan to another to build it.",
  },
  {
    id: "tasks",
    glyph: (
      <>
        <rect x="3" y="4" width="14" height="5" rx="2.5" />
        <path d="M5.5 13.5h9M7.5 16.5h5" />
      </>
    ),
    title: "Background tasks in view",
    text: "Subagents and long commands wait in a tray over the message box, each with its time.",
  },
  {
    id: "accounts",
    glyph: (
      <>
        <circle cx="10" cy="10" r="6.5" />
        <path d="M10 3.5a6.5 6.5 0 0 1 5.6 3.2" strokeWidth="2" />
      </>
    ),
    title: "Past a usage limit",
    text: "When an account runs out, switch to another and the conversation carries on.",
  },
  {
    id: "phone",
    glyph: (
      <>
        <rect x="6" y="2.5" width="8" height="15" rx="2" />
        <path d="M9 15h2" />
      </>
    ),
    title: "A phone beside the chat",
    text: "An Android emulator, or an iPhone simulator on a Mac, that you and the agent both use.",
  },
  {
    id: "dictation",
    glyph: (
      <>
        <rect x="7.5" y="2.5" width="5" height="9" rx="2.5" />
        <path d="M4.5 9.5a5.5 5.5 0 0 0 11 0M10 15v2.5" />
      </>
    ),
    title: "Dictation on your device",
    text: "Talk instead of typing. Your words appear as you speak, and recognition runs on your computer.",
  },
  {
    id: "canvas",
    glyph: (
      <>
        <path d="M3.5 13.5 12 5l3 3-8.5 8.5H3.5v-3Z" />
        <path d="M10.5 6.5l3 3" />
      </>
    ),
    title: "A canvas to draw on",
    text: "Sketch the flow and say what you mean while you draw. The drawing goes with your message.",
  },
  {
    id: "memory",
    glyph: (
      <>
        <path d="M5 3.5h8.5A1.5 1.5 0 0 1 15 5v11.5H6.5A1.5 1.5 0 0 1 5 15V3.5Z" />
        <path d="M5 14.5A1.5 1.5 0 0 1 6.5 13H15M8 7h4" />
      </>
    ),
    title: "Memory every agent reads",
    text: "Project notes that follow the work from agent to agent. You can review or undo each edit.",
  },
  {
    id: "themes",
    glyph: (
      <>
        <circle cx="10" cy="10" r="7" />
        <path d="M10 3v14M10 6.5a3.5 3.5 0 0 1 0 7" />
      </>
    ),
    title: "A theme for every agent",
    text: "Ten palettes, each in light and dark, with the system's appearance followed by default.",
  },
];

/**
 * Six parts of the app, each drawn small in a panel of its own (Scenes.tsx), as X Money shows its
 * six: the panel and, beside it, the part's name and two grey lines. `wide` panels take more of
 * the row, as X Money's third and sixth do.
 */
const SIX: readonly { id: string; title: string; text: string; view: ReactNode; wide?: boolean }[] =
  [
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
      wide: true,
    },
    {
      id: "browser",
      title: "A browser beside the chat",
      text: "Open what you are building next to the conversation, point at the part that should change, and send it to the agent.",
      view: <BrowserView />,
    },
    {
      id: "dashboard",
      title: "A dashboard over every chat",
      text: "One board shows which agents are working, which need you and which have finished. It opens over the window you are in.",
      view: <DashboardView />,
    },
    {
      id: "new",
      title: "Any agent, any folder",
      text: "Pick the agent and the folder. A Git folder's chat starts in a worktree of its own, so its work stays apart.",
      view: <NewChatView chosen="claude-code" />,
      wide: true,
    },
  ];

/**
 * What the app does, as X Money sets "One app": the heading, then six panels in X Money's staggered
 * rows, each a part of the app drawn small with its name and two grey lines beside it, then the
 * rest of the workspace as plain rows between hairlines, two to a line. No numbers over the names
 * (no eyebrows); everything is in the markup, whole without scripts.
 */
export function Everything() {
  return (
    <section className="section everything" id="features" aria-labelledby="features-title">
      <div className="wrap">
        <h2 id="features-title" className="title everything-title">
          One app. <span className="tone">Everything the work needs.</span>
        </h2>
        <ul className="six">
          {SIX.map((part) => (
            <li
              key={part.id}
              className="six-item"
              data-part={part.id}
              data-wide={part.wide ? "" : undefined}
            >
              <div className="panel six-panel" aria-hidden="true">
                <div className="six-view">{part.view}</div>
              </div>
              <div className="six-text">
                <h3>{part.title}</h3>
                <p>{part.text}</p>
              </div>
            </li>
          ))}
        </ul>
        <ul className="parts" aria-label="The rest of the workspace">
          {PARTS.map((part) => (
            <li key={part.id} className="part">
              <Glyph>{part.glyph}</Glyph>
              <span>
                <strong>{part.title}</strong> {part.text}
              </span>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
