import { springs, usePrefersReducedMotion } from "@ghost/ui";
import { AgentMark, PixelLoader, StatusDot } from "@ghost/ui/graphite";
import { Check, ChevronRight, Square, X } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { createPortal } from "react-dom";
import type { ChatProcesses, ChatSummary, ProcessesSnapshot } from "../../bindings";
import type { AppStore } from "../../lib/chat/appStore";
import {
  type Arrivals,
  boardOf,
  boardSummary,
  holdsAgent,
  LANE_TITLES,
  LANES,
  type Lane,
  stateLine,
} from "../../lib/dashboard";
import { type DashboardCleared, dashboardCleared } from "../../lib/dashboardCleared";
import { errorMessage } from "../../lib/errors";
import { chatStop } from "../../lib/ipc";
import { type LineEditsApi, lineEditsApi } from "../../lib/lineEdits";
import { ago } from "../../lib/plugins/format";
import {
  cpuPercent,
  formatBytes,
  formatUptime,
  listTree,
  orderedTasks,
  type ProcessesApi,
  processesApi,
  REFRESH_MS,
} from "../../lib/processes";
import { EMPTY_WORKSPACE, type WorkspaceStore } from "../../lib/workspace";
import { TaskKindIcon, TaskMark, taskFacts, useClock } from "../chat/BackgroundTasks";
import { projectName } from "../chat/ChatList";
import { DialogClose, showDialog } from "../chat/Modal";
import { terminalChats } from "../chat/terminalChatRuntime";
import { LineEditsCard } from "./LineEditsCard";
import "./dashboard.css";

const noSubscribe = () => () => {};
const emptyWorkspace = () => EMPTY_WORKSPACE;

/** The snapshot shown and the one before it, for processor use between the two. */
interface Reading {
  readonly previous: ProcessesSnapshot | null;
  readonly current: ProcessesSnapshot | null;
}

const EMPTY: Record<Lane, string> = {
  attention: "Nothing needs you.",
  working: "No agent is working.",
  finished: "Nothing finished in the last day.",
};

/** A working card lists this many of its running tasks; the programs disclosure has the rest. */
const TASKS_SHOWN = 3;

/** Finished shows this many cards until asked for every one. */
export const FINISHED_SHOWN = 8;

/**
 * A card arrives at the top of its lane, rising into place as the lane makes room, and leaves its
 * old lane by fading while the cards below close up. It never crosses the board over the others.
 * Reduced motion places it at once.
 */
function cardMotion(reduced: boolean) {
  if (reduced) return {};
  return {
    layout: "position" as const,
    initial: { opacity: 0, y: 6 },
    animate: { opacity: 1, y: 0 },
    exit: { opacity: 0, transition: { duration: 0.12, ease: "easeOut" as const } },
    transition: springs.smooth,
  };
}

/**
 * The Dashboard: every chat on a board of three lanes, Needs you, Working and Finished, each a card
 * that opens its chat. A working card says what its agent and its tasks are doing, and folds the
 * programs it runs (read from the process table every two seconds while it is open), each with
 * Stop. A lane keeps its order: a chat that changes lane arrives at the top of the new one. Under
 * the board, AI line edits shows the lines agents' edits changed over the last year.
 *
 * It opens over the window and goes again, and never takes the chats' place (owner, 26 November
 * 2026: "the dashboard shouldnt be a whole different thing it should be something that breifly
 * opens up without going to a whole different location"): a modal panel over the chats, which stay
 * where they were behind it. Escape, its close button, a click beside it or opening a chat close
 * it; the native dialog keeps focus inside and gives it back to what opened it.
 */
export function Dashboard({
  app,
  workspace,
  api = processesApi,
  lineEdits = lineEditsApi,
  cleared = dashboardCleared,
  stopChat = chatStop,
  onClose,
  onOpenChat,
}: {
  readonly app: AppStore;
  /** Names each chat's project, and leaves archived chats off the board. */
  readonly workspace?: WorkspaceStore;
  readonly api?: ProcessesApi;
  /** The lines agents' edits changed, for AI line edits. */
  readonly lineEdits?: LineEditsApi;
  /** The cards cleared on this device. */
  readonly cleared?: DashboardCleared;
  /** Stops a chat's agent, as its header's Stop does. */
  readonly stopChat?: (chatId: string) => Promise<void>;
  /** Called once the panel has gone; the caller then unmounts it. */
  readonly onClose: () => void;
  readonly onOpenChat: (chatId: string) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const [closing, setClosing] = useState(false);
  // Shown as a modal at once, and closed before it leaves the page, so the dialog itself hands
  // focus back to the control that opened it.
  useLayoutEffect(() => {
    const node = dialog.current;
    if (!node) return;
    if (!node.open) showDialog(node);
    return () => node.close();
  }, []);
  // The panel settles out before it goes; where nothing animates (reduced motion, the app's motion
  // setting: dashboard.css) it goes at once.
  const close = () => {
    const panel = dialog.current?.querySelector(".dashboard-panel");
    const moving = panel ? getComputedStyle(panel).animationName !== "none" : false;
    if (moving) setClosing(true);
    else onClose();
  };
  const openChat = (chatId: string) => {
    onOpenChat(chatId);
    onClose();
  };
  const listed = useSyncExternalStore(app.subscribe, app.getSnapshot).chats ?? [];
  const organization = useSyncExternalStore(
    workspace?.subscribe ?? noSubscribe,
    workspace?.getSnapshot ?? emptyWorkspace,
    workspace?.getSnapshot ?? emptyWorkspace,
  ).data;
  const entries = new Map(organization?.chats.map((entry) => [entry.chat.id, entry]) ?? []);
  const projectNames = new Map(
    organization?.projects.map((project) => [project.id, project.name]) ?? [],
  );
  // Archived chats are put away; the board leaves them out as the sidebar does.
  const summaries = listed.filter((chat) => !entries.get(chat.id)?.archived);
  // A chat's place is its project's name: a worktree's folder is an app-owned name.
  const placeOf = (chat: ChatSummary) => {
    const project = entries.get(chat.id)?.project_id;
    return (project ? projectNames.get(project) : undefined) ?? projectName(chat.folder);
  };
  const [allFinished, setAllFinished] = useState(false);
  const [reading, setReading] = useState<Reading>({ previous: null, current: null });
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [stopping, setStopping] = useState<ReadonlySet<number>>(new Set());
  const [stoppingChats, setStoppingChats] = useState<ReadonlySet<string>>(new Set());
  useSyncExternalStore(cleared.subscribe, cleared.getVersion, cleared.getVersion);
  const now = useClock(true);
  const reduced = usePrefersReducedMotion();
  const day = new Date(now);
  const today = new Date(day.getFullYear(), day.getMonth(), day.getDate()).getTime();
  // Agents' names as their chats show them: the same map until a chat brings a new one, so AI
  // line edits does not draw again with every tick of the clock.
  const namesKey = JSON.stringify(
    [...new Map(listed.map((chat) => [chat.agent_id, chat.agent_name] as const))].sort(),
  );
  const agentNames = useMemo(
    () => new Map<string, string>(JSON.parse(namesKey) as [string, string][]),
    [namesKey],
  );

  const refresh = useCallback(async () => {
    try {
      const next = await api.snapshot();
      setReading((last) => ({ previous: last.current, current: next }));
      setError(null);
    } catch (failure) {
      setError(errorMessage(failure));
    }
  }, [api]);

  useEffect(() => {
    let alive = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      if (!alive) return;
      if (!document.hidden) await refresh();
      if (alive) timer = setTimeout(tick, REFRESH_MS);
    };
    void tick();
    return () => {
      alive = false;
      clearTimeout(timer);
    };
  }, [refresh]);

  const stop = async (chatId: string, pid: number, name: string) => {
    setStopping((current) => new Set(current).add(pid));
    setNotice(null);
    try {
      await api.stop(chatId, pid);
      setNotice(`Stopped ${name} (process ${pid}) and what it started.`);
      await refresh();
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setStopping((current) => {
        const next = new Set(current);
        next.delete(pid);
        return next;
      });
    }
  };

  const stopAgent = async (chat: ChatSummary) => {
    setStoppingChats((current) => new Set(current).add(chat.id));
    setNotice(null);
    try {
      await stopChat(chat.id);
      setNotice(`Stopped the agent in ${chat.title ?? "New chat"}.`);
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setStoppingChats((current) => {
        const next = new Set(current);
        next.delete(chat.id);
        return next;
      });
    }
  };

  // A chat open in its terminal here shows as stopped to the store; its terminal says whether its
  // agent works (draws output) or waits at its prompt, read again with every tick of the clock.
  // When each chat came into its lane keeps the lanes' order steady while their agents stream.
  const inTerminal = (chatId: string) => terminalChats.use(chatId, now);
  const arrivals = useRef<Arrivals>(new Map());
  const board = boardOf(summaries, now, inTerminal, arrivals.current, cleared.isCleared);
  const processes = new Map(
    (reading.current?.chats ?? []).map((chat) => [chat.chat_id, chat] as const),
  );

  return createPortal(
    // A click on the dialog itself is a click beside the panel: the dialog fills the window and
    // the panel is the only thing drawn in it.
    <dialog
      ref={dialog}
      className="g-root dashboard"
      data-testid="dashboard"
      data-closing={closing ? "" : undefined}
      aria-labelledby={title}
      onCancel={(event) => {
        event.preventDefault();
        close();
      }}
      // The browser may close it itself (an Escape it will not let the page hold back): then it is
      // simply gone. A close the panel made and undid at once (React's development remount) has
      // left it open by the time its event arrives, and changes nothing.
      onClose={() => {
        if (!dialog.current?.open) onClose();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape") event.stopPropagation();
      }}
      onClick={(event) => {
        if (event.target === event.currentTarget) close();
      }}
    >
      <div
        className="dashboard-panel"
        onAnimationEnd={(event) => {
          if (closing && event.target === event.currentTarget) onClose();
        }}
      >
        <header className="dashboard-bar">
          <div className="dashboard-head">
            <h2 id={title}>Dashboard</h2>
            <p data-testid="dashboard-summary">{boardSummary(board)}</p>
          </div>
          <DialogClose label="Close the Dashboard" onClick={close} />
        </header>
        {/*
         * Focus starts on the board, so the arrow keys scroll it; it is a stop of its own for Tab
         * too, so a board with nothing to press still scrolls from the keyboard.
         */}
        {/* biome-ignore lint/a11y/noNoninteractiveTabindex: a scrolling region must take focus. */}
        <div className="dashboard-page" tabIndex={0} data-autofocus="">
          {error !== null ? (
            <p role="alert" className="ui-error dashboard-message">
              {error}
            </p>
          ) : null}
          <p role="status" className="dashboard-message">
            {notice}
          </p>
          <div className="dashboard-board">
            {LANES.map((lane) => {
              const cards =
                lane === "finished" && !allFinished
                  ? board[lane].slice(0, FINISHED_SHOWN)
                  : board[lane];
              return (
                <section
                  key={lane}
                  className="dashboard-lane"
                  data-lane={lane}
                  aria-labelledby={`dashboard-lane-${lane}`}
                  data-testid="dashboard-lane"
                >
                  <div className="dashboard-lane-head">
                    <h3 id={`dashboard-lane-${lane}`} className="dashboard-lane-title">
                      <LaneMark lane={lane} busy={board[lane].length > 0} />
                      {LANE_TITLES[lane]}{" "}
                      <span className="dashboard-lane-count">{board[lane].length}</span>
                    </h3>
                    {lane === "finished" && board.finished.length > 0 ? (
                      <button
                        type="button"
                        className="g-button g-button-ghost g-button-28 dashboard-lane-clear"
                        data-testid="dashboard-clear-finished"
                        title="Take every finished chat off the Dashboard until it is used again"
                        onClick={() => cleared.clear(board.finished, listed)}
                      >
                        Clear finished
                      </button>
                    ) : null}
                  </div>
                  {board[lane].length === 0 ? (
                    <p className="dashboard-lane-empty">{EMPTY[lane]}</p>
                  ) : null}
                  <ul className="dashboard-cards">
                    <AnimatePresence initial={false} mode="popLayout">
                      {cards.map((chat) => (
                        <motion.li
                          key={chat.id}
                          className="dashboard-card"
                          data-lane={lane}
                          data-testid="dashboard-card"
                          data-test-chat-id={chat.id}
                          {...cardMotion(reduced)}
                        >
                          <DashboardCard
                            chat={chat}
                            lane={lane}
                            line={stateLine(chat, inTerminal, lane)}
                            place={placeOf(chat)}
                            processes={processes.get(chat.id)}
                            reading={reading}
                            now={now}
                            stopping={stopping}
                            onStop={stop}
                            onOpenChat={openChat}
                            holdsAgent={holdsAgent(chat, lane)}
                            stoppingAgent={stoppingChats.has(chat.id)}
                            onStopAgent={() => void stopAgent(chat)}
                            onClear={() => cleared.clear([chat], listed)}
                          />
                        </motion.li>
                      ))}
                    </AnimatePresence>
                  </ul>
                  {lane === "finished" && board.finished.length > FINISHED_SHOWN ? (
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-28 dashboard-lane-more"
                      aria-expanded={allFinished}
                      onClick={() => setAllFinished((all) => !all)}
                    >
                      {allFinished
                        ? "Show fewer"
                        : `Show ${board.finished.length - FINISHED_SHOWN} more`}
                    </button>
                  ) : null}
                </section>
              );
            })}
          </div>
          <LineEditsCard api={lineEdits} agentNames={agentNames} today={today} />
        </div>
      </div>
    </dialog>,
    document.body,
  );
}

/** Beside a lane's title: a volt dot, the working loader while agents work, or a check. */
function LaneMark({ lane, busy }: { readonly lane: Lane; readonly busy: boolean }) {
  return (
    <span className="dashboard-lane-mark" data-busy={busy ? "" : undefined} aria-hidden="true">
      {lane === "attention" ? (
        <StatusDot tone={busy ? "volt" : "neutral"} size={8} />
      ) : lane === "working" ? (
        busy ? (
          <PixelLoader size={12} tempo="tool" />
        ) : (
          <StatusDot tone="neutral" size={8} />
        )
      ) : (
        <Check size={14} />
      )}
    </span>
  );
}

function markState(chat: ChatSummary, lane: Lane) {
  if (lane === "attention") return chat.needs_attention ? "waiting" : "error";
  if (lane === "working") return "tool";
  return chat.status === "ready" ? "done" : "idle";
}

function DashboardCard({
  chat,
  lane,
  line,
  place,
  processes,
  reading,
  now,
  stopping,
  onStop,
  onOpenChat,
  holdsAgent,
  stoppingAgent,
  onStopAgent,
  onClear,
}: {
  readonly chat: ChatSummary;
  readonly lane: Lane;
  /** What the chat is doing, or why it needs you. */
  readonly line: string;
  /** Its project's name, or its folder's when it has none. */
  readonly place: string;
  readonly processes: ChatProcesses | undefined;
  readonly reading: Reading;
  readonly now: number;
  readonly stopping: ReadonlySet<number>;
  readonly onStop: (chatId: string, pid: number, name: string) => void;
  readonly onOpenChat: (chatId: string) => void;
  /** Its agent is live (working, or waiting for an answer): it offers Stop rather than Clear. */
  readonly holdsAgent: boolean;
  readonly stoppingAgent: boolean;
  readonly onStopAgent: () => void;
  readonly onClear: () => void;
}) {
  const title = chat.title ?? "New chat";
  const tasks = orderedTasks(processes?.tasks ?? []).filter((task) => task.status === "running");
  const rows = (processes?.agents ?? []).flatMap((agent) => listTree(agent.processes));
  const memory = rows.reduce((sum, { row }) => sum + (row.memory_bytes ?? 0), 0);
  return (
    <>
      <div className="dashboard-card-head">
        <AgentMark agent={chat.agent_id} size={28} state={markState(chat, lane)} label={line} />
        <div className="dashboard-card-text">
          <h4 className="dashboard-card-title">
            <button
              type="button"
              className="dashboard-card-open"
              title={`${title}\n${chat.folder}`}
              onClick={() => onOpenChat(chat.id)}
            >
              {title}
            </button>
          </h4>
          <p className="dashboard-card-meta">
            {chat.agent_name} · {place}
          </p>
        </div>
        <ChevronRight className="dashboard-card-go" size={14} aria-hidden="true" />
        {holdsAgent ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-28 dashboard-card-action"
            data-testid="dashboard-card-stop"
            aria-label={`Stop the agent in ${title}`}
            title="Stop this chat's agent"
            disabled={stoppingAgent}
            onClick={onStopAgent}
          >
            <Square size={12} aria-hidden="true" /> {stoppingAgent ? "Stopping…" : "Stop"}
          </button>
        ) : (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28 dashboard-card-action"
            data-testid="dashboard-card-clear"
            aria-label={`Clear ${title} from the Dashboard`}
            title="Take this card off the Dashboard until the chat is used again"
            onClick={onClear}
          >
            <X size={14} aria-hidden="true" />
          </button>
        )}
      </div>
      <p className="dashboard-card-state" data-testid="dashboard-card-state">
        <span>{line}</span>
        {lane === "working" ? null : (
          <span className="dashboard-card-time"> · {ago(chat.updated_at, now)}</span>
        )}
      </p>
      {tasks.length > 0 ? (
        <ul className="dashboard-tasks" aria-label="Running tasks">
          {tasks.slice(0, TASKS_SHOWN).map((task, order) => {
            const item = { task, steps: [], endedIn: null, order, placeholder: false };
            return (
              <li key={task.task_id} className="dashboard-task" data-testid="dashboard-task">
                <TaskKindIcon kind={task.kind} />
                <span className="dashboard-task-text">
                  <span className="dashboard-task-title">{task.title}</span>
                  {task.activity ? (
                    <span className="dashboard-task-activity">{task.activity}</span>
                  ) : null}
                </span>
                <span className="dashboard-task-facts">
                  {taskFacts(item, task.status, now, true).join(" · ")}
                </span>
                <TaskMark display={task.status} />
              </li>
            );
          })}
          {tasks.length > TASKS_SHOWN ? (
            <li className="dashboard-task-more">{tasks.length - TASKS_SHOWN} more running</li>
          ) : null}
        </ul>
      ) : null}
      {rows.length > 0 ? (
        <details className="dashboard-programs" data-testid="dashboard-programs">
          <summary>
            {rows.length} {rows.length === 1 ? "program" : "programs"}
            {memory > 0 ? (
              <span className="dashboard-programs-facts"> · {formatBytes(memory)}</span>
            ) : null}
          </summary>
          <ul className="dashboard-process-list" aria-label={`Programs of ${title}`}>
            {rows.map(({ row, depth, root }) => {
              const cpu = reading.current
                ? cpuPercent(reading.previous, reading.current, row.pid)
                : null;
              const facts = [
                root ? "Agent" : null,
                `PID ${row.pid}`,
                cpu === null ? null : `${cpu < 10 ? cpu.toFixed(1) : Math.round(cpu)}% CPU`,
                formatBytes(row.memory_bytes),
                formatUptime(row.started_ms, now),
              ].filter((part): part is string => part !== null);
              return (
                <li
                  key={row.pid}
                  className="dashboard-process"
                  data-depth={Math.min(depth, 4)}
                  data-testid="process-row"
                >
                  <div className="dashboard-process-text">
                    <span className="dashboard-process-name">{row.name}</span>
                    <span className="dashboard-process-facts">{facts.join(" · ")}</span>
                    {row.command ? (
                      <code className="dashboard-process-command" title={row.command}>
                        {row.command}
                      </code>
                    ) : null}
                  </div>
                  {root ? null : (
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-28"
                      aria-label={`Stop ${row.name}, process ${row.pid}`}
                      disabled={stopping.has(row.pid)}
                      onClick={() => onStop(chat.id, row.pid, row.name)}
                    >
                      <Square size={12} aria-hidden="true" /> Stop
                    </button>
                  )}
                </li>
              );
            })}
          </ul>
        </details>
      ) : null}
    </>
  );
}
