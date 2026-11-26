import { getBrand, monogram, type OrbState, orbPosterBackground } from "@ghost/ui";
import {
  ArrowUp,
  ChevronRight,
  PanelLeft,
  Plus,
  Search,
  Settings,
  SquarePen,
  UserRound,
} from "lucide-react";
import { type CSSProperties, memo, useId, useState } from "react";
import type { AgentInfo, ChatStatus, ChatSummary, Project, Workspace } from "../../kit";
import { Collapse, Glass, IconButton, Kbd, RADIUS } from "./primitives";

/** The orb state a sidebar row's mini-orb borrows its colours from. */
const MINI_ORB_STATE: Readonly<Record<ChatStatus, OrbState>> = {
  running: "thinking",
  needs_approval: "waiting",
  done_unseen: "done",
  error: "error",
  idle: "idle",
};

const STATUS_LABEL: Readonly<Record<ChatStatus, string>> = {
  running: "Working",
  needs_approval: "Needs your approval",
  done_unseen: "Finished, not seen yet",
  error: "Stopped with an error",
  idle: "Idle",
};

/**
 * A chat's live mini-orb: the agent's colours in CSS (no WebGL in the sidebar). Running turns a
 * ring of light around it, needing approval breathes amber, finished-but-unseen glows, errors go
 * red, idle rests dim.
 */
export function MiniOrb({
  agent,
  status,
  size = 14,
}: {
  readonly agent: string;
  readonly status: ChatStatus;
  readonly size?: number;
}) {
  const brand = getBrand(agent);
  return (
    <span
      className="halo-mini-orb"
      data-status={status}
      role="img"
      aria-label={`${brand.name}, ${STATUS_LABEL[status].toLowerCase()}`}
      style={
        {
          width: size,
          height: size,
          "--mini-orb": orbPosterBackground(brand, MINI_ORB_STATE[status]),
          "--mini-accent": brand.accent,
          "--mini-glow": brand.glow,
        } as CSSProperties
      }
    >
      <span className="halo-mini-orb-core" />
    </span>
  );
}

/** The wordmark's mark: a ring of light around a bright point. */
function HaloMark() {
  const id = `halo-mark-${useId().replace(/[^\w-]/g, "")}`;
  return (
    <svg className="halo-mark" viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#ffffff" />
          <stop offset="0.55" className="halo-mark-mid" />
          <stop offset="1" className="halo-mark-end" />
        </linearGradient>
      </defs>
      <circle cx="12" cy="12" r="8.25" fill="none" stroke={`url(#${id})`} strokeWidth="2.5" />
      <circle cx="12" cy="12" r="2.6" fill="#ffffff" />
    </svg>
  );
}

function ChatRow({ chat, active }: { readonly chat: ChatSummary; readonly active: boolean }) {
  return (
    <li>
      <button
        type="button"
        className="halo-chat"
        data-halo-light=""
        aria-current={active ? "page" : undefined}
        data-status={chat.status}
        title={`${chat.title}: ${STATUS_LABEL[chat.status]}`}
      >
        <MiniOrb agent={chat.agent} status={chat.status} />
        <span className="halo-chat-title">{chat.title}</span>
        {chat.status === "done_unseen" ? (
          <span
            className="halo-unseen"
            role="img"
            aria-label="New"
            style={{ "--chat-glow": getBrand(chat.agent).glow } as CSSProperties}
          />
        ) : (
          <span className="halo-chat-time">{chat.updated}</span>
        )}
      </button>
    </li>
  );
}

function ProjectGroup({
  project,
  activeChatId,
}: {
  readonly project: Project;
  readonly activeChatId: string | null;
}) {
  const [open, setOpen] = useState(project.expanded);
  const busy = project.chats.filter((chat) => chat.status === "running").length;
  return (
    <li className="halo-project" data-open={open ? "" : undefined}>
      <button
        type="button"
        className="halo-project-row"
        data-halo-light=""
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span
          className="halo-project-dot"
          style={{ "--project": project.color } as CSSProperties}
          aria-hidden="true"
        />
        <span className="halo-project-name">{project.name}</span>
        {!open && busy > 0 ? <span className="halo-project-busy">{busy} running</span> : null}
        <span className="halo-project-count">{project.chats.length}</span>
        <ChevronRight size={14} className="halo-project-chevron" aria-hidden="true" />
      </button>
      <Collapse open={open}>
        <ul className="halo-chats">
          {project.chats.map((chat) => (
            <ChatRow key={chat.id} chat={chat} active={chat.id === activeChatId} />
          ))}
        </ul>
      </Collapse>
    </li>
  );
}

function agentCaption(agent: AgentInfo): string {
  if (!agent.installed) return "Not installed";
  if (agent.update) return `${agent.version} · update to ${agent.update}`;
  if (agent.signedIn === false) return `${agent.version} · sign in`;
  return `${agent.version} · ready`;
}

/**
 * The agent dock: every agent as an orb in a recessed tray, the active one lifted. Missing agents
 * are hollow rings with their initials, to install; an arrow badge marks an update. The line under
 * the tray names whichever orb you point at.
 */
function AgentDock({
  agents,
  active,
}: {
  readonly agents: readonly AgentInfo[];
  readonly active: string;
}) {
  const [pointed, setPointed] = useState<string | null>(null);
  const shown = agents.find((agent) => agent.id === (pointed ?? active)) ?? agents[0];
  const updates = agents.filter((agent) => agent.update !== null).length;
  return (
    <section className="halo-dock" aria-label="Agents">
      <div className="halo-section-head">
        <span>Agents</span>
        {updates > 0 ? (
          <span className="halo-dock-updates">
            {updates === 1 ? "1 update" : `${updates} updates`}
          </span>
        ) : null}
      </div>
      <div className="halo-dock-tray">
        <Glass material="well" radius={RADIUS.well} />
        {agents.map((agent) => {
          const brand = getBrand(agent.id);
          return (
            <button
              key={agent.id}
              type="button"
              className="halo-dock-orb"
              data-installed={agent.installed ? "" : undefined}
              data-active={agent.id === active ? "" : undefined}
              aria-label={`${agent.name}: ${agentCaption(agent)}`}
              onPointerEnter={() => setPointed(agent.id)}
              onPointerLeave={() => setPointed(null)}
              onFocus={() => setPointed(agent.id)}
              onBlur={() => setPointed(null)}
              style={
                {
                  "--dock-orb": orbPosterBackground(brand, "idle"),
                  "--dock-glow": brand.glow,
                } as CSSProperties
              }
            >
              <span className="halo-dock-orb-body">
                {agent.installed ? null : (
                  <span className="halo-dock-monogram">{monogram(agent.name)}</span>
                )}
              </span>
              {agent.update ? (
                <span className="halo-update-badge" aria-hidden="true">
                  <ArrowUp size={8} strokeWidth={3} />
                </span>
              ) : null}
            </button>
          );
        })}
      </div>
      {shown ? (
        <p className="halo-dock-caption">
          <span className="halo-dock-name">{shown.name}</span>
          <span className="halo-dock-detail" data-update={shown.update ? "" : undefined}>
            {agentCaption(shown)}
          </span>
        </p>
      ) : null}
    </section>
  );
}

/**
 * The sidebar: a slab of glass floating inside the window, the traffic lights resting in its top
 * corner the way macOS 26 places them.
 */
export const Sidebar = memo(function Sidebar({
  workspace,
  newChat,
  activeAgent,
}: {
  readonly workspace: Workspace;
  /** True when the open chat is new: "New chat" shows as the current place. */
  readonly newChat: boolean;
  /** The agent the composer points at: lifted in the dock. */
  readonly activeAgent: string;
}) {
  return (
    <aside className="halo-sidebar" aria-label="Sidebar">
      <Glass material="slab" radius={RADIUS.slab} />
      <div className="halo-sidebar-top">
        <IconButton icon={PanelLeft} label="Hide sidebar" />
      </div>
      <div className="halo-brand">
        <HaloMark />
        <span className="halo-wordmark">auracode</span>
      </div>
      <div className="halo-sidebar-actions">
        <button
          type="button"
          className="halo-new-chat"
          data-halo-light=""
          aria-current={newChat ? "page" : undefined}
        >
          <SquarePen size={15} aria-hidden="true" />
          <span>New chat</span>
          <Kbd>⌘N</Kbd>
        </button>
        <label className="halo-search">
          <Search size={15} aria-hidden="true" />
          <input type="search" placeholder="Search" aria-label="Search chats" />
          <Kbd>⌘K</Kbd>
        </label>
      </div>
      <nav className="halo-projects" aria-label="Projects">
        <div className="halo-section-head">
          <span>Projects</span>
          <IconButton icon={Plus} label="Add a project" size={14} className="halo-section-add" />
        </div>
        <ul className="halo-project-list">
          {workspace.projects.map((project) => (
            <ProjectGroup
              key={project.id}
              project={project}
              activeChatId={workspace.activeChatId}
            />
          ))}
        </ul>
      </nav>
      <AgentDock agents={workspace.agents} active={activeAgent} />
      <div className="halo-account">
        <span className="halo-avatar" aria-hidden="true">
          <UserRound size={15} />
        </span>
        <span className="halo-account-text">
          <span className="halo-account-name">{workspace.account.label}</span>
          <span className="halo-account-sub">On this Mac</span>
        </span>
        <IconButton icon={Settings} label="Settings" />
      </div>
    </aside>
  );
});
