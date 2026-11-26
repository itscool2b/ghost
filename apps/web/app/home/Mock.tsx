import type { AgentBrandId } from "@ghost/ui";
import { AGENT_BRANDS } from "@ghost/ui";
import type { CSSProperties, ReactNode } from "react";
import { SpriteMark } from "./Sprite";

/*
 * Illustration parts that mirror the desktop in its flat look: solid panes on the window's colour,
 * the chat header, the transcript's gutter, the composer with access on its left and the agent
 * picker on its right, tool rows and the Changes panel. They draw supported app states only.
 * Everything here is decorative and sits inside aria-hidden views; the words around them carry the
 * meaning.
 */

export type IconName =
  | "panel"
  | "more"
  | "attach"
  | "chevron"
  | "chevron-down"
  | "arrow-up"
  | "copy"
  | "retry"
  | "fork"
  | "receipt"
  | "close"
  | "refresh"
  | "folder"
  | "file"
  | "edit"
  | "terminal"
  | "search"
  | "new"
  | "settings"
  | "shield"
  | "sliders"
  | "check"
  | "globe"
  | "lock"
  | "back"
  | "forward"
  | "crosshair"
  | "plus"
  | "board";

const ICONS: Record<IconName, ReactNode> = {
  panel: (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="2" />
      <path d="M9.5 3v10" />
    </>
  ),
  more: (
    <>
      <circle cx="3.5" cy="8" r="0.75" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="0.75" fill="currentColor" stroke="none" />
      <circle cx="12.5" cy="8" r="0.75" fill="currentColor" stroke="none" />
    </>
  ),
  attach: (
    <path d="M10.6 4.8 6.1 9.3a1.25 1.25 0 0 0 1.8 1.8l4.6-4.6a2.6 2.6 0 0 0-3.7-3.7L4.2 7.4a4 4 0 0 0 5.7 5.7l3.6-3.6" />
  ),
  chevron: <path d="m6.5 4.5 3.5 3.5-3.5 3.5" />,
  "chevron-down": <path d="m5 6.5 3 3 3-3" />,
  "arrow-up": <path d="M8 12.5v-9M4.5 7 8 3.5 11.5 7" />,
  copy: (
    <>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.5" />
      <path d="M10.5 3.5V3a1 1 0 0 0-1-1H3.5a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1H4" />
    </>
  ),
  retry: (
    <>
      <path d="M3 8a5 5 0 1 0 1.6-3.7" />
      <path d="M3 2.8v2.7h2.7" />
    </>
  ),
  fork: (
    <>
      <circle cx="4.5" cy="3.5" r="1.5" />
      <circle cx="11.5" cy="3.5" r="1.5" />
      <circle cx="8" cy="12.5" r="1.5" />
      <path d="M4.5 5v1a2 2 0 0 0 2 2h3a2 2 0 0 0 2-2V5M8 8v3" />
    </>
  ),
  receipt: (
    <>
      <path d="M4 2.5h8v11H4z" />
      <path d="M6 5.5h4M6 8h4M6 10.5h2" />
    </>
  ),
  close: <path d="m4.5 4.5 7 7m0-7-7 7" />,
  refresh: (
    <>
      <path d="M13 8a5 5 0 0 1-8.6 3.5M3 8a5 5 0 0 1 8.6-3.5" />
      <path d="M11.8 2.2v2.5H9.3M4.2 13.8v-2.5h2.5" />
    </>
  ),
  folder: (
    <path d="M2.5 4.5a1 1 0 0 1 1-1h3l1.5 1.5h4.5a1 1 0 0 1 1 1v5.5a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1z" />
  ),
  file: (
    <>
      <path d="M4 2.5h5l3 3v8H4z" />
      <path d="M9 2.5v3h3" />
    </>
  ),
  edit: (
    <>
      <path d="M12 7.5v-2l-3-3H4v11h3.5" />
      <path d="M9 2.5v3h3" />
      <path d="m9.5 13.5.4-1.8 3.4-3.4 1.4 1.4-3.4 3.4z" />
    </>
  ),
  terminal: (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="2" />
      <path d="m5 6.5 1.8 1.5L5 9.5M8.5 10H11" />
    </>
  ),
  search: (
    <>
      <circle cx="7" cy="7" r="4" />
      <path d="m10 10 3 3" />
    </>
  ),
  new: (
    <>
      <path d="M8 3H4a1.5 1.5 0 0 0-1.5 1.5v7A1.5 1.5 0 0 0 4 13h7a1.5 1.5 0 0 0 1.5-1.5V8" />
      <path d="m11.5 2.5 2 2L8 10H6V8z" />
    </>
  ),
  settings: (
    <>
      <circle cx="8" cy="8" r="2" />
      <path d="M8 2v1.5M8 12.5V14M2 8h1.5M12.5 8H14M3.8 3.8l1 1M11.2 11.2l1 1M3.8 12.2l1-1M11.2 4.8l1-1" />
    </>
  ),
  shield: (
    <>
      <path d="M8 2.2 3 4v3.6c0 3 2.1 5.3 5 6.2 2.9-.9 5-3.2 5-6.2V4z" />
      <path d="m5.8 8 1.6 1.6 2.9-3.1" />
    </>
  ),
  sliders: (
    <>
      <path d="M3 4.5h6M12 4.5h1M3 11.5h1M7 11.5h6" />
      <circle cx="10.5" cy="4.5" r="1.5" />
      <circle cx="5.5" cy="11.5" r="1.5" />
    </>
  ),
  check: <path d="m3.5 8.3 2.8 2.8 6.2-6.4" />,
  globe: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M2.5 8h11M8 2.5c1.7 1.5 2.5 3.4 2.5 5.5S9.7 12 8 13.5C6.3 12 5.5 10.1 5.5 8S6.3 4 8 2.5Z" />
    </>
  ),
  lock: (
    <>
      <rect x="4" y="7" width="8" height="6" rx="1.5" />
      <path d="M5.75 7V5.5a2.25 2.25 0 0 1 4.5 0V7" />
    </>
  ),
  back: <path d="M12.5 8h-9M7 4.5 3.5 8 7 11.5" />,
  forward: <path d="M3.5 8h9M9 4.5l3.5 3.5L9 11.5" />,
  crosshair: (
    <>
      <circle cx="8" cy="8" r="4.5" />
      <path d="M8 1.5v3M8 11.5v3M1.5 8h3M11.5 8h3" />
    </>
  ),
  plus: <path d="M8 3.5v9M3.5 8h9" />,
  board: (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="2" />
      <path d="M6.2 3v10M9.8 3v10" />
    </>
  ),
};

export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg
      className="m-icon"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {ICONS[name]}
    </svg>
  );
}

/** The 2x3 pixel loader the app draws while an agent works. */
export function Loader() {
  return (
    <svg className="m-loader" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      {[
        [3, 2, 1],
        [7, 2, 3],
        [3, 5, 0],
        [7, 5, 2],
        [3, 8, 0],
        [7, 8, 1],
      ].map(([x, y, level]) => (
        <rect key={`${x}-${y}`} x={x} y={y} width="2" height="2" data-level={level} />
      ))}
    </svg>
  );
}

/** The chat's toolbar: title, project, a status only when it says something, two controls. */
export function Header({
  title,
  project,
  working,
}: {
  title: string;
  project: string;
  working?: boolean;
}) {
  return (
    <div className="m-head">
      <span className="m-title">{title}</span>
      <span className="m-project">{project}</span>
      {working ? (
        <span className="m-status">
          <Loader />
          Working
        </span>
      ) : null}
      <span className="m-head-end">
        <span className="m-icon-button">
          <Icon name="panel" />
        </span>
        <span className="m-icon-button">
          <Icon name="more" />
        </span>
      </span>
    </div>
  );
}

/** The agent picker pill on the composer's right: the agent's mark, its name and the model. */
export function AgentChip({ agent, model }: { agent: AgentBrandId; model?: string }) {
  return (
    <span className="m-agent-chip" data-agent={agent}>
      <SpriteMark agent={agent} size={16} />
      <span className="m-strong">{AGENT_BRANDS[agent].name}</span>
      {model ? <span className="m-model">{model}</span> : null}
      <Icon name="chevron-down" size={12} />
    </span>
  );
}

/** Typed text: each character arrives on its own beat when the scene runs. */
export function Typed({ text }: { text: string }) {
  return (
    <span className="m-typed" aria-hidden="true">
      {Array.from(text).map((char, i) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: characters repeat; the index is identity.
          key={i}
          className="m-char"
          style={{ "--c": i } as CSSProperties}
        >
          {char}
        </span>
      ))}
    </span>
  );
}

/** Streamed prose: words arrive one after another, the way an agent's reply streams in. */
export function Streamed({ text }: { text: string }) {
  return (
    <>
      {text.split(" ").map((word, i) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: words repeat; the index is identity.
          key={i}
          className="m-word"
          style={{ "--w": i } as CSSProperties}
        >
          {word}{" "}
        </span>
      ))}
    </>
  );
}

/**
 * The composer: the message on top, one row of ghost controls under it. Access sits on the left
 * (with attach), the agent picker and send on the right, as in the desktop.
 */
export function Composer({
  agent,
  model,
  text,
  typed,
  placeholder,
  send,
  access = "Full access",
}: {
  agent: AgentBrandId;
  model?: string;
  text?: ReactNode;
  typed?: boolean;
  placeholder?: string;
  send: ReactNode;
  access?: string;
}) {
  return (
    <div className="m-composer">
      <p className={text ? "m-input" : "m-input is-empty"}>
        {text ?? placeholder}
        {typed ? <span className="m-caret" /> : null}
      </p>
      <div className="m-controls">
        <span className="m-attach">
          <Icon name="attach" />
        </span>
        <span className="m-access">
          <Icon name="shield" size={14} />
          {access}
          <Icon name="chevron-down" size={12} />
        </span>
        <span className="m-controls-end">
          {/* Keyed by agent so a new pick flips in rather than repainting in place. */}
          <AgentChip key={agent} agent={agent} model={model} />
          {send}
        </span>
      </div>
    </div>
  );
}

export function SendDisc({ ready }: { ready?: boolean }) {
  return (
    <span className={ready ? "m-disc is-ready" : "m-disc"}>
      <Icon name="arrow-up" size={14} />
    </span>
  );
}

/** A turn's footer: copy, retry, fork and the receipt, then the time. */
export function Actions({ time }: { time: string }) {
  return (
    <div className="m-actions">
      {(["copy", "retry", "fork", "receipt"] as const).map((name) => (
        <span key={name} className="m-icon-button">
          <Icon name={name} />
        </span>
      ))}
      <span className="m-time">{time}</span>
    </div>
  );
}

export function Fold({ children }: { children: ReactNode }) {
  return (
    <p className="m-line m-fold">
      <span className="m-gutter">
        <Icon name="chevron" size={12} />
      </span>
      {children}
    </p>
  );
}

export function Tool({
  icon,
  verb,
  object,
  children,
  live,
}: {
  icon: IconName;
  verb: string;
  object: string;
  children?: ReactNode;
  live?: boolean;
}) {
  return (
    <p className="m-line m-tool">
      <span className="m-gutter">{live ? <Loader /> : <Icon name={icon} size={14} />}</span>
      <span className="m-verb">{verb}</span>
      <code className="m-code">{object}</code>
      {children}
    </p>
  );
}

/** A diff stat: green additions, red deletions. */
export function Stat({ add, del }: { add?: number; del?: number }) {
  return (
    <span className="m-stat">
      {add ? <span className="m-add">+{add}</span> : null}
      {del ? <span className="m-del">−{del}</span> : null}
    </span>
  );
}

/** The agent's name over its turn, with its mark. */
export function TurnAgent({ agent }: { agent: AgentBrandId }) {
  return (
    <p className="m-turn-agent">
      <SpriteMark agent={agent} size={16} />
      {AGENT_BRANDS[agent].name}
    </p>
  );
}

export function Check() {
  return (
    <svg className="m-check" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <path d="m2.5 6.2 2.3 2.3 4.7-4.9" fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}
