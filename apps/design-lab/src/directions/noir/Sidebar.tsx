import { GlassPanel } from "@ghost/ui";
import { ChevronRight, PanelLeft, Plus, Search, Settings, SquarePen } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { type FocusEvent, memo, useState } from "react";
import type { AgentInfo, ChatStatus, ChatSummary, Project, Workspace } from "../../kit";
import { AgentAction, IconButton, Kbd, MiniOrb, reveal, useReduced } from "./ui";

const STATUS_LABEL: Readonly<Record<ChatStatus, string>> = {
  running: "Working",
  needs_approval: "Needs approval",
  done_unseen: "Finished, not yet seen",
  error: "Stopped with an error",
  idle: "Idle",
};

/** Chat statuses a folded project surfaces, most urgent first. */
const ATTENTION_ORDER: readonly ChatStatus[] = ["needs_approval", "error", "done_unseen"];

/**
 * The sidebar: a smoked glass slab inset from the window edge, its corners concentric with the
 * window's. Everything in it is quiet type. Colour appears only where it says something: a chat's
 * orb (which agent, and what it is doing), Approve, an error, and the agent the composer is on.
 */
export const Sidebar = memo(function Sidebar({
  workspace,
  currentAgent,
  activeStatus,
}: {
  readonly workspace: Workspace;
  /** The composer's agent: the one orb in the dock that keeps its colour. */
  readonly currentAgent: string;
  /** The open chat's status as this window knows it (an approval answered here), if it differs. */
  readonly activeStatus: ChatStatus | null;
}) {
  const newChat = workspace.activeChatId === null;
  return (
    <aside className="n-side" aria-label="Sidebar">
      <GlassPanel elevation={1} squircle radius={12} className="n-side-panel">
        <div className="n-side-top">
          <span className="n-wordmark">
            aura<em>code</em>
          </span>
          <IconButton icon={PanelLeft} label="Hide sidebar" className="n-side-toggle" />
        </div>
        <div className="n-side-actions">
          <button type="button" className="n-side-btn" data-selected={newChat ? "" : undefined}>
            <SquarePen className="n-side-btn-icon" size={16} strokeWidth={1.6} aria-hidden="true" />
            <span className="n-side-btn-label">New chat</span>
            <Kbd>{"⌘N"}</Kbd>
          </button>
          <button type="button" className="n-side-btn" data-quiet="">
            <Search className="n-side-btn-icon" size={16} strokeWidth={1.6} aria-hidden="true" />
            <span className="n-side-btn-label">Search</span>
            <Kbd>{"⌘K"}</Kbd>
          </button>
        </div>
        <nav className="n-side-scroll" aria-label="Projects">
          <div className="n-side-label">
            <span>Projects</span>
            <IconButton icon={Plus} label="New project" size="sm" />
          </div>
          {workspace.projects.map((project) => (
            <ProjectGroup
              key={project.id}
              project={project}
              activeChatId={workspace.activeChatId}
              activeStatus={activeStatus}
            />
          ))}
        </nav>
        <AgentDock agents={workspace.agents} current={currentAgent} />
        <div className="n-account">
          <span className="n-avatar" aria-hidden="true">
            {workspace.account.label.slice(0, 1).toUpperCase()}
          </span>
          <span className="n-account-text">
            <span className="n-account-name">{workspace.account.label}</span>
            <span className="n-account-sub">Sign in to sync</span>
          </span>
          <IconButton icon={Settings} label="Settings" />
        </div>
      </GlassPanel>
    </aside>
  );
});

const ProjectGroup = memo(function ProjectGroup({
  project,
  activeChatId,
  activeStatus,
}: {
  readonly project: Project;
  readonly activeChatId: string | null;
  readonly activeStatus: ChatStatus | null;
}) {
  const reduced = useReduced();
  // The open chat's project arrives expanded (the workspace sees to it); the reader may fold it.
  const [expanded, setExpanded] = useState(
    project.expanded || project.chats.some((chat) => chat.id === activeChatId),
  );
  const statusOf = (chat: ChatSummary): ChatStatus =>
    chat.id === activeChatId && activeStatus ? activeStatus : chat.status;
  // Folded, a project still says when one of its chats wants the reader, most urgent first.
  const attention = ATTENTION_ORDER.find((status) =>
    project.chats.some((chat) => statusOf(chat) === status),
  );
  return (
    <div className="n-project">
      <button
        type="button"
        className="n-project-row"
        aria-expanded={expanded}
        onClick={() => setExpanded(!expanded)}
      >
        <ChevronRight className="n-project-chev" size={12} strokeWidth={1.8} aria-hidden="true" />
        <span className="n-project-name">{project.name}</span>
        {!expanded && attention ? (
          <span className="n-project-dot" data-status={attention} title={STATUS_LABEL[attention]} />
        ) : null}
        <span className="n-project-count n-num">{project.chats.length}</span>
      </button>
      <AnimatePresence initial={false}>
        {expanded ? (
          <motion.ul key="chats" className="n-chats" {...reveal(reduced)}>
            {project.chats.map((chat) => (
              <ChatRow
                key={chat.id}
                chat={chat}
                status={statusOf(chat)}
                active={chat.id === activeChatId}
              />
            ))}
          </motion.ul>
        ) : null}
      </AnimatePresence>
    </div>
  );
});

/**
 * A chat: its agent's orb, its title, and its state in words where the state needs the reader
 * (Working, Approve, Error); finished and idle chats show when they last changed, an unseen
 * finish marked with a dot.
 */
const ChatRow = memo(function ChatRow({
  chat,
  status,
  active,
}: {
  readonly chat: ChatSummary;
  readonly status: ChatStatus;
  readonly active: boolean;
}) {
  const reduced = useReduced();
  return (
    <li>
      <button
        type="button"
        className="n-chat"
        data-status={status}
        aria-current={active ? "page" : undefined}
        title={`${chat.title}, ${STATUS_LABEL[status].toLowerCase()}`}
      >
        <MiniOrb agent={chat.agent} status={status} size={10} className="n-chat-orb" />
        <span className="n-chat-title">{chat.title}</span>
        <span className="n-chat-meta n-num">
          {status === "running" ? (
            <span className="n-chat-working" data-shimmer={reduced ? undefined : ""}>
              Working
            </span>
          ) : status === "needs_approval" ? (
            "Approve"
          ) : status === "error" ? (
            "Error"
          ) : (
            <>
              {status === "done_unseen" ? (
                <span className="n-chat-unseen" aria-hidden="true" />
              ) : null}
              {chat.updated}
            </>
          )}
        </span>
      </button>
    </li>
  );
});

/**
 * The agent dock: one orb per agent, in grey until it matters. The composer's agent keeps its
 * colour, and an orb takes its colour back under the pointer. Missing agents are empty rings. The
 * line under the orbs names whatever needs attention (an update), or the agent being looked at,
 * with the action for it.
 */
function AgentDock({
  agents,
  current,
}: {
  readonly agents: readonly AgentInfo[];
  readonly current: string;
}) {
  const [looked, setLooked] = useState<AgentInfo | null>(null);
  const update = agents.find((agent) => agent.update !== null) ?? null;
  const installed = agents.filter((agent) => agent.installed).length;
  const focus = looked ?? update;
  // The line keeps its agent while the pointer or focus moves from an orb to the line's button.
  const onBlur = (event: FocusEvent<HTMLElement>): void => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setLooked(null);
  };
  return (
    <section
      className="n-dock-agents"
      aria-label="Agents"
      onPointerLeave={() => setLooked(null)}
      onBlur={onBlur}
    >
      <div className="n-side-label">
        <span>Agents</span>
        <span className="n-side-label-meta n-num">{installed} ready</span>
      </div>
      <div className="n-dock-row">
        {agents.map((agent) => (
          <button
            key={agent.id}
            type="button"
            className="n-dock-agent"
            data-missing={agent.installed ? undefined : ""}
            data-current={agent.id === current ? "" : undefined}
            aria-label={dockLabel(agent)}
            onPointerEnter={() => setLooked(agent)}
            onFocus={() => setLooked(agent)}
          >
            <MiniOrb
              agent={agent.id}
              status="live"
              size={18}
              ghost={!agent.installed}
              className="n-dock-orb"
            />
            {agent.update ? <span className="n-dock-badge" aria-hidden="true" /> : null}
          </button>
        ))}
      </div>
      <div className="n-dock-line" aria-live="polite">
        {focus ? <DockLine agent={focus} /> : <span>All agents up to date</span>}
      </div>
    </section>
  );
}

function dockLabel(agent: AgentInfo): string {
  if (!agent.installed) return `${agent.name}, not installed`;
  if (agent.update) return `${agent.name} ${agent.version}, update to ${agent.update}`;
  return `${agent.name} ${agent.version}`;
}

function DockLine({ agent }: { readonly agent: AgentInfo }) {
  if (!agent.installed) {
    return (
      <>
        <span className="n-dock-name">{agent.name}</span>
        <span className="n-dock-sub">Not installed</span>
        <AgentAction kind="install" agent={agent} />
      </>
    );
  }
  return (
    <>
      <span className="n-dock-name">{agent.name}</span>
      <span className="n-dock-sub n-num">
        {agent.update ? `${agent.version} → ${agent.update}` : agent.version}
      </span>
      {agent.update ? <AgentAction kind="update" agent={agent} /> : null}
    </>
  );
}
