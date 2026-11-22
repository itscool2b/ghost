import { PixelLoader, StatusDot } from "@o7/ui/graphite";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  Ellipsis,
  KanbanSquare,
  LayoutDashboard,
  Pin,
  Plus,
  Search,
  SlidersHorizontal,
  SquarePen,
  Workflow,
} from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type {
  ChatSummary,
  CoordinationMember,
  ProjectSummary,
  WorkspaceSnapshot,
} from "../../bindings";
import { folderStateLabel, statusWord } from "../../lib/chat/present";
import { commandKey, Keys } from "./Keys";

export function projectsOf(
  chats: readonly ChatSummary[],
): { folder: string; chats: ChatSummary[] }[] {
  const projects = new Map<string, ChatSummary[]>();
  for (const chat of chats) {
    const list = projects.get(chat.folder) ?? [];
    list.push(chat);
    projects.set(chat.folder, list);
  }
  return [...projects].map(([folder, list]) => ({ folder, chats: list }));
}

/**
 * The chats the sidebar's list shows, top to bottom: each open project's chats, in the order they
 * are drawn. Folded projects and a folded section show none. The "Needs you" rows repeat chats
 * that are also in this list, so they are not counted twice.
 */
export function visibleChatIds(
  projects: readonly { readonly folder: string; readonly chats: readonly ChatSummary[] }[],
  folded: ReadonlySet<string>,
  sectionFolded: boolean,
): string[] {
  if (sectionFolded) return [];
  return projects.flatMap((project) =>
    folded.has(project.folder) ? [] : project.chats.map((chat) => chat.id),
  );
}

/**
 * Which chat Ctrl+1 to Ctrl+9 (Cmd on macOS) opens: 0 for the first. The physical digit key
 * counts too, for layouts whose digits need Shift. `null` for any other key or while composing.
 */
export function chatShortcutIndex(event: {
  readonly key: string;
  readonly code: string;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly isComposing: boolean;
}): number | null {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.isComposing) return null;
  const digit = /^Digit([1-9])$/.exec(event.code)?.[1] ?? /^[1-9]$/.exec(event.key)?.[0];
  return digit === undefined ? null : Number(digit) - 1;
}

export function projectName(folder: string): string {
  const parts = folder.split(/[\\/]/).filter((part) => part !== "");
  return parts[parts.length - 1] ?? folder;
}

/**
 * A sidebar with nothing to show yet waits this long before it draws placeholder rows: a list
 * that answers at once must never flash them.
 */
export const SKELETON_AFTER_MS = 150;
/** The lists are this late before the sidebar says, quietly, what the app is doing. */
export const OPENING_NOTE_AFTER_MS = 2000;

/** Whether `on` has held for `ms` without a break. */
function useHeldFor(on: boolean, ms: number): boolean {
  const [held, setHeld] = useState(false);
  useEffect(() => {
    if (!on) {
      setHeld(false);
      return;
    }
    const timer = setTimeout(() => setHeld(true), ms);
    return () => clearTimeout(timer);
  }, [on, ms]);
  return on && held;
}

/**
 * The sidebar before its first list, on a launch with nothing kept from the last one: two projects
 * and their chats as bars of the rows' own size. It says nothing and is not read out; the section
 * around it is marked busy.
 */
function SidebarSkeleton() {
  return (
    <ul className="chat-skeleton" aria-hidden="true" data-testid="chats-skeleton">
      <li data-project="" />
      <li />
      <li />
      <li />
      <li data-project="" />
      <li />
    </ul>
  );
}

/** How long a chat that just appeared keeps its arrival mark: its row's rise, with room to spare. */
const ARRIVAL_MS = 700;

/**
 * The chats that appeared after the list first showed, each for ARRIVAL_MS, so their rows rise in
 * once (motion.css) while the rows already there never move. Measured before paint, so a new row's
 * first frame is already its arrival's first frame.
 */
export function useArrivals(ids: readonly string[] | null): ReadonlySet<string> {
  const known = useRef<Set<string> | null>(null);
  const timers = useRef(new Set<ReturnType<typeof setTimeout>>());
  const [arrived, setArrived] = useState<ReadonlySet<string>>(() => new Set());
  const key = ids === null ? null : ids.join("\n");
  // biome-ignore lint/correctness/useExhaustiveDependencies: The ids are read through their key.
  useLayoutEffect(() => {
    if (ids === null) return;
    if (known.current === null) {
      known.current = new Set(ids);
      return;
    }
    const seen = known.current;
    const fresh = ids.filter((id) => !seen.has(id));
    if (fresh.length === 0) return;
    for (const id of fresh) seen.add(id);
    setArrived((current) => new Set([...current, ...fresh]));
    const timer = setTimeout(() => {
      timers.current.delete(timer);
      setArrived((current) => {
        const next = new Set(current);
        for (const id of fresh) next.delete(id);
        return next;
      });
    }, ARRIVAL_MS);
    timers.current.add(timer);
  }, [key]);
  useEffect(() => {
    const pending = timers.current;
    return () => {
      for (const timer of pending) clearTimeout(timer);
    };
  }, []);
  return arrived;
}

/**
 * The sidebar's navigation (Grok's order): Search and New chat, the chats waiting on you, then the
 * projects with their chats, and a quiet "Archived chats" row at the end, like Grok's "See all".
 */
export function ChatList({
  active = true,
  chats,
  error,
  selected,
  onSelect,
  onNewChat,
  organization,
  onNewProject,
  onProjectNewChat,
  onCustomize,
  onDashboard,
  onProjectSettings,
  onChatOptions,
  archived = false,
  onShowArchived,
  onSearch,
  onProjectBoard,
  boardProject = null,
  members,
  opening = false,
  onRetry,
}: {
  readonly active?: boolean;
  readonly chats: readonly ChatSummary[] | null;
  readonly error: string | null;
  /**
   * The database has not answered with the lists yet: `chats` is what the last launch kept, or
   * nothing. The rows read as they will; only a wait long enough to notice is mentioned.
   */
  readonly opening?: boolean;
  /** Reads the lists again, for the error's Retry. */
  readonly onRetry?: () => void;
  readonly selected: string | null;
  readonly onSelect: (chatId: string) => void;
  readonly onNewChat: () => void;
  readonly organization?: WorkspaceSnapshot | null;
  readonly onNewProject?: () => void;
  readonly onProjectNewChat?: (project: ProjectSummary) => void;
  readonly onCustomize?: () => void;
  /**
   * Opens the Dashboard; the row counts the chats whose agent works now, after a volt dot while
   * one needs you.
   */
  readonly onDashboard?: () => void;
  readonly onProjectSettings?: (project: ProjectSummary) => void;
  readonly onChatOptions?: (chat: ChatSummary) => void;
  readonly archived?: boolean;
  readonly onShowArchived?: () => void;
  readonly onSearch?: () => void;
  /**
   * Coordinated projects (an experiment): opens a project's page, with its board, agents and
   * Context. Absent while the experiment is off, and then nothing of it shows here.
   */
  readonly onProjectBoard?: (project: ProjectSummary) => void;
  /** The project whose page is open. */
  readonly boardProject?: string | null;
  /** Coordinator and worker chats, by chat: each wears its part's mark before its title. */
  readonly members?: ReadonlyMap<string, CoordinationMember>;
}) {
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [folded, setFolded] = useState<ReadonlySet<string>>(new Set());
  const [sectionFolded, setSectionFolded] = useState(false);
  const search = useRef<HTMLInputElement>(null);
  const all = chats ?? [];
  const arrived = useArrivals(chats === null ? null : chats.map((chat) => chat.id));
  const unanswered = opening && error === null;
  const skeleton = useHeldFor(unanswered && chats === null, SKELETON_AFTER_MS);
  const late = useHeldFor(unanswered, OPENING_NOTE_AFTER_MS);
  // The rows that take the placeholders' place ease in once, so the swap is not a cut.
  const followsSkeleton = useRef(false);
  if (skeleton) followsSkeleton.current = true;
  const filtered = all.filter((chat) =>
    `${chat.title ?? "New chat"} ${chat.folder} ${chat.agent_name}`
      .toLowerCase()
      .includes(query.toLowerCase()),
  );
  const membership = new Map(organization?.chats.map((entry) => [entry.chat.id, entry]));
  const projects = organization
    ? organization.projects
        .map((project) => ({
          folder: project.id,
          name: project.name,
          color: project.color,
          project,
          chats: filtered
            .filter((chat) => membership.get(chat.id)?.project_id === project.id)
            .sort(
              (a, b) => Number(membership.get(b.id)?.pinned) - Number(membership.get(a.id)?.pinned),
            ),
        }))
        .filter(
          (project) =>
            !query ||
            project.chats.length > 0 ||
            project.name.toLowerCase().includes(query.toLowerCase()),
        )
    : projectsOf(filtered).map((project) => ({
        ...project,
        name: projectName(project.folder),
        color: null,
        project: null,
      }));
  const waiting = filtered.filter((chat) => chat.needs_attention);
  const visible = visibleChatIds(projects, folded, sectionFolded);
  const shortcuts = useRef(visible);
  shortcuts.current = visible;
  useEffect(() => {
    if (!active) return;
    const keys = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !(event.metaKey || event.ctrlKey) || event.altKey) return;
      const index = chatShortcutIndex(event);
      if (index !== null) {
        // Like Ctrl+J, a chat never opens behind an open dialog.
        if (document.querySelector("dialog[open]")) return;
        const chatId = shortcuts.current[index];
        if (chatId === undefined) return;
        event.preventDefault();
        onSelect(chatId);
      } else if (event.key.toLowerCase() === "k") {
        event.preventDefault();
        if (onSearch) onSearch();
        else setSearching(true);
      } else if (event.key.toLowerCase() === "n") {
        event.preventDefault();
        onNewChat();
      }
    };
    window.addEventListener("keydown", keys);
    return () => window.removeEventListener("keydown", keys);
  }, [onNewChat, onSearch, onSelect, active]);
  useEffect(() => {
    if (searching) search.current?.focus();
  }, [searching]);
  const section = archived ? "Archived" : "Projects";
  const working = chats?.filter((chat) => chat.status === "running").length ?? 0;
  const needing = chats?.filter((chat) => chat.needs_attention).length ?? 0;
  return (
    <nav aria-label="Chats" className="chat-navigation" data-testid="chat-list">
      <div className="chat-nav-actions">
        <button
          type="button"
          className="chat-nav-action"
          data-field=""
          aria-label="Search"
          aria-keyshortcuts="Control+K Meta+K"
          onClick={() => (onSearch ? onSearch() : setSearching((value) => !value))}
          aria-expanded={onSearch ? undefined : searching}
        >
          <Search size={16} />
          <span>Search</span>
          <Keys keys={[commandKey(), "K"]} />
        </button>
        <button
          type="button"
          className="chat-nav-action"
          data-testid="new-chat-button"
          aria-label="New chat"
          aria-keyshortcuts="Control+N Meta+N"
          onClick={onNewChat}
        >
          <SquarePen size={16} />
          <span>New chat</span>
          <Keys keys={[commandKey(), "N"]} />
        </button>
        {onCustomize ? (
          <button
            type="button"
            className="chat-nav-action"
            onClick={onCustomize}
            data-testid="customize-button"
            aria-label="Customize"
          >
            <SlidersHorizontal size={16} />
            <span>Customize</span>
          </button>
        ) : null}
        {onDashboard ? (
          <button
            type="button"
            className="chat-nav-action"
            onClick={onDashboard}
            data-testid="dashboard-button"
            aria-label={[
              "Dashboard",
              needing > 0 ? `${needing} ${needing === 1 ? "needs" : "need"} you` : null,
              working > 0 ? `${working} working` : null,
            ]
              .filter(Boolean)
              .join(", ")}
          >
            <LayoutDashboard size={16} />
            <span>Dashboard</span>
            {needing > 0 || working > 0 ? (
              <span className="chat-nav-count" aria-hidden="true">
                {needing > 0 ? <StatusDot tone="volt" /> : null}
                {working > 0 ? working : null}
              </span>
            ) : null}
          </button>
        ) : null}
        {searching ? (
          <input
            ref={search}
            className="g-input chat-sidebar-search"
            aria-label="Search chats"
            placeholder="Find a chat…"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.preventDefault();
                setSearching(false);
                setQuery("");
              }
            }}
          />
        ) : null}
      </div>
      {error !== null ? (
        <p role="alert" className="ui-error chat-sidebar-error" data-testid="chats-error">
          <span>{error}</span>
          {onRetry ? (
            <button type="button" className="g-button g-button-ghost g-button-24" onClick={onRetry}>
              Retry
            </button>
          ) : null}
        </p>
      ) : null}
      {late ? (
        <p role="status" className="chat-sidebar-empty" data-testid="chats-opening">
          Finishing the last session
        </p>
      ) : null}
      <div className="chat-project-scroll">
        {waiting.length > 0 ? (
          <section className="chat-attention" aria-label="Needs you">
            <h2 className="chat-section-label">
              <span className="chat-section-title">Needs you</span>
              <span className="chat-section-count">{waiting.length}</span>
            </h2>
            {waiting.map((chat) => (
              <button
                key={chat.id}
                type="button"
                className="chat-attention-row"
                onClick={() => onSelect(chat.id)}
              >
                <span className="chat-attention-mark">
                  <StatusDot tone="volt" />
                </span>
                <span>
                  {chat.title ?? "New chat"}
                  <small>
                    {" · "}
                    {organization?.projects.find(
                      (project) => project.id === membership.get(chat.id)?.project_id,
                    )?.name ?? projectName(chat.folder)}
                  </small>
                </span>
              </button>
            ))}
          </section>
        ) : null}
        <section
          className="chat-project-section"
          aria-label={section}
          aria-busy={unanswered && chats === null ? true : undefined}
        >
          <h2 className="chat-section-label">
            <button
              type="button"
              className="chat-section-toggle"
              aria-expanded={!sectionFolded}
              onClick={() => setSectionFolded((value) => !value)}
            >
              <span className="chat-section-title">{section}</span>
              <ChevronDown className="chat-section-chevron" size={12} aria-hidden="true" />
            </button>
            {onNewProject && !archived ? (
              <button
                type="button"
                className="g-button g-button-ghost g-button-icon g-button-24 chat-section-action"
                title="New project"
                aria-label="New project"
                onClick={onNewProject}
              >
                <Plus size={14} />
              </button>
            ) : null}
          </h2>
          {sectionFolded ? null : (
            <>
              {chats !== null && all.length === 0 && (archived || projects.length === 0) ? (
                <p className="chat-sidebar-empty">
                  {archived ? "No archived chats." : "Your projects and chats will appear here."}
                </p>
              ) : null}
              {query && filtered.length === 0 ? (
                <p className="chat-sidebar-empty">No chats match “{query}”.</p>
              ) : null}
              {skeleton ? <SidebarSkeleton /> : null}
              <ul
                className="chat-projects"
                data-settled={followsSkeleton.current && chats !== null ? "" : undefined}
              >
                {projects.map((project) => {
                  const open = !folded.has(project.folder);
                  return (
                    <li key={project.folder}>
                      <div className="chat-project-heading">
                        <button
                          type="button"
                          className="chat-project-toggle"
                          title={project.folder}
                          aria-expanded={open}
                          onClick={() =>
                            setFolded((current) => {
                              const next = new Set(current);
                              if (next.has(project.folder)) next.delete(project.folder);
                              else next.add(project.folder);
                              return next;
                            })
                          }
                        >
                          {/* A neutral square: project colours never reach the chrome. */}
                          <span className="chat-project-swatch" aria-hidden="true">
                            <ChevronRight size={12} />
                          </span>
                          <span>{project.name}</span>
                          {open ? null : <FoldedState chats={project.chats} />}
                        </button>
                        {onProjectBoard && project.project && !archived ? (
                          <button
                            type="button"
                            className="g-button g-button-ghost g-button-icon g-button-24 chat-project-new"
                            aria-label={`Board for ${project.name}`}
                            title={`${project.name}: board, agents and Context`}
                            aria-pressed={boardProject === project.project.id}
                            data-testid="project-board-button"
                            onClick={() => {
                              if (project.project) onProjectBoard(project.project);
                            }}
                          >
                            <KanbanSquare size={14} />
                          </button>
                        ) : null}
                        {onProjectNewChat && project.project && !archived ? (
                          <button
                            type="button"
                            className="g-button g-button-ghost g-button-icon g-button-24 chat-project-new"
                            aria-label={`New chat in ${project.name}`}
                            title={`New chat in ${project.name}`}
                            data-testid="project-new-chat"
                            onClick={() => {
                              if (project.project) onProjectNewChat(project.project);
                            }}
                          >
                            <Plus size={14} />
                          </button>
                        ) : null}
                        {onProjectSettings && project.project ? (
                          <button
                            type="button"
                            className="g-button g-button-ghost g-button-icon g-button-24 chat-row-menu"
                            aria-label={`Settings for ${project.name}`}
                            onClick={() => {
                              if (project.project) onProjectSettings(project.project);
                            }}
                          >
                            <Ellipsis size={14} />
                          </button>
                        ) : null}
                      </div>
                      {open ? (
                        <ul className="chat-chats">
                          {project.chats.map((chat) => (
                            <li
                              key={chat.id}
                              className="chat-row-container"
                              data-arrived={arrived.has(chat.id) ? "" : undefined}
                            >
                              <ChatRow
                                chat={chat}
                                selected={chat.id === selected}
                                onSelect={onSelect}
                                shortcut={visible.indexOf(chat.id)}
                                member={members?.get(chat.id)}
                              />
                              {membership.get(chat.id)?.pinned ? (
                                <Pin className="chat-pin-mark" size={10} aria-label="Pinned" />
                              ) : null}
                              {onChatOptions ? (
                                <button
                                  type="button"
                                  className="g-button g-button-ghost g-button-icon g-button-24 chat-row-menu"
                                  aria-label={`Options for ${chat.title ?? "New chat"}`}
                                  onClick={() => onChatOptions(chat)}
                                >
                                  <Ellipsis size={14} />
                                </button>
                              ) : null}
                            </li>
                          ))}
                        </ul>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            </>
          )}
          {onShowArchived ? (
            <button type="button" className="chat-see-all" onClick={onShowArchived}>
              {archived ? "Back to projects" : "Archived chats"}
            </button>
          ) : null}
        </section>
      </div>
    </nav>
  );
}

/**
 * A folded project says what its chats are doing where its chevron sits: a volt square while one
 * needs you, the loader while one works, a red square when one failed. Otherwise it is quiet.
 */
function FoldedState({ chats }: { readonly chats: readonly ChatSummary[] }) {
  if (chats.some((chat) => chat.needs_attention))
    return (
      <span className="chat-project-state">
        <StatusDot tone="volt" label="A chat needs you" />
      </span>
    );
  if (chats.some((chat) => chat.status === "running" || chat.status === "starting"))
    return (
      <span className="chat-project-state">
        <PixelLoader size={12} tempo="tool" label="A chat is working" />
      </span>
    );
  if (chats.some((chat) => chat.status === "failed"))
    return (
      <span className="chat-project-state">
        <StatusDot tone="red" label="A chat failed" />
      </span>
    );
  return null;
}

function ChatRow({
  chat,
  selected,
  onSelect,
  shortcut = -1,
  member,
}: {
  readonly chat: ChatSummary;
  readonly selected: boolean;
  readonly onSelect: (chatId: string) => void;
  /** The chat's part in a coordinated project, if it has one. */
  readonly member?: CoordinationMember;
  /** The row's place among the visible chats: the first nine open with Ctrl+1 to Ctrl+9. */
  readonly shortcut?: number;
}) {
  const folder = folderStateLabel(chat.folder_state);
  const label = chat.needs_attention
    ? "Waiting for your answer"
    : (folder ?? statusWord(chat.status));
  const running = chat.status === "running" || chat.status === "starting";
  return (
    <button
      type="button"
      className="chat-row"
      aria-current={selected}
      title={`${member ? `${member.role === "coordinator" ? "Coordinator" : "Worker"} · ` : ""}${chat.agent_name} · ${label}\n${chat.folder}`}
      data-test-role={member?.role}
      data-testid="chat-row"
      data-test-chat-id={chat.id}
      data-test-status={chat.status}
      data-test-attention={String(chat.needs_attention)}
      data-test-folder-state={chat.folder_state}
      data-running={running ? "" : undefined}
      aria-keyshortcuts={
        shortcut >= 0 && shortcut < 9 ? `Control+${shortcut + 1} Meta+${shortcut + 1}` : undefined
      }
      onClick={() => onSelect(chat.id)}
    >
      <span className="chat-row-title" data-testid="chat-row-title">
        {member ? (
          <span className="chat-row-role" aria-hidden="true">
            {member.role === "coordinator" ? <Workflow size={12} /> : <Bot size={12} />}
          </span>
        ) : null}
        {chat.title ?? "New chat"}
      </span>
      {chat.needs_attention ? (
        <span className="chat-row-status">
          <StatusDot tone="volt" label={label} />
        </span>
      ) : chat.status === "failed" || folder !== null ? (
        <span className="chat-row-status">
          <StatusDot tone="red" label={label} />
        </span>
      ) : running ? (
        <span className="chat-row-status">
          <PixelLoader size={12} tempo="tool" label={label} />
        </span>
      ) : null}
    </button>
  );
}
