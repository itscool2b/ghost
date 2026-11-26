import type { OrbState } from "@ghost/ui";
import {
  Archive,
  ChevronRight,
  FolderInput,
  MoreHorizontal,
  PanelLeft,
  PenLine,
  Pin,
  Search,
  Settings,
  SquarePen,
  Trash2,
} from "lucide-react";
import { motion } from "motion/react";
import { type KeyboardEvent, memo, type ReactNode, useEffect, useRef, useState } from "react";
import { DISCOVERY } from "../../fixture/discovery";
import type { AgentInfo, ChatStatus, ChatSummary, Player, Project, Workspace } from "../../kit";
import type { MarkTimeline } from "./geometry";
import { Menu } from "./Menu";
import { OrbitMark } from "./OrbitMark";
import {
  DotLoader,
  Elapsed,
  IconButton,
  Keycap,
  Pip,
  type PipKind,
  popover,
  useOutsideClose,
  useReduced,
} from "./ui";

/*
 * The sidebar (visual-language.md 4.2): 264px on the tray, rows 30px. X Money's nav hover changes
 * a row's weight, not its fill; the active chat is a piece of the pane lifted onto the desk (Aside,
 * Arc); chats that need you are grouped first and listed only there (Agent Dashboard, Harvey's
 * review queue); the agent dock is every agent's orbit, live, with its two-letter code under it
 * (Antimetal's dot strip, SpaceXAI's marker and mono label). The rows are one keyboard stop: arrows
 * move a rail, Enter opens, Left and Right fold. Hover swaps a chat's trailing slot for archive and
 * more (Superhuman). Folded, the sidebar is X's 72px icon rail.
 */

function pipFor(status: ChatStatus): PipKind {
  switch (status) {
    case "running":
      return "running";
    case "needs_approval":
      return "needs";
    case "done_unseen":
      return "done";
    case "error":
      return "error";
    default:
      return "idle";
  }
}

/** The most urgent state among a project's chats, for its row while it is folded. */
export function projectPip(project: Project): PipKind {
  const order: readonly ChatStatus[] = ["needs_approval", "error", "running", "done_unseen"];
  for (const status of order) {
    if (project.chats.some((chat) => chat.status === status)) return pipFor(status);
  }
  return "idle";
}

export type ProjectTone = "slate" | "stone" | "sage" | "clay";

/** A colour's hue in degrees, or null for a grey or something that is not #rrggbb. */
function hueOf(color: string): number | null {
  const match = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color.trim());
  if (!match) return null;
  const [r, g, b] = [match[1], match[2], match[3]].map(
    (part) => Number.parseInt(part ?? "0", 16) / 255,
  ) as [number, number, number];
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  if (delta < 0.04) return null;
  let hue: number;
  if (max === r) hue = ((g - b) / delta) % 6;
  else if (max === g) hue = (b - r) / delta + 2;
  else hue = (r - g) / delta + 4;
  return (hue * 60 + 360) % 360;
}

/**
 * A project's square, in one of four muted tones (section 3.1). The user's colour picks the tone by
 * hue; no tone is saturated, none is violet, and none sits near the signal orange, so the square
 * never competes with the one colour that means look here. Blues are slate, violets stone, warm
 * colours sage, greens clay.
 */
export function projectTone(color: string): ProjectTone {
  const hue = hueOf(color);
  if (hue === null) return "stone";
  if (hue >= 170 && hue < 245) return "slate";
  if (hue >= 245 && hue < 330) return "stone";
  if (hue >= 70 && hue < 170) return "clay";
  return "sage";
}

/** Two-letter codes printed under each dock mark, so the strip reads without tooltips. */
const AGENT_CODES: Readonly<Record<string, string>> = {
  "claude-code": "CC",
  codex: "CX",
  pi: "PI",
  "oh-my-pi": "MP",
  antigravity: "AG",
  hermes: "HM",
  cursor: "CU",
  grok: "GB",
  opencode: "OC",
};

/** An agent's dock code: its own, else the first two letters of its name. */
export function agentCode(agent: { readonly id: string; readonly name: string }): string {
  if (Object.hasOwn(AGENT_CODES, agent.id)) return AGENT_CODES[agent.id] ?? "";
  return agent.name
    .replace(/[^a-z0-9]/gi, "")
    .slice(0, 2)
    .toUpperCase();
}

/** What an agent's dock mark shows, and why: the most urgent state among its chats. */
export function agentState(
  workspace: Workspace,
  agentId: string,
): { readonly state: OrbState; readonly chat: ChatSummary | null } {
  let found: { state: OrbState; chat: ChatSummary | null } = { state: "idle", chat: null };
  const rank: Readonly<Record<OrbState, number>> = {
    idle: 0,
    done: 1,
    thinking: 2,
    streaming: 2,
    tool: 2,
    error: 3,
    waiting: 4,
  };
  for (const project of workspace.projects) {
    for (const chat of project.chats) {
      if (chat.agent !== agentId) continue;
      const next: OrbState =
        chat.status === "running"
          ? "thinking"
          : chat.status === "needs_approval"
            ? "waiting"
            : chat.status === "error"
              ? "error"
              : "idle";
      if (rank[next] > rank[found.state]) found = { state: next, chat };
    }
  }
  return found;
}

/** A dock cell's name for assistive tech and its tooltip: the agent, then its state as words. */
export function dockLabel(agent: AgentInfo, state: OrbState, chat: ChatSummary | null): string {
  if (!agent.installed) return `${agent.name} · not installed`;
  const parts = [`${agent.name} ${agent.version ?? ""}`.trim()];
  if (state === "waiting") parts.push(chat ? `needs you in ${chat.title}` : "needs you");
  else if (state === "error") parts.push(chat ? `failed in ${chat.title}` : "failed");
  else if (state === "thinking") parts.push(chat ? `working in ${chat.title}` : "working");
  if (agent.update) parts.push(`update to ${agent.update}`);
  return parts.join(" · ");
}

const AGENT_PATHS: ReadonlyMap<string, string> = new Map(
  DISCOVERY.events.flatMap((entry) =>
    entry.event.type === "agent_found" ? [[entry.event.agent, entry.event.path] as const] : [],
  ),
);

function Trailing({
  chat,
  active,
  player,
  runningSince,
}: {
  readonly chat: ChatSummary;
  readonly active: boolean;
  readonly player: Player;
  readonly runningSince: number | null;
}) {
  switch (chat.status) {
    case "running":
      return (
        <span className="o-trail">
          <DotLoader />
          <span className="o-t-mono o-mono-11 o-c3">
            {active && runningSince !== null ? (
              <Elapsed player={player} since={runningSince} />
            ) : (
              chat.updated
            )}
          </span>
        </span>
      );
    case "needs_approval":
      return (
        <span className="o-trail">
          <Pip kind="needs" />
          <span className="o-trail-word o-signal-text">Needs you</span>
        </span>
      );
    case "done_unseen":
      return (
        <span className="o-trail">
          <Pip kind="done" />
          <span className="o-t-mono o-mono-11 o-c3">{chat.updated}</span>
        </span>
      );
    case "error":
      return (
        <span className="o-trail">
          <Pip kind="error" />
          <span className="o-trail-word o-del">Failed</span>
        </span>
      );
    default:
      return <span className="o-t-mono o-mono-11 o-c3 o-trail">{chat.updated}</span>;
  }
}

/** The agent card (section 4.2): the agent's mark, version, sign-in, path, defaults and update. */
function AgentCard({
  agent,
  label,
  anchor,
  onClose,
  onSoon,
}: {
  readonly agent: AgentInfo;
  readonly label: string;
  readonly anchor: { readonly current: HTMLElement | null };
  readonly onClose: () => void;
  readonly onSoon: (what: string) => void;
}) {
  const reduced = useReduced();
  const ref = useRef<HTMLDivElement>(null);
  useOutsideClose(ref, onClose, anchor);
  // Esc closes the card first, in the capture phase, so it never stops a running turn.
  const latest = useRef(onClose);
  latest.current = onClose;
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      latest.current();
      anchor.current?.focus({ preventScroll: true });
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [anchor]);
  const model = agent.models.find((entry) => entry.id === agent.defaultModel) ?? null;
  const effort = model?.efforts.find((entry) => entry.id === model.defaultEffort) ?? null;
  return (
    <motion.div
      ref={ref}
      className="o-agent-card"
      role="dialog"
      aria-label={label}
      data-orbit-menu=""
      {...popover(reduced)}
    >
      <div className="o-agent-card-head">
        <OrbitMark agent={agent.id} size={32} surface="var(--o-raised)" ghost={!agent.installed} />
        <div className="o-agent-card-name">
          <span className="o-t-heading o-c1">{agent.name}</span>
          <span className="o-t-mono o-mono-11 o-c3">
            {agent.installed ? agent.version : "Not installed"}
          </span>
        </div>
      </div>
      {agent.installed ? (
        <dl className="o-agent-card-rows o-t-meta">
          <div className="o-prop">
            <dt className="o-c3">Account</dt>
            <dd className={agent.signedIn ? "o-c1" : "o-signal-text o-w-strong"}>
              {agent.signedIn ? "Signed in" : "Sign in"}
            </dd>
          </div>
          <div className="o-prop">
            <dt className="o-c3">Binary</dt>
            <dd className="o-t-mono o-c1 o-agent-card-path">
              {AGENT_PATHS.get(agent.id) ?? "on PATH"}
            </dd>
          </div>
          <div className="o-prop">
            <dt className="o-c3">Default</dt>
            <dd className="o-c1">
              {model?.label ?? "None"}
              {effort ? <span className="o-c3"> {effort.label}</span> : null}
            </dd>
          </div>
        </dl>
      ) : null}
      <div className="o-agent-card-actions">
        {agent.update ? (
          <button
            type="button"
            className="o-btn o-btn-ink o-btn-sm"
            onClick={() => onSoon("Updating agents")}
          >
            Update to {agent.update}
          </button>
        ) : null}
        <button
          type="button"
          className="o-btn o-btn-ghost o-btn-sm"
          onClick={() => onSoon(agent.installed ? "Agent settings" : "Installing agents")}
        >
          {agent.installed ? "Agent settings" : "Install"}
        </button>
      </div>
    </motion.div>
  );
}

function DockCell({
  agent,
  workspace,
  live,
  open,
  focusable,
  onOpen,
  onClose,
  onSoon,
}: {
  readonly agent: AgentInfo;
  readonly workspace: Workspace;
  readonly live?: { readonly timeline: MarkTimeline; readonly player: Player };
  readonly open: boolean;
  readonly focusable: boolean;
  readonly onOpen: () => void;
  readonly onClose: () => void;
  readonly onSoon: (what: string) => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const { state, chat } = agentState(workspace, agent.id);
  const label = dockLabel(agent, state, chat);
  return (
    <span className="o-dock-anchor">
      <button
        ref={ref}
        type="button"
        className="o-dock-cell"
        title={label}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        tabIndex={focusable ? 0 : -1}
        data-dock-cell=""
        data-open={open ? "" : undefined}
        onClick={open ? onClose : onOpen}
      >
        <span className="o-dock-mark">
          <OrbitMark
            agent={agent.id}
            size={24}
            surface="var(--o-dock-surface)"
            ghost={!agent.installed}
            state={state}
            live={live}
            label={label}
          />
          {/* One orange per cell: while the agent waits, its diamond says it, not the update mark. */}
          {agent.update && state !== "waiting" ? (
            <span className="o-dock-update" aria-hidden="true" />
          ) : null}
        </span>
        <span
          className="o-dock-code"
          data-missing={agent.installed ? undefined : ""}
          aria-hidden="true"
        >
          {agentCode(agent)}
        </span>
      </button>
      {open ? (
        <AgentCard agent={agent} label={label} anchor={ref} onClose={onClose} onSoon={onSoon} />
      ) : null}
    </span>
  );
}

/** Moves focus through a group of rows with the arrow keys; one row at a time is a Tab stop. */
function roving(event: KeyboardEvent<HTMLElement>, selector: string, keys: [string, string]) {
  if (event.key !== keys[0] && event.key !== keys[1]) return false;
  const rows = [...event.currentTarget.querySelectorAll<HTMLElement>(selector)];
  const index = rows.indexOf(document.activeElement as HTMLElement);
  if (index === -1) return false;
  event.preventDefault();
  const next =
    rows[Math.min(rows.length - 1, Math.max(0, index + (event.key === keys[1] ? 1 : -1)))];
  if (!next) return true;
  for (const row of rows) row.tabIndex = row === next ? 0 : -1;
  next.focus();
  return true;
}

/** A chat row's hover tools: archive, and more with the chat's menu. */
function ChatTools({
  chat,
  onArchive,
  onDelete,
  onMenu,
}: {
  readonly chat: ChatSummary;
  readonly onArchive: (chat: ChatSummary) => void;
  readonly onDelete: (chat: ChatSummary) => void;
  readonly onMenu: (open: boolean) => void;
}) {
  const [menu, setMenu] = useState(false);
  const moreRef = useRef<HTMLButtonElement>(null);
  const toggle = (open: boolean): void => {
    setMenu(open);
    onMenu(open);
  };
  return (
    <span className="o-chat-tools">
      <IconButton
        icon={Archive}
        label="Archive"
        size={24}
        tabIndex={-1}
        onClick={() => onArchive(chat)}
      />
      <span className="o-menu-anchor">
        <IconButton
          ref={moreRef}
          icon={MoreHorizontal}
          label="Chat menu"
          size={24}
          tabIndex={-1}
          active={menu}
          aria-haspopup="menu"
          aria-expanded={menu}
          onClick={() => toggle(!menu)}
        />
        {menu ? (
          <Menu
            label={chat.title}
            anchor={moreRef}
            onClose={() => toggle(false)}
            groups={[
              [
                { label: "Rename", icon: PenLine, soon: true },
                { label: "Pin", icon: Pin, soon: true },
                { label: "Move to project", icon: FolderInput, soon: true },
                {
                  label: "Archive",
                  icon: Archive,
                  shortcut: "⌘⌫",
                  onSelect: () => onArchive(chat),
                },
              ],
              [
                {
                  label: "Delete",
                  icon: Trash2,
                  destructive: true,
                  onSelect: () => onDelete(chat),
                },
              ],
            ]}
          />
        ) : null}
      </span>
    </span>
  );
}

export const Sidebar = memo(function Sidebar({
  workspace,
  player,
  liveAgent,
  timeline,
  runningSince,
  rail,
  onToggleRail,
  onOpenChat,
  onNewChat,
  onArchive,
  onDelete,
  onSoon,
}: {
  readonly workspace: Workspace;
  readonly player: Player;
  /** The open chat's agent, whose dock mark moves with the turn. */
  readonly liveAgent: string | null;
  readonly timeline: MarkTimeline;
  /** When the open chat's turn started, for its live timer. */
  readonly runningSince: number | null;
  /** Folded to the 72px icon rail. */
  readonly rail: boolean;
  readonly onToggleRail: () => void;
  readonly onOpenChat: (chat: ChatSummary) => void;
  readonly onNewChat: () => void;
  readonly onArchive: (chat: ChatSummary) => void;
  readonly onDelete: (chat: ChatSummary) => void;
  readonly onSoon: (what: string) => void;
}) {
  // What the user folded or unfolded here; a project the user never touched keeps its own state.
  const [overrides, setOverrides] = useState<ReadonlyMap<string, boolean>>(() => new Map());
  const [card, setCard] = useState<string | null>(null);
  const [menuChat, setMenuChat] = useState<string | null>(null);
  const listRef = useRef<HTMLElement>(null);
  const waiting = workspace.projects.flatMap((project) =>
    project.chats
      .filter((chat) => chat.status === "needs_approval")
      .map((chat) => ({ chat, project: project.name })),
  );
  const installed = workspace.agents.filter((agent) => agent.installed).length;
  const isOpen = (project: Project): boolean => overrides.get(project.id) ?? project.expanded;
  const fold = (project: Project, next?: boolean): void =>
    setOverrides((current) => new Map(current).set(project.id, next ?? !isOpen(project)));

  // One Tab stop for the rows: the active chat, else the first row.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const rows = [...list.querySelectorAll<HTMLElement>("[data-row]")];
    if (rows.some((row) => row.tabIndex === 0 && row === document.activeElement)) return;
    const active = rows.find((row) => row.dataset.active !== undefined) ?? rows[0];
    for (const row of rows) row.tabIndex = row === active ? 0 : -1;
  });

  const row = (
    key: string,
    className: string,
    content: ReactNode,
    props: {
      readonly onClick: () => void;
      readonly active?: boolean;
      readonly project?: string;
      readonly expanded?: boolean;
      readonly label?: string;
      readonly status?: string;
      readonly strong?: boolean;
    },
  ) => (
    <button
      key={key}
      type="button"
      className={`o-side-row ${className}`}
      data-row=""
      data-active={props.active ? "" : undefined}
      data-project={props.project}
      data-status={props.status}
      data-strong={props.strong ? "" : undefined}
      aria-current={props.active ? "page" : undefined}
      aria-expanded={props.expanded}
      aria-label={props.label}
      title={rail ? props.label : undefined}
      onClick={props.onClick}
    >
      {content}
    </button>
  );

  return (
    <aside className="o-sidebar" aria-label="Chats" data-rail={rail ? "" : undefined}>
      <div className="o-band">
        <IconButton
          icon={PanelLeft}
          label={rail ? "Show the sidebar" : "Hide the sidebar"}
          className="o-band-toggle"
          onClick={onToggleRail}
        />
      </div>
      <nav
        ref={listRef}
        className="o-side-scroll o-scroll"
        onKeyDown={(event) => {
          if (roving(event, "[data-row]", ["ArrowUp", "ArrowDown"])) return;
          const target = document.activeElement as HTMLElement | null;
          const projectId = target?.dataset.project;
          const project = workspace.projects.find((entry) => entry.id === projectId);
          if (project && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
            event.preventDefault();
            fold(project, event.key === "ArrowRight");
          }
        }}
      >
        <div className="o-side-group">
          {row(
            "new",
            "o-utility",
            <>
              <SquarePen size={16} strokeWidth={1.5} className="o-c3" aria-hidden="true" />
              <span className="o-side-title">New chat</span>
              <span className="o-keys">
                <Keycap label="Command">{"⌘"}</Keycap>
                <Keycap>N</Keycap>
              </span>
            </>,
            { onClick: onNewChat, label: "New chat" },
          )}
          {row(
            "search",
            "o-utility",
            <>
              <Search size={16} strokeWidth={1.5} className="o-c3" aria-hidden="true" />
              <span className="o-side-title">Search</span>
              <span className="o-keys">
                <Keycap label="Command">{"⌘"}</Keycap>
                <Keycap>K</Keycap>
              </span>
            </>,
            { onClick: () => onSoon("Search"), label: "Search" },
          )}
        </div>
        {waiting.length > 0 ? (
          <div className="o-side-group o-needs-group">
            <p className="o-side-label o-t-caps o-signal-text">
              Needs you <span className="o-num">{waiting.length}</span>
            </p>
            {waiting.map(({ chat, project }) =>
              row(
                chat.id,
                "o-needs-row",
                <>
                  <span className="o-needs-pip">
                    <Pip kind="needs" />
                  </span>
                  {/* One run of text: when room runs out, the project gives way before the title. */}
                  <span className="o-side-title o-c1">
                    {chat.title}
                    <span className="o-needs-project"> · {project}</span>
                  </span>
                </>,
                {
                  onClick: () => onOpenChat(chat),
                  active: chat.id === workspace.activeChatId,
                  label: `${chat.title}, ${project}, needs you`,
                },
              ),
            )}
          </div>
        ) : null}
        <div className="o-side-group">
          <p className="o-side-label o-t-caps o-c3">
            Projects <span className="o-num o-c4">{workspace.projects.length}</span>
          </p>
          {workspace.projects.map((project) => {
            const open = isOpen(project);
            // A chat that needs you lives in the inbox above: it is listed, and counted, once.
            const chats = project.chats.filter((chat) => chat.status !== "needs_approval");
            // Folded, or on the rail where no chat rows show, the row carries its chats' state.
            const pip = open && !rail ? "idle" : projectPip(project);
            const failed = project.chats.filter((chat) => chat.status === "error").length;
            return (
              <div key={project.id} className="o-project">
                {row(
                  project.id,
                  "o-project-row",
                  <>
                    <span
                      className="o-project-square"
                      data-tone={projectTone(project.color)}
                      aria-hidden="true"
                    />
                    <span className="o-side-title o-project-name">{project.name}</span>
                    <span className="o-project-side">
                      {pip === "error" ? (
                        <span className="o-project-pip">
                          <Pip kind="error" />
                          <span className="o-trail-word o-del">{failed} failed</span>
                        </span>
                      ) : pip !== "idle" ? (
                        <span className="o-project-pip">
                          <Pip kind={pip} />
                        </span>
                      ) : (
                        <span className="o-project-count o-t-mono o-mono-11 o-c3 o-num">
                          {chats.length}
                        </span>
                      )}
                      <ChevronRight
                        className="o-project-chev"
                        data-open={open ? "" : undefined}
                        size={12}
                        strokeWidth={1.75}
                        aria-hidden="true"
                      />
                    </span>
                  </>,
                  {
                    onClick: () => fold(project),
                    project: project.id,
                    expanded: open,
                    label: `${project.name}, ${chats.length} ${chats.length === 1 ? "chat" : "chats"}`,
                  },
                )}
                {open
                  ? chats.map((chat) => {
                      const active = chat.id === workspace.activeChatId;
                      const pipKind = pipFor(chat.status);
                      return (
                        <div
                          key={chat.id}
                          className="o-chat-line"
                          data-active={active ? "" : undefined}
                          data-menu={menuChat === chat.id ? "" : undefined}
                        >
                          {row(
                            chat.id,
                            "o-chat-row",
                            <>
                              <span className="o-side-title">{chat.title}</span>
                              <Trailing
                                chat={chat}
                                active={active}
                                player={player}
                                runningSince={runningSince}
                              />
                            </>,
                            {
                              onClick: () => onOpenChat(chat),
                              active,
                              status: chat.status,
                              strong: active || pipKind === "done",
                              label: chat.title,
                            },
                          )}
                          <ChatTools
                            chat={chat}
                            onArchive={onArchive}
                            onDelete={onDelete}
                            onMenu={(menuOpen) => setMenuChat(menuOpen ? chat.id : null)}
                          />
                        </div>
                      );
                    })
                  : null}
              </div>
            );
          })}
        </div>
      </nav>
      <div className="o-dock-strip">
        <p className="o-side-label o-t-caps o-c3">
          Agents{" "}
          <span className="o-num o-c4">
            {installed}/{workspace.agents.length}
          </span>
        </p>
        <div
          className="o-dock-cells"
          role="toolbar"
          aria-label="Agents"
          onKeyDown={(event) => {
            if (!roving(event, "[data-dock-cell]", ["ArrowLeft", "ArrowRight"])) {
              roving(event, "[data-dock-cell]", ["ArrowUp", "ArrowDown"]);
            }
          }}
        >
          {workspace.agents.map((agent, index) => (
            <DockCell
              key={agent.id}
              agent={agent}
              workspace={workspace}
              live={agent.id === liveAgent ? { timeline, player } : undefined}
              open={card === agent.id}
              focusable={index === 0}
              onOpen={() => setCard(agent.id)}
              onClose={() => setCard(null)}
              onSoon={onSoon}
            />
          ))}
        </div>
      </div>
      <div className="o-account">
        <span className="o-avatar" aria-hidden="true">
          L
        </span>
        <span className="o-account-text">
          <span className="o-t-ui o-w-strong o-c1">{workspace.account.label}</span>
          <span className="o-t-meta o-c3">Sign in to sync</span>
        </span>
        <IconButton icon={Settings} label="Settings" onClick={() => onSoon("Settings")} />
      </div>
    </aside>
  );
});
