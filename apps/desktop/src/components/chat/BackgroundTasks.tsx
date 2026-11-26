import { PixelLoader } from "@ghost/ui/graphite";
import { Bot, Check, CircleStop, History, TerminalSquare, TriangleAlert } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { TaskKind, TaskStatus } from "../../bindings";
import {
  compactElapsed,
  elapsedLabel,
  endedAnnouncement,
  shownTasks,
  stripTasks,
  TASK_STATUS_LABEL,
  type TaskItem,
  taskDisplay,
  taskElapsed,
  toolUsesLabel,
} from "../../lib/chat/tasks";
import { closePopover, Popover } from "./Popover";
import "./tasks.css";

/** More tasks than this collapse into one "N background tasks" pill. */
const STRIP_PILLS = 3;

/** What a task runs: a subagent, or a command. */
export function TaskKindIcon({
  kind,
  size = 14,
}: {
  readonly kind: TaskKind;
  readonly size?: number;
}) {
  const Icon = kind === "agent" ? Bot : TerminalSquare;
  return <Icon className="chat-task-kind" size={size} strokeWidth={1.5} aria-hidden="true" />;
}

/** A task's state: the shared loader while it runs, then a check, a warning or a stop. */
export function TaskMark({ display }: { readonly display: TaskStatus }) {
  switch (display) {
    case "running":
      return <PixelLoader className="chat-task-mark" size={12} tempo="tool" />;
    case "completed":
      return (
        <Check className="chat-task-mark" data-status={display} size={14} aria-hidden="true" />
      );
    case "failed":
      return (
        <TriangleAlert
          className="chat-task-mark"
          data-status={display}
          size={14}
          aria-hidden="true"
        />
      );
    case "stopped":
      return (
        <CircleStop className="chat-task-mark" data-status={display} size={14} aria-hidden="true" />
      );
  }
}

/** The wall clock, ticking each second while `running`, so elapsed times move on their own. */
export function useClock(running: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [running]);
  return now;
}

/**
 * A task's facts as one line: its elapsed time, and its tool count when it is an agent. `compact`:
 * the strip's short time ("1m 13s").
 */
export function taskFacts(
  item: TaskItem,
  display: TaskStatus,
  now: number,
  compact = false,
): string[] {
  const elapsed = taskElapsed(item.task, display, now);
  return [
    compact ? compactElapsed(elapsed) : elapsedLabel(elapsed),
    item.task.kind === "agent" ? toolUsesLabel(item.task.tool_uses) : null,
  ].filter((part): part is string => part !== null);
}

/**
 * A task's line in a pill, a menu row or the panel's list of the tasks it started: the kind's glyph,
 * the title, what it does now, its facts and its mark. Its words are its button's name, in the
 * order they show (spaces between them, so the name reads them apart), and the agent type and the
 * status, which the glyph and the mark show, follow for screen readers.
 */
export function TaskLine({
  item,
  display,
  activity = null,
  facts,
}: {
  readonly item: TaskItem;
  readonly display: TaskStatus;
  readonly activity?: string | null;
  readonly facts: readonly string[];
}) {
  const unseen = [item.task.agent_type, TASK_STATUS_LABEL[display]].filter(Boolean).join(", ");
  return (
    <>
      <TaskKindIcon kind={item.task.kind} />
      <span className="chat-task-title">{item.task.title}</span>{" "}
      {activity ? (
        <>
          <span className="chat-task-activity">{activity}</span>{" "}
        </>
      ) : null}
      {facts.length ? (
        <>
          <span className="chat-task-facts">{facts.join(" · ")}</span>{" "}
        </>
      ) : null}
      <span className="sr-only">{unseen}</span>
      <TaskMark display={display} />
    </>
  );
}

function TaskPill({
  item,
  display,
  now,
  open,
  onOpen,
}: {
  readonly item: TaskItem;
  readonly display: TaskStatus;
  readonly now: number;
  readonly open: boolean;
  readonly onOpen: (taskId: string) => void;
}) {
  // An ended pill says only what ended and how: its facts wait in the panel, so the running pills
  // beside it keep their room.
  const facts = display === "running" ? taskFacts(item, display, now, true) : [];
  const activity = display === "running" ? item.task.activity : null;
  return (
    <li className="chat-task-slot">
      <button
        type="button"
        className="chat-task-pill"
        data-status={display}
        data-testid="task-pill"
        data-test-task-id={item.task.task_id}
        aria-expanded={open}
        title={item.task.title}
        onClick={(event) => {
          // WebKit does not focus a clicked button; the panel returns focus to it when it closes.
          event.currentTarget.focus();
          onOpen(item.task.task_id);
        }}
      >
        <TaskLine item={item} display={display} activity={activity} facts={facts} />
      </button>
    </li>
  );
}

/**
 * The background strip in the chat's dock: the agent's running subagents and background commands,
 * and those that ended in the newest turn, as compact pills that open the task panel. More than
 * three collapse into one pill that lists them all. It sits in the dock's column above the queue
 * and the composer, one row high, so the composer never moves while tasks report.
 */
export function BackgroundTasks({
  tasks,
  turnKey,
  live,
  liveTasks,
  openTaskId,
  onOpen,
}: {
  readonly tasks: ReadonlyMap<string, TaskItem>;
  /** The newest turn's key: an ended task stays listed until the next turn starts. */
  readonly turnKey: string | null;
  /** The session has a process. */
  readonly live: boolean;
  readonly liveTasks: ReadonlySet<string>;
  /** The task the panel shows. */
  readonly openTaskId: string | null;
  readonly onOpen: (taskId: string) => void;
}) {
  const listed = stripTasks(tasks, turnKey, live, liveTasks);
  const earlier = earlierTasks(tasks, listed);
  const displays = listed.map((item) => taskDisplay(item, live, liveTasks));
  const running = displays.some((display) => display === "running");
  const now = useClock(running);
  return (
    <>
      <TaskAnnouncer tasks={tasks} live={live} liveTasks={liveTasks} />
      {listed.length === 0 && earlier.length === 0 ? null : (
        <section className="chat-tasks" aria-label="Background tasks" data-testid="task-strip">
          {listed.length === 0 ? null : listed.length <= STRIP_PILLS ? (
            <ul className="chat-tasks-list">
              {listed.map((item, index) => (
                <TaskPill
                  key={item.task.task_id}
                  item={item}
                  display={displays[index] ?? "running"}
                  now={now}
                  open={openTaskId === item.task.task_id}
                  onOpen={onOpen}
                />
              ))}
            </ul>
          ) : (
            <TaskOverflow
              listed={listed}
              displays={displays}
              now={now}
              openTaskId={openTaskId}
              onOpen={onOpen}
            />
          )}
          {earlier.length > 0 ? (
            <EarlierTasks
              items={earlier}
              displays={earlier.map((item) => taskDisplay(item, live, liveTasks))}
              now={now}
              openTaskId={openTaskId}
              onOpen={onOpen}
            />
          ) : null}
        </section>
      )}
    </>
  );
}

/** The tasks that ended before the newest turn, newest first: the strip's last pill lists them. */
export function earlierTasks(
  tasks: ReadonlyMap<string, TaskItem>,
  listed: readonly TaskItem[],
): TaskItem[] {
  const shown = new Set(listed.map((item) => item.task.task_id));
  return shownTasks(tasks)
    .filter(
      (item) =>
        !shown.has(item.task.task_id) &&
        (item.task.parent_task_id === null || !tasks.has(item.task.parent_task_id)),
    )
    .reverse();
}

/**
 * "3 earlier": the tasks of earlier turns, so what an agent ran never vanishes when the next
 * message is sent. Its list is a solid menu; a row opens the task panel.
 */
function EarlierTasks({
  items,
  displays,
  now,
  openTaskId,
  onOpen,
}: {
  readonly items: readonly TaskItem[];
  readonly displays: readonly TaskStatus[];
  readonly now: number;
  readonly openTaskId: string | null;
  readonly onOpen: (taskId: string) => void;
}) {
  const words = `${items.length} earlier ${items.length === 1 ? "task" : "tasks"}`;
  return (
    <Popover
      label="Earlier tasks"
      triggerLabel={words}
      className="chat-task-overflow chat-task-earlier"
      trigger={
        <span className="chat-task-pill" data-testid="task-earlier">
          <History className="chat-task-kind" size={14} strokeWidth={1.5} aria-hidden="true" />
          <span className="chat-task-title">{items.length} earlier</span>
        </span>
      }
    >
      <ul className="chat-task-menu">
        {items.map((item, index) => {
          const display = displays[index] ?? item.task.status;
          return (
            <li key={item.task.task_id}>
              <button
                type="button"
                className="g-menu-item chat-task-menu-item"
                aria-current={openTaskId === item.task.task_id ? "true" : undefined}
                onClick={(event) => {
                  closePopover(event.currentTarget, true);
                  onOpen(item.task.task_id);
                }}
              >
                <TaskLine item={item} display={display} facts={taskFacts(item, display, now)} />
              </button>
            </li>
          );
        })}
      </ul>
    </Popover>
  );
}

/** More than three: one pill, "5 background tasks · 2 running", opening the list of all of them. */
function TaskOverflow({
  listed,
  displays,
  now,
  openTaskId,
  onOpen,
}: {
  readonly listed: readonly TaskItem[];
  readonly displays: readonly TaskStatus[];
  readonly now: number;
  readonly openTaskId: string | null;
  readonly onOpen: (taskId: string) => void;
}) {
  const running = displays.filter((display) => display === "running").length;
  const count = `${listed.length} background tasks`;
  return (
    <Popover
      label="Background tasks"
      // The summary's name is its words as they show.
      triggerLabel={running ? `${count} ${running} running` : count}
      className="chat-task-overflow"
      trigger={
        <span className="chat-task-pill" data-testid="task-overflow">
          <Bot className="chat-task-kind" size={14} strokeWidth={1.5} aria-hidden="true" />
          <span className="chat-task-title">{count}</span>{" "}
          {running ? <span className="chat-task-facts">{running} running</span> : null}
          <TaskMark
            display={running ? "running" : (displays[displays.length - 1] ?? "completed")}
          />
        </span>
      }
    >
      <ul className="chat-task-menu">
        {listed.map((item, index) => {
          const display = displays[index] ?? "running";
          const facts = taskFacts(item, display, now);
          return (
            <li key={item.task.task_id}>
              <button
                type="button"
                className="g-menu-item chat-task-menu-item"
                aria-current={openTaskId === item.task.task_id ? "true" : undefined}
                onClick={(event) => {
                  // The list's pill keeps the focus, for the panel to return it there.
                  closePopover(event.currentTarget, true);
                  onOpen(item.task.task_id);
                }}
              >
                <TaskLine item={item} display={display} facts={facts} />
              </button>
            </li>
          );
        })}
      </ul>
    </Popover>
  );
}

/**
 * Says once, politely, when a task this view saw running ends: "Map account switching code
 * finished". Progress is never announced, and neither is a task that had ended before it was seen.
 */
function TaskAnnouncer({
  tasks,
  live,
  liveTasks,
}: {
  readonly tasks: ReadonlyMap<string, TaskItem>;
  readonly live: boolean;
  readonly liveTasks: ReadonlySet<string>;
}) {
  const seenRunning = useRef(new Set<string>());
  const announced = useRef(new Set<string>());
  const [message, setMessage] = useState("");
  useEffect(() => {
    const ended: string[] = [];
    for (const item of shownTasks(tasks)) {
      const id = item.task.task_id;
      const display = taskDisplay(item, live, liveTasks);
      if (display === "running") {
        seenRunning.current.add(id);
        continue;
      }
      if (!seenRunning.current.has(id) || announced.current.has(id)) continue;
      announced.current.add(id);
      const words = endedAnnouncement(item.task, display);
      if (words) ended.push(words);
    }
    if (ended.length) setMessage(ended.join(". "));
  }, [tasks, live, liveTasks]);
  return (
    <p className="sr-only" role="status" data-testid="task-announcement">
      {message}
    </p>
  );
}
