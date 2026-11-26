import { type AgentBrandId, getBrand, type OrbState, orbPosterBackground } from "@ghost/ui";
import {
  ArrowUp,
  ChevronRight,
  Laptop,
  PanelLeft,
  Plus,
  Search,
  Settings,
  SquarePen,
} from "lucide-react";
import { type CSSProperties, memo, useState } from "react";
import type { AgentInfo, ChatStatus, ChatSummary, Project, Workspace } from "../../kit";
import { Glass } from "./Glass";
import { Collapse, IconButton, Kbd } from "./ui";

const STATUS_ORB: Readonly<Record<ChatStatus, OrbState>> = {
  running: "streaming",
  needs_approval: "waiting",
  done_unseen: "done",
  error: "error",
  idle: "idle",
};

const STATUS_TEXT: Readonly<Record<ChatStatus, string>> = {
  running: "Working",
  needs_approval: "Needs your approval",
  done_unseen: "Finished, not opened yet",
  error: "Stopped with an error",
  idle: "Idle",
};

/**
 * A chat's live state as a small CSS orb in its agent's colours: a circling arc while it works, an
 * amber breath while it waits for you, a lit core once it finished unseen, red on error, and a dim
 * ember when nothing is going on.
 */
export function MiniOrb({
  agent,
  status,
  size = 14,
}: {
  readonly agent: AgentBrandId;
  readonly status: ChatStatus;
  readonly size?: number;
}) {
  const brand = getBrand(agent);
  return (
    <span
      className="au-mini-orb"
      data-status={status}
      style={
        {
          width: size,
          height: size,
          "--au-mo-bg": orbPosterBackground(brand, STATUS_ORB[status]),
          "--au-mo-glow": brand.glow,
        } as CSSProperties
      }
      aria-hidden="true"
    >
      <span className="au-mini-orb-disc" />
    </span>
  );
}

/** What a folded project's flag reports, most urgent first. */
const FLAG_ORDER: readonly ChatStatus[] = ["needs_approval", "error", "done_unseen"];

function ChatRow({ chat, active }: { readonly chat: ChatSummary; readonly active: boolean }) {
  return (
    <button
      type="button"
      className="au-chat-row"
      aria-current={active ? "page" : undefined}
      data-status={chat.status}
      aria-label={`${chat.title}, ${getBrand(chat.agent).name}, ${STATUS_TEXT[chat.status]}`}
    >
      <MiniOrb agent={chat.agent} status={chat.status} />
      <span className="au-chat-title">{chat.title}</span>
      {/* A chat that waits on you says what it wants instead of when it last moved. */}
      <span className="au-chat-meta">
        {chat.status === "needs_approval" ? "Approve" : chat.updated}
      </span>
    </button>
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
  // A folded project still says when one of its chats needs you, failed, or finished unseen.
  const flag = FLAG_ORDER.find((status) => project.chats.some((chat) => chat.status === status));
  return (
    <div className="au-project">
      <button
        type="button"
        className="au-project-row"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span className="au-project-dot" style={{ "--au-dot": project.color } as CSSProperties} />
        <span className="au-project-name">{project.name}</span>
        {!open && flag ? (
          <span
            className="au-project-flag"
            data-status={flag}
            role="img"
            aria-label={STATUS_TEXT[flag]}
          />
        ) : null}
        <span className="au-project-count">{project.chats.length}</span>
        <ChevronRight className="au-project-chevron" data-open={open ? "" : undefined} size={14} />
      </button>
      <Collapse open={open}>
        <div className="au-chat-list">
          {project.chats.map((chat) => (
            <ChatRow key={chat.id} chat={chat} active={chat.id === activeChatId} />
          ))}
        </div>
      </Collapse>
    </div>
  );
}

function dockTip(agent: AgentInfo): string {
  if (!agent.installed) return `${agent.name}: not installed`;
  if (agent.update) return `${agent.name} ${agent.version}: update to ${agent.update}`;
  return `${agent.name} ${agent.version}`;
}

/**
 * Every detected agent as an orb in a glass tray; updates wear a badge, missing ones are ghosts.
 * Clicking an installed agent sends the next message to it (with its last-used model).
 */
const AgentDock = memo(function AgentDock({
  agents,
  activeAgent,
  onSelect,
}: {
  readonly agents: readonly AgentInfo[];
  readonly activeAgent: AgentBrandId;
  readonly onSelect: (agent: AgentBrandId) => void;
}) {
  const updates = agents.filter((agent) => agent.update !== null).length;
  return (
    <div className="au-agents">
      <div className="au-sb-label">
        <span>Agents</span>
        {updates > 0 ? (
          <span className="au-update-note">
            <ArrowUp size={10} strokeWidth={2.6} />
            {updates === 1 ? "1 update" : `${updates} updates`}
          </span>
        ) : null}
      </div>
      <div className="au-agent-tray">
        {agents.map((agent) => (
          <button
            key={agent.id}
            type="button"
            className="au-agent-slot"
            data-installed={agent.installed ? "" : undefined}
            data-active={agent.id === activeAgent ? "" : undefined}
            aria-label={dockTip(agent)}
            aria-pressed={agent.installed ? agent.id === activeAgent : undefined}
            data-tip={dockTip(agent)}
            data-tip-side="top"
            data-tip-align="start"
            onClick={() => {
              if (agent.installed) onSelect(agent.id);
            }}
          >
            {agent.installed ? (
              <span
                className="au-agent-orb"
                style={{ background: orbPosterBackground(getBrand(agent.id), "idle") }}
              />
            ) : (
              <span className="au-agent-ghost">
                <Plus size={11} strokeWidth={2.2} />
              </span>
            )}
            {agent.update ? (
              <span className="au-update-badge">
                <ArrowUp size={8} strokeWidth={3.2} />
              </span>
            ) : null}
          </button>
        ))}
      </div>
    </div>
  );
});

/** The auracode mark: a ring of the active agent's light around a bright core. */
export function AuraMark({ size = 18 }: { readonly size?: number }) {
  return (
    <span className="au-logo" style={{ width: size, height: size }} aria-hidden="true">
      <span className="au-logo-core" />
    </span>
  );
}

/**
 * The frosted sidebar: wordmark, new chat and search, projects with their chats (each with a live
 * mini-orb), the agent dock and the local profile.
 */
export const Sidebar = memo(function Sidebar({
  workspace,
  activeAgent,
  onSelectAgent,
}: {
  readonly workspace: Workspace;
  readonly activeAgent: AgentBrandId;
  readonly onSelectAgent: (agent: AgentBrandId) => void;
}) {
  return (
    <Glass as="aside" level={1} radius={18} className="au-sidebar" aria-label="Projects and agents">
      <div className="au-sb-band">
        <IconButton label="Hide sidebar" tip="bottom">
          <PanelLeft size={16} strokeWidth={1.75} />
        </IconButton>
      </div>
      <div className="au-brand">
        <AuraMark />
        <span className="au-wordmark">auracode</span>
      </div>
      <div className="au-sb-actions">
        <button
          type="button"
          className="au-newchat"
          data-active={workspace.activeChatId === null ? "" : undefined}
        >
          <SquarePen size={15} strokeWidth={1.75} />
          <span>New chat</span>
          <Kbd>⌘N</Kbd>
        </button>
        <IconButton label="Search  ⌘K" className="au-search-btn">
          <Search size={15} strokeWidth={1.75} />
        </IconButton>
      </div>
      <nav className="au-sb-scroll" aria-label="Projects">
        <div className="au-sb-label">
          <span>Projects</span>
          <IconButton label="New project" className="au-sb-add" tip="left">
            <Plus size={14} strokeWidth={1.75} />
          </IconButton>
        </div>
        {workspace.projects.map((project) => (
          <ProjectGroup key={project.id} project={project} activeChatId={workspace.activeChatId} />
        ))}
      </nav>
      <AgentDock agents={workspace.agents} activeAgent={activeAgent} onSelect={onSelectAgent} />
      <div className="au-account">
        <button type="button" className="au-account-btn">
          <span className="au-avatar">
            <Laptop size={14} strokeWidth={1.75} />
          </span>
          <span className="au-account-text">
            <span className="au-account-name">{workspace.account.label} profile</span>
            <span className="au-account-sub">Sync is off</span>
          </span>
        </button>
        <IconButton label="Settings" tip="top">
          <Settings size={16} strokeWidth={1.75} />
        </IconButton>
      </div>
    </Glass>
  );
});
