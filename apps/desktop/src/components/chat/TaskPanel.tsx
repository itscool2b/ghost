import { StatusDot } from "@ghost/ui/graphite";
import { ArrowLeft, X } from "lucide-react";
import { type CSSProperties, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import type { TaskStatus, TaskStep, ToolStatus } from "../../bindings";
import type { BlobState } from "../../lib/chat/chatStore";
import {
  childTasks,
  elapsedLabel,
  repeatsSummary,
  TASK_STATUS_LABEL,
  type TaskItem,
  taskDisplay,
  taskElapsed,
  taskTokensLabel,
  toolUsesLabel,
} from "../../lib/chat/tasks";
import type { ToolDisplayStatus } from "../../lib/chat/transcript";
import { Answer } from "../Markdown";
import { TaskKindIcon, TaskLine, TaskMark, taskFacts, useClock } from "./BackgroundTasks";
import { FullOutput, TOOL_ICON, ToolPhraseText } from "./ToolBlock";
import "./tasks.css";

/** How close to the end the list must be to keep following new steps. */
const FOLLOW_SLACK = 32;

const STEP_STATUS_LABEL: Record<ToolDisplayStatus, string> = {
  pending: "Pending",
  in_progress: "Running",
  completed: "Done",
  failed: "Failed",
  stopped: "Stopped",
};

/** A step still running shows as stopped once its task is no longer running. */
function stepDisplay(status: ToolStatus | null, task: TaskStatus): ToolDisplayStatus {
  const own = status ?? "completed";
  if ((own === "pending" || own === "in_progress") && task !== "running") return "stopped";
  return own;
}

/**
 * One background task beside the chat, in the Changes panel's place and layout: what it is and how
 * far it got, its steps as they arrive (following the newest while the reader is at the end), the
 * prompt it was given, its report once it ends, and the tasks it started. Escape closes it, and
 * focus returns to what opened it.
 */
export function TaskPanel({
  item,
  tasks,
  command = null,
  live,
  liveTasks,
  blobs,
  onLoadBlob,
  onOpen,
  onClose,
}: {
  readonly item: TaskItem;
  readonly tasks: ReadonlyMap<string, TaskItem>;
  /** The transcript's call that started it: a command shows the command line it runs. */
  readonly command?: string | null;
  readonly live: boolean;
  readonly liveTasks: ReadonlySet<string>;
  readonly blobs: ReadonlyMap<string, BlobState>;
  readonly onLoadBlob: (sha256: string) => void;
  /** Shows another task here: one this task started, or the one that started it. */
  readonly onOpen: (taskId: string) => void;
  readonly onClose: () => void;
}) {
  const titleId = useId();
  const closeButton = useRef<HTMLButtonElement>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const [width, setWidth] = useState(380);
  const { task } = item;
  const display = taskDisplay(item, live, liveTasks);
  const now = useClock(display === "running");
  const children = childTasks(tasks, task.task_id);
  const parent = task.parent_task_id !== null ? (tasks.get(task.parent_task_id) ?? null) : null;

  // Focus returns, on close, to what last opened a task here from outside: a pill, an Open.
  const panel = useRef<HTMLElement>(null);
  const returnTo = useRef<HTMLElement | null>(null);
  useEffect(
    () => () => {
      const target = returnTo.current;
      if (target?.isConnected) target.focus();
    },
    [],
  );
  // A new task here (a child, its parent) starts at its end with the focus on its close button.
  useEffect(() => {
    void task.task_id;
    // A click inside the panel can leave the focus on the page (WebKit): that is no opener.
    const active = document.activeElement;
    if (
      active instanceof HTMLElement &&
      active !== document.body &&
      !panel.current?.contains(active)
    )
      returnTo.current = active;
    following.current = true;
    closeButton.current?.focus();
  }, [task.task_id]);

  const summaryBlob = task.summary_truncated?.blob ?? null;
  const loadedSummary = summaryBlob ? blobs.get(summaryBlob) : undefined;
  useEffect(() => {
    // The full report is one page at most: load it as soon as the panel shows a cut one.
    if (summaryBlob && loadedSummary === undefined) onLoadBlob(summaryBlob);
  }, [summaryBlob, loadedSummary, onLoadBlob]);
  const summary = loadedSummary?.eof ? loadedSummary.text : task.summary;

  const steps =
    summary !== null && repeatsSummary(item.steps[item.steps.length - 1], summary)
      ? item.steps.slice(0, -1)
      : item.steps;
  useLayoutEffect(() => {
    void steps.length;
    void summary;
    const node = scroller.current;
    if (node && following.current) node.scrollTop = node.scrollHeight;
  }, [steps.length, summary]);

  // The header's facts: the agent type, the status, how long it ran, then its tokens and tools.
  const facts = [
    task.agent_type,
    TASK_STATUS_LABEL[display],
    elapsedLabel(taskElapsed(task, display, now)),
    taskTokensLabel(task.tokens),
    task.kind === "agent" ? toolUsesLabel(task.tool_uses) : null,
  ].filter((part): part is string => Boolean(part));

  return (
    <aside
      ref={panel}
      className="changes-panel task-panel"
      style={{ "--changes-width": `${width}px` } as CSSProperties}
      aria-labelledby={titleId}
      data-testid="task-panel"
      data-test-task-id={task.task_id}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
    >
      <hr
        className="changes-resizer"
        tabIndex={0}
        aria-label="Resize task panel"
        aria-orientation="vertical"
        aria-valuemin={320}
        aria-valuemax={560}
        aria-valuenow={width}
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
            event.preventDefault();
            setWidth((value) =>
              Math.max(320, Math.min(560, value + (event.key === "ArrowLeft" ? 20 : -20))),
            );
          }
        }}
        onPointerDown={(event) => event.currentTarget.setPointerCapture(event.pointerId)}
        onPointerMove={(event) => {
          if (event.currentTarget.hasPointerCapture(event.pointerId)) {
            const edge = event.currentTarget.parentElement?.getBoundingClientRect().right;
            if (edge) setWidth(Math.max(320, Math.min(560, edge - event.clientX)));
          }
        }}
        onPointerUp={(event) => event.currentTarget.releasePointerCapture(event.pointerId)}
      />
      <header className="changes-panel-header task-panel-header">
        {parent ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label={`Back to ${parent.task.title}`}
            title={`Back to ${parent.task.title}`}
            onClick={() => onOpen(parent.task.task_id)}
          >
            <ArrowLeft size={16} />
          </button>
        ) : (
          <span className="task-panel-icon">
            <TaskKindIcon kind={task.kind} size={16} />
          </span>
        )}
        <h2 id={titleId} className="task-panel-title" title={task.title}>
          {task.title}
        </h2>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Close task"
          ref={closeButton}
          onClick={onClose}
        >
          <X size={16} />
        </button>
      </header>
      <p className="task-panel-facts" data-status={display} data-testid="task-panel-facts">
        <TaskMark display={display} />
        <span>{facts.join(" · ")}</span>
      </p>
      {display === "running" && task.activity ? (
        <p className="task-panel-activity" title={task.activity}>
          {task.activity}
        </p>
      ) : null}
      <div
        ref={scroller}
        className="changes-panel-scroll task-panel-scroll"
        onScroll={(event) => {
          const node = event.currentTarget;
          following.current =
            node.scrollHeight - node.scrollTop - node.clientHeight <= FOLLOW_SLACK;
        }}
      >
        {command ? (
          <pre className="ui-pre task-panel-command" data-testid="task-command">
            {command}
          </pre>
        ) : null}
        {task.kind === "command" && display === "running" ? (
          <p className="changes-empty task-panel-note">
            Its output goes to the agent. The result shows here when it ends.
          </p>
        ) : null}
        {task.prompt ? (
          <details className="task-panel-prompt" data-testid="task-prompt">
            <summary>Prompt</summary>
            <div className="task-panel-prose">
              <Answer text={task.prompt} />
            </div>
          </details>
        ) : null}
        {steps.length ? (
          <ol className="task-steps" aria-label="Steps" data-testid="task-steps">
            {steps.map((step) => (
              <StepRow
                key={step.step_id}
                step={step}
                task={display}
                blobs={blobs}
                onLoadBlob={onLoadBlob}
              />
            ))}
          </ol>
        ) : display === "running" && task.kind === "agent" ? (
          <p className="changes-empty" role="status">
            Waiting for the first step…
          </p>
        ) : null}
        {children.length ? (
          <ul className="task-children" aria-label="Tasks it started">
            {children.map((child) => (
              <ChildRow
                key={child.task.task_id}
                item={child}
                display={taskDisplay(child, live, liveTasks)}
                now={now}
                onOpen={onOpen}
              />
            ))}
          </ul>
        ) : null}
        {summary ? (
          <section
            className="task-panel-summary"
            aria-label="Report"
            data-testid="task-summary"
            data-status={display}
          >
            <div className="task-panel-prose">
              <Answer text={summary} />
            </div>
            {task.summary_truncated && !loadedSummary?.eof ? (
              <FullOutput
                truncation={task.summary_truncated}
                blob={loadedSummary}
                onLoadBlob={onLoadBlob}
              />
            ) : null}
          </section>
        ) : null}
      </div>
    </aside>
  );
}

function StepRow({
  step,
  task,
  blobs,
  onLoadBlob,
}: {
  readonly step: TaskStep;
  readonly task: TaskStatus;
  readonly blobs: ReadonlyMap<string, BlobState>;
  readonly onLoadBlob: (sha256: string) => void;
}) {
  if (step.kind === "text") {
    const blob = step.truncated?.blob ? blobs.get(step.truncated.blob) : undefined;
    const text = blob?.eof ? blob.text : (step.text ?? "");
    return (
      <li className="task-step task-step-text" data-testid="task-step" data-test-kind="text">
        <div className="task-panel-prose">
          <Answer text={text} />
        </div>
        {step.truncated && !blob?.eof ? (
          <FullOutput truncation={step.truncated} blob={blob} onLoadBlob={onLoadBlob} />
        ) : null}
      </li>
    );
  }
  const display = stepDisplay(step.status, task);
  const kind = step.tool_kind ?? "other";
  const Icon = TOOL_ICON[kind];
  return (
    <li
      className="task-step task-step-tool"
      data-testid="task-step"
      data-test-kind="tool"
      data-test-status={display}
      title={step.title}
    >
      <span className="chat-gutter">
        {display === "failed" ? (
          <StatusDot tone="red" />
        ) : display === "stopped" ? (
          <StatusDot tone="neutral" />
        ) : (
          <Icon className="chat-tool-icon" size={14} strokeWidth={1.5} aria-hidden="true" />
        )}
      </span>
      <ToolPhraseText title={step.title || "Tool"} kind={kind} display={display} />
      <span className="chat-tool-status" data-status={display}>
        {STEP_STATUS_LABEL[display]}
      </span>
    </li>
  );
}

function ChildRow({
  item,
  display,
  now,
  onOpen,
}: {
  readonly item: TaskItem;
  readonly display: TaskStatus;
  readonly now: number;
  readonly onOpen: (taskId: string) => void;
}) {
  const facts = taskFacts(item, display, now);
  return (
    <li>
      <button
        type="button"
        className="task-child"
        data-status={display}
        onClick={() => onOpen(item.task.task_id)}
      >
        <TaskLine item={item} display={display} facts={facts} />
      </button>
    </li>
  );
}
