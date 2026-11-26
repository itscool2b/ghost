import { PixelLoader, StatusDot } from "@ghost/ui/graphite";
import {
  ArrowUpRight,
  Bot,
  FileText,
  Link as LinkIcon,
  Pause,
  Play,
  Plus,
  Trash2,
  Workflow,
} from "lucide-react";
import { type CSSProperties, type DragEvent, useEffect, useId, useState } from "react";
import type {
  AgentEntry,
  ChatSummary,
  ContextFile,
  CoordinatorRoutine,
  CoordinatorSnapshot,
  CoordinatorTask,
  CoordinatorTaskStatus,
  CoordinatorWorker,
  ProjectSummary,
} from "../../bindings";
import {
  BRIEF_BYTES,
  CONTEXT_FILE_BYTES,
  type CoordinatorApi,
  DECISION_BYTES,
  everyWords,
  type ProjectCoordination,
  ROUTINE_PRESETS,
  sourceName,
  TASK_COLUMNS,
  taskName,
  useProjectCoordination,
} from "../../lib/coordinator";
import { errorMessage } from "../../lib/errors";
import { ago } from "../../lib/plugins/format";
import { Modal } from "../chat/Modal";
import { Select } from "../Select";
import "../dashboard/dashboard.css";
import "../settings/settings.css";
import "./project.css";

type Tab = "board" | "agents" | "context" | "decisions" | "routines";
const TABS: readonly { id: Tab; name: string }[] = [
  { id: "board", name: "Board" },
  { id: "agents", name: "Agents" },
  { id: "context", name: "Context" },
  { id: "decisions", name: "Decisions" },
  { id: "routines", name: "Routines" },
];

const bytesOf = (text: string) => new TextEncoder().encode(text).length;

/** A link as its site: "github.com" for a pull request there. */
function linkHost(link: string): string {
  try {
    return new URL(link).host || link;
  } catch {
    return link;
  }
}

/** What a chat is doing, in the page's words. */
function chatState(chat: ChatSummary | undefined): {
  word: string;
  tone: "work" | "you" | "bad" | "idle";
} {
  if (!chat) return { word: "Not open", tone: "idle" };
  if (chat.needs_attention) return { word: "Needs you", tone: "you" };
  if (chat.status === "running" || chat.status === "starting" || chat.status === "cancelling")
    return { word: "Working", tone: "work" };
  if (chat.status === "failed" || chat.status === "auth_required")
    return { word: "Stopped on an error", tone: "bad" };
  return { word: "Idle", tone: "idle" };
}

function StateMark({
  tone,
  label,
}: {
  readonly tone: "work" | "you" | "bad" | "idle";
  readonly label: string;
}) {
  if (tone === "work") return <PixelLoader size={12} tempo="tool" label={label} />;
  if (tone === "you") return <StatusDot tone="volt" label={label} />;
  if (tone === "bad") return <StatusDot tone="red" label={label} />;
  return null;
}

/**
 * A coordinated project (an experiment; owner, 19 November 2026): the page for one of the
 * sidebar's projects, in the chat pane's place. A coordinator chat plans the project's work and
 * hands it to worker chats; this page is where the person sees and steers it without reading every
 * chat: the board of tasks, the agents, the Context files every agent reads and writes, the
 * decision log, and routines. Everything on it can be changed by hand, and each change an agent
 * makes shows here as it lands.
 */
export function ProjectPage({
  api,
  project,
  chats,
  agents,
  onOpenChat,
}: {
  readonly api: CoordinatorApi;
  readonly project: ProjectSummary;
  /** The live chat list, for what each agent is doing now. */
  readonly chats: readonly ChatSummary[];
  readonly agents: readonly AgentEntry[] | null;
  readonly onOpenChat: (chatId: string) => void;
}) {
  const coordination = useProjectCoordination(api, project.id);
  const [tab, setTab] = useState<Tab>("board");
  const { snapshot, error } = coordination;
  const live = new Map(chats.map((chat) => [chat.id, chat]));
  const coordinator = snapshot?.coordinator_chat_id
    ? live.get(snapshot.coordinator_chat_id)
    : undefined;
  const working =
    snapshot?.workers.filter((worker) => chatState(live.get(worker.chat_id)).tone === "work")
      .length ?? 0;
  const open = snapshot?.tasks.filter((task) => task.status !== "done").length ?? 0;
  const summary = !snapshot
    ? ""
    : snapshot.paused
      ? (snapshot.pause_reason ?? "Paused. No worker starts and no routine runs until you resume.")
      : !snapshot.coordinator_chat_id
        ? "No coordinator yet."
        : [
            `Coordinator ${chatState(coordinator).word.toLowerCase()}`,
            working > 0 ? `${working} ${working === 1 ? "worker" : "workers"} working` : null,
            `${open} open ${open === 1 ? "task" : "tasks"}`,
          ]
            .filter(Boolean)
            .join(" · ");
  return (
    <div className="project-page" data-testid="project-page">
      <div className="project-scroll">
        <header className="project-head">
          <div className="project-head-text">
            <h1>{project.name}</h1>
            <p
              role="status"
              data-testid="project-summary"
              data-paused={snapshot?.paused ? "" : undefined}
            >
              {summary}
            </p>
          </div>
          {snapshot ? (
            <div className="project-head-actions">
              {snapshot.coordinator_chat_id ? (
                <button
                  type="button"
                  className="g-button g-button-28"
                  data-testid="project-open-coordinator"
                  onClick={() => {
                    if (snapshot.coordinator_chat_id) onOpenChat(snapshot.coordinator_chat_id);
                  }}
                >
                  <Workflow size={14} aria-hidden="true" /> Open coordinator
                </button>
              ) : null}
              <button
                type="button"
                className="g-button g-button-ghost g-button-28"
                data-testid="project-pause"
                disabled={coordination.busy}
                title={
                  snapshot.paused
                    ? "Let workers start, routines run and reports reach the coordinator again"
                    : "Stop new workers, routines and reports. Agents already working finish their turn."
                }
                onClick={() => void coordination.apply({ type: "pause", paused: !snapshot.paused })}
              >
                {snapshot.paused ? (
                  <>
                    <Play size={14} aria-hidden="true" /> Resume
                  </>
                ) : (
                  <>
                    <Pause size={14} aria-hidden="true" /> Pause
                  </>
                )}
              </button>
            </div>
          ) : null}
        </header>
        {error ? (
          <p role="alert" className="ui-error project-error" data-testid="project-error">
            {error}
          </p>
        ) : null}
        {snapshot && !snapshot.coordinator_chat_id ? (
          <StartCoordinator project={project} agents={agents} coordination={coordination} />
        ) : null}
        {snapshot ? (
          <>
            <fieldset className="g-segmented project-tabs" aria-label="Show">
              {TABS.map((choice) => (
                <button
                  key={choice.id}
                  type="button"
                  className="g-segment"
                  aria-pressed={tab === choice.id}
                  data-testid={`project-tab-${choice.id}`}
                  onClick={() => setTab(choice.id)}
                >
                  {choice.name}
                </button>
              ))}
            </fieldset>
            {tab === "board" ? <Board snapshot={snapshot} coordination={coordination} /> : null}
            {tab === "agents" ? (
              <Agents
                snapshot={snapshot}
                coordination={coordination}
                live={live}
                onOpenChat={onOpenChat}
              />
            ) : null}
            {tab === "context" ? (
              <Context api={api} snapshot={snapshot} coordination={coordination} />
            ) : null}
            {tab === "decisions" ? (
              <Decisions snapshot={snapshot} coordination={coordination} />
            ) : null}
            {tab === "routines" ? (
              <Routines snapshot={snapshot} coordination={coordination} />
            ) : null}
          </>
        ) : error ? null : (
          <p className="project-loading">Reading the project…</p>
        )}
      </div>
    </div>
  );
}

function StartCoordinator({
  project,
  agents,
  coordination,
}: {
  readonly project: ProjectSummary;
  readonly agents: readonly AgentEntry[] | null;
  readonly coordination: ProjectCoordination;
}) {
  const folders = project.folders.filter((folder) => !folder.worktree_of);
  const ready = (agents ?? []).filter((agent) => agent.availability.state === "ready");
  const [brief, setBrief] = useState("");
  const [agent, setAgent] = useState(
    () => ready.find((entry) => entry.id === project.default_agent)?.id ?? ready[0]?.id ?? "",
  );
  const [folder, setFolder] = useState(() => folders[0]?.id ?? "");
  const size = useId();
  const bytes = bytesOf(brief);
  const chosen = ready.some((entry) => entry.id === agent) ? agent : (ready[0]?.id ?? "");
  return (
    <section
      className="project-start"
      aria-label="Start the coordinator"
      data-testid="project-start"
    >
      <div className="settings-sheet">
        <div className="settings-sheet-head">
          <strong>What is this project for?</strong>
          <span
            id={size}
            className="settings-meter"
            data-over={bytes > BRIEF_BYTES ? "" : undefined}
          >
            {bytes.toLocaleString()} of {BRIEF_BYTES.toLocaleString()} bytes
          </span>
        </div>
        <textarea
          className="g-textarea settings-sheet-text project-brief"
          aria-label="Brief"
          aria-describedby={size}
          rows={6}
          value={brief}
          disabled={coordination.busy}
          placeholder="The goal, what done looks like, and anything the agents must not change. A coordinator plans it, hands the work to worker chats and reports back to you."
          onChange={(event) => setBrief(event.currentTarget.value)}
        />
        <div className="settings-sheet-foot">
          <label className="project-inline">
            <span>Agent</span>
            <Select
              className="g-select project-select"
              aria-label="Coordinator's agent"
              value={chosen}
              disabled={coordination.busy || ready.length === 0}
              onChange={(event) => setAgent(event.currentTarget.value)}
            >
              {ready.map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.name}
                </option>
              ))}
            </Select>
          </label>
          {folders.length > 1 ? (
            <label className="project-inline">
              <span>Folder</span>
              <Select
                className="g-select project-select"
                aria-label="Coordinator's folder"
                value={folder}
                disabled={coordination.busy}
                onChange={(event) => setFolder(event.currentTarget.value)}
              >
                {folders.map((entry) => (
                  <option key={entry.id} value={entry.id}>
                    {entry.display_name}
                  </option>
                ))}
              </Select>
            </label>
          ) : null}
          <span className="project-foot-space" />
          <button
            type="button"
            className="g-button g-button-primary g-button-28"
            data-testid="project-start-button"
            disabled={
              coordination.busy || !brief.trim() || bytes > BRIEF_BYTES || !chosen || !folder
            }
            onClick={() =>
              void coordination.start({ folder_id: folder, agent_id: chosen, brief: brief.trim() })
            }
          >
            Start coordinator
          </button>
        </div>
      </div>
      {ready.length === 0 ? (
        <p className="project-note">
          No agent is ready on this computer. Set one up in Settings &gt; Agents.
        </p>
      ) : (
        <p className="project-note">
          The coordinator does not edit files itself. It starts worker chats, each in its own Git
          worktree where the folder is a repository, and they run with the access level your new
          chats get.
        </p>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------------- board

function Board({
  snapshot,
  coordination,
}: {
  readonly snapshot: CoordinatorSnapshot;
  readonly coordination: ProjectCoordination;
}) {
  const [editing, setEditing] = useState<CoordinatorTask | "new" | null>(null);
  const [dragged, setDragged] = useState<number | null>(null);
  const [over, setOver] = useState<CoordinatorTaskStatus | null>(null);
  const drop = (event: DragEvent, status: CoordinatorTaskStatus, before: number | null) => {
    event.preventDefault();
    event.stopPropagation();
    setOver(null);
    if (dragged === null || dragged === before) return;
    void coordination.apply({ type: "task_move", number: dragged, status, before });
    setDragged(null);
  };
  return (
    <>
      <div className="project-board" data-testid="project-board">
        {TASK_COLUMNS.map((column) => {
          const tasks = snapshot.tasks.filter((task) => task.status === column.status);
          return (
            <section
              key={column.status}
              className="dashboard-lane project-lane"
              data-over={over === column.status ? "" : undefined}
              aria-labelledby={`project-lane-${column.status}`}
              data-testid="project-lane"
              data-test-status={column.status}
              onDragOver={(event) => {
                if (dragged === null) return;
                event.preventDefault();
                setOver(column.status);
              }}
              onDragLeave={() => setOver((current) => (current === column.status ? null : current))}
              onDrop={(event) => drop(event, column.status, null)}
            >
              <div className="dashboard-lane-head">
                <h2 id={`project-lane-${column.status}`} className="dashboard-lane-title">
                  {column.name} <span className="dashboard-lane-count">{tasks.length}</span>
                </h2>
                {column.status === "todo" ? (
                  <button
                    type="button"
                    className="g-button g-button-ghost g-button-icon g-button-24"
                    aria-label="Add a task"
                    title="Add a task"
                    data-testid="project-task-add"
                    onClick={() => setEditing("new")}
                  >
                    <Plus size={14} />
                  </button>
                ) : null}
              </div>
              {tasks.length === 0 ? (
                <p className="dashboard-lane-empty">
                  {column.status === "todo" ? "Nothing waiting." : "Nothing here."}
                </p>
              ) : null}
              <ul className="dashboard-cards">
                {tasks.map((task) => (
                  <li
                    key={task.number}
                    className="dashboard-card project-task"
                    data-testid="project-task"
                    data-test-task={task.number}
                    draggable
                    onDragStart={(event) => {
                      event.dataTransfer.effectAllowed = "move";
                      event.dataTransfer.setData("text/plain", taskName(task));
                      setDragged(task.number);
                    }}
                    onDragEnd={() => {
                      setDragged(null);
                      setOver(null);
                    }}
                    onDragOver={(event) => {
                      if (dragged !== null) event.preventDefault();
                    }}
                    onDrop={(event) => drop(event, column.status, task.number)}
                  >
                    <button
                      type="button"
                      className="project-task-open"
                      onClick={() => setEditing(task)}
                    >
                      <span className="project-task-number">{taskName(task)}</span>
                      <span className="project-task-title">{task.title}</span>
                    </button>
                    {task.worker || task.tags.length > 0 || task.link ? (
                      <p className="project-task-meta">
                        {task.worker ? (
                          <span className="project-task-worker">
                            <Bot size={12} aria-hidden="true" /> {task.worker}
                          </span>
                        ) : null}
                        {task.tags.map((tag) => (
                          <span key={tag} className="project-chip">
                            {tag}
                          </span>
                        ))}
                        {task.link ? (
                          <a
                            className="project-task-link"
                            href={task.link}
                            target="_blank"
                            rel="noreferrer noopener"
                            title={task.link}
                          >
                            <LinkIcon size={12} aria-hidden="true" /> {linkHost(task.link)}
                          </a>
                        ) : null}
                      </p>
                    ) : null}
                  </li>
                ))}
              </ul>
            </section>
          );
        })}
      </div>
      {editing ? (
        <TaskDialog
          task={editing === "new" ? null : editing}
          coordination={coordination}
          onClose={() => setEditing(null)}
        />
      ) : null}
    </>
  );
}

function TaskDialog({
  task,
  coordination,
  onClose,
}: {
  readonly task: CoordinatorTask | null;
  readonly coordination: ProjectCoordination;
  readonly onClose: () => void;
}) {
  const [title, setTitle] = useState(task?.title ?? "");
  const [body, setBody] = useState(task?.body ?? "");
  const [status, setStatus] = useState<CoordinatorTaskStatus>(task?.status ?? "todo");
  const [tags, setTags] = useState(task?.tags.join(", ") ?? "");
  const [link, setLink] = useState(task?.link ?? "");
  const [failure, setFailure] = useState<string | null>(null);
  const save = async () => {
    setFailure(null);
    const done = await coordination.apply({
      type: "task_save",
      number: task?.number ?? null,
      title: title.trim(),
      body,
      status,
      tags: tags
        .split(",")
        .map((tag) => tag.trim())
        .filter(Boolean),
      link: link.trim(),
    });
    if (done) onClose();
    else setFailure("The task was not saved. The reason is on the page.");
  };
  return (
    <Modal title={task ? `${taskName(task)}` : "New task"} size="md" onClose={onClose}>
      <form
        className="project-task-form"
        data-testid="project-task-form"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <label className="ui-field">
          <span>Title</span>
          <input
            className="g-input"
            aria-label="Title"
            value={title}
            maxLength={200}
            // biome-ignore lint/a11y/noAutofocus: a dialog opened to name a task starts in its name.
            autoFocus={!task}
            onChange={(event) => setTitle(event.currentTarget.value)}
          />
        </label>
        <label className="ui-field">
          <span>Notes</span>
          <textarea
            className="g-textarea"
            aria-label="Notes"
            rows={6}
            value={body}
            placeholder="What done means, and anything the task needs."
            onChange={(event) => setBody(event.currentTarget.value)}
          />
        </label>
        <div className="project-task-fields">
          <label className="ui-field">
            <span>Column</span>
            <Select
              className="g-select"
              aria-label="Column"
              value={status}
              onChange={(event) => setStatus(event.currentTarget.value as CoordinatorTaskStatus)}
            >
              {TASK_COLUMNS.map((column) => (
                <option key={column.status} value={column.status}>
                  {column.name}
                </option>
              ))}
            </Select>
          </label>
          <label className="ui-field">
            <span>Tags</span>
            <input
              className="g-input"
              aria-label="Tags"
              value={tags}
              placeholder="api, urgent"
              onChange={(event) => setTags(event.currentTarget.value)}
            />
          </label>
        </div>
        <label className="ui-field">
          <span>Link</span>
          <input
            className="g-input"
            aria-label="Link"
            value={link}
            placeholder="https://… the pull request, or anything that shows the work"
            onChange={(event) => setLink(event.currentTarget.value)}
          />
        </label>
        {task ? (
          <p className="project-note">
            {task.worker ? `Assigned to ${task.worker}. ` : ""}Last changed by{" "}
            {sourceName(task.source).toLowerCase()} {ago(task.updated_at)}.
          </p>
        ) : null}
        {failure ? (
          <p role="alert" className="ui-error">
            {coordination.error ?? failure}
          </p>
        ) : null}
        <div className="g-dialog-actions project-task-actions">
          {task ? (
            <button
              type="button"
              className="g-button g-button-ghost project-danger"
              disabled={coordination.busy}
              onClick={async () => {
                if (await coordination.apply({ type: "task_delete", number: task.number }))
                  onClose();
              }}
            >
              <Trash2 size={14} aria-hidden="true" /> Delete
            </button>
          ) : null}
          <span className="project-foot-space" />
          <button type="button" className="g-button" onClick={onClose}>
            Cancel
          </button>
          <button
            type="submit"
            className="g-button g-button-primary"
            data-testid="project-task-save"
            disabled={coordination.busy || !title.trim()}
          >
            {task ? "Save" : "Add task"}
          </button>
        </div>
      </form>
    </Modal>
  );
}

// ---------------------------------------------------------------------------------- agents

function Agents({
  snapshot,
  coordination,
  live,
  onOpenChat,
}: {
  readonly snapshot: CoordinatorSnapshot;
  readonly coordination: ProjectCoordination;
  readonly live: ReadonlyMap<string, ChatSummary>;
  readonly onOpenChat: (chatId: string) => void;
}) {
  const [dismissed, setDismissed] = useState(false);
  const shown = snapshot.workers.filter((worker) => dismissed || !worker.closed);
  const hidden = snapshot.workers.filter((worker) => worker.closed).length;
  const row = (
    key: string,
    name: string,
    chatId: string,
    detail: string | null,
    worker: CoordinatorWorker | null,
  ) => {
    const state = chatState(live.get(chatId));
    return (
      <li key={key} className="project-agent" data-testid="project-agent" data-test-name={name}>
        <span className="settings-tile" aria-hidden="true">
          {worker ? <Bot size={16} /> : <Workflow size={16} />}
        </span>
        <div className="project-agent-text">
          <strong>{name}</strong>
          <span className="project-agent-state" data-tone={state.tone}>
            <StateMark tone={state.tone} label={state.word} /> {state.word}
            {live.get(chatId) ? ` · ${live.get(chatId)?.agent_name}` : ""}
            {detail ? ` · ${detail}` : ""}
          </span>
          {worker?.report ? (
            <details className="project-report">
              <summary>
                Last report{worker.reported_at ? `, ${ago(worker.reported_at)}` : ""}
                {worker.pending ? " (not yet passed to the coordinator)" : ""}
              </summary>
              <p>{worker.report}</p>
            </details>
          ) : null}
        </div>
        <div className="project-agent-actions">
          {worker && !worker.closed ? (
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              title="Take this worker off the list. Its chat stays."
              disabled={coordination.busy}
              onClick={() => void coordination.apply({ type: "worker_close", chat_id: chatId })}
            >
              Dismiss
            </button>
          ) : null}
          <button
            type="button"
            className="g-button g-button-28"
            aria-label={`Open ${name}'s chat`}
            onClick={() => onOpenChat(chatId)}
          >
            Open <ArrowUpRight size={14} aria-hidden="true" />
          </button>
        </div>
      </li>
    );
  };
  return (
    <section className="project-section" aria-label="Agents">
      <p className="project-lead">
        The coordinator plans and delegates. Each worker is a chat of its own with a fresh context,
        which reports back when its turn ends.
      </p>
      {!snapshot.coordinator_chat_id && snapshot.workers.length === 0 ? (
        <p className="settings-empty">
          <strong>No agents yet</strong>Start the coordinator above and its workers show here.
        </p>
      ) : (
        <ul className="project-agents">
          {snapshot.coordinator_chat_id
            ? row("coordinator", "Coordinator", snapshot.coordinator_chat_id, null, null)
            : null}
          {shown.map((worker) =>
            row(
              worker.chat_id,
              worker.name,
              worker.chat_id,
              [
                worker.task ? `T-${worker.task}` : null,
                `${worker.turns} ${worker.turns === 1 ? "turn" : "turns"} finished`,
              ]
                .filter(Boolean)
                .join(" · "),
              worker,
            ),
          )}
        </ul>
      )}
      {hidden > 0 ? (
        <button
          type="button"
          className="g-button g-button-ghost g-button-28 project-more"
          onClick={() => setDismissed((value) => !value)}
        >
          {dismissed ? "Hide dismissed workers" : `Show ${hidden} dismissed`}
        </button>
      ) : null}
    </section>
  );
}

// ---------------------------------------------------------------------------------- context

function Context({
  api,
  snapshot,
  coordination,
}: {
  readonly api: CoordinatorApi;
  readonly snapshot: CoordinatorSnapshot;
  readonly coordination: ProjectCoordination;
}) {
  const [path, setPath] = useState<string | null>(snapshot.files[0]?.path ?? null);
  const [file, setFile] = useState<ContextFile | null>(null);
  const [text, setText] = useState("");
  const [naming, setNaming] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [viewing, setViewing] = useState<string | null>(null);
  // Counts the times the person asked for the file as it reads now, over their own edit.
  const [reload, setReload] = useState(0);
  const size = useId();
  const listed = snapshot.files.find((entry) => entry.path === path);
  const revision = listed?.revision ?? null;
  const dirty = file !== null && viewing === null && text !== file.content;
  // The file is read again when its revision changes (an agent wrote it), unless an edit is open.
  // biome-ignore lint/correctness/useExhaustiveDependencies: an open edit is never replaced under the person.
  useEffect(() => {
    if (path === null || revision === null) {
      setFile(null);
      setText("");
      return;
    }
    if (dirty) return;
    let live = true;
    api.file(snapshot.project_id, path, viewing).then(
      (read) => {
        if (!live) return;
        setFile(read);
        setText(read.content);
        setFailure(null);
      },
      (error) => {
        if (live) setFailure(errorMessage(error));
      },
    );
    return () => {
      live = false;
    };
  }, [api, snapshot.project_id, path, revision, viewing, reload]);
  const bytes = bytesOf(text);
  const stale =
    file !== null && viewing === null && revision !== null && file.revision !== revision;
  const create = async () => {
    const name = (naming ?? "").trim();
    if (!name) return;
    const full = name.endsWith(".md") ? name : `${name}.md`;
    if (
      await coordination.apply({
        type: "file_save",
        path: full,
        content: `# ${full.replace(/\.md$/u, "").split("/").pop()}\n`,
        expected_revision: null,
      })
    ) {
      setNaming(null);
      setViewing(null);
      setPath(full);
    }
  };
  return (
    <section className="project-section project-context" aria-label="Context">
      <p className="project-lead">
        What every agent in this project is shown before it works, and adds to as it learns: how to
        build and test, how the code is laid out, how you like things done. Each write keeps the
        text it replaced.
      </p>
      <div className="project-context-grid">
        <div className="project-files">
          {snapshot.files.length === 0 ? (
            <p className="settings-empty">
              <strong>No Context files yet</strong>Agents add them as they learn, or add one
              yourself.
            </p>
          ) : (
            <ul className="project-file-list" aria-label="Context files">
              {snapshot.files.map((entry) => (
                <li key={entry.path}>
                  <button
                    type="button"
                    className="project-file"
                    aria-current={entry.path === path}
                    data-testid="project-file"
                    disabled={dirty && entry.path !== path}
                    title={
                      dirty && entry.path !== path ? "Save or discard your edit first" : entry.path
                    }
                    onClick={() => {
                      setViewing(null);
                      setPath(entry.path);
                    }}
                  >
                    <FileText size={14} aria-hidden="true" />
                    <span className="project-file-text">
                      <span className="project-file-name">{entry.path}</span>
                      <small>
                        {sourceName(entry.source)} · {ago(entry.updated_at)}
                      </small>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
          {naming === null ? (
            <button
              type="button"
              className="g-button g-button-ghost g-button-28 project-more"
              data-testid="project-file-new"
              disabled={dirty}
              onClick={() => setNaming("")}
            >
              <Plus size={14} aria-hidden="true" /> New file
            </button>
          ) : (
            <form
              className="project-new-file"
              onSubmit={(event) => {
                event.preventDefault();
                void create();
              }}
            >
              <input
                className="g-input"
                aria-label="New file's name"
                placeholder="notes.md"
                value={naming}
                // biome-ignore lint/a11y/noAutofocus: the field opened to be typed in.
                autoFocus
                onChange={(event) => setNaming(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape") setNaming(null);
                }}
              />
              <button
                type="submit"
                className="g-button g-button-28"
                disabled={!naming.trim() || coordination.busy}
              >
                Add
              </button>
            </form>
          )}
        </div>
        {file && path ? (
          <div className="settings-sheet project-sheet" data-testid="project-file-sheet">
            <div className="settings-sheet-head">
              <strong>{file.path}</strong>
              <span
                id={size}
                className="settings-meter"
                data-over={bytes > CONTEXT_FILE_BYTES ? "" : undefined}
                style={
                  {
                    "--settings-fill": `${Math.min(100, Math.round((bytes / CONTEXT_FILE_BYTES) * 100))}%`,
                  } as CSSProperties
                }
              >
                {bytes.toLocaleString()} of {CONTEXT_FILE_BYTES.toLocaleString()} bytes
                <i aria-hidden="true" />
              </span>
            </div>
            <textarea
              className="g-textarea settings-sheet-text project-file-text-area"
              aria-label={file.path}
              aria-describedby={size}
              spellCheck={false}
              rows={18}
              value={text}
              readOnly={viewing !== null}
              disabled={coordination.busy}
              onChange={(event) => setText(event.currentTarget.value)}
            />
            <div className="settings-sheet-foot">
              <span className="project-sheet-status" role="status">
                {failure ??
                  (viewing !== null
                    ? `An earlier version, by ${sourceName(file.source).toLowerCase()}.`
                    : stale
                      ? "This file changed while you were editing. Saving is refused until you take the newer text."
                      : dirty
                        ? "Not saved yet."
                        : `Last written by ${sourceName(file.source).toLowerCase()} ${ago(file.updated_at)}.`)}
              </span>
              {file.history.length > 1 || viewing !== null ? (
                <Select
                  className="g-select project-select"
                  aria-label="Version"
                  value={viewing ?? ""}
                  disabled={dirty || coordination.busy}
                  onChange={(event) => setViewing(event.currentTarget.value || null)}
                >
                  <option value="">Current version</option>
                  {file.history
                    .filter((entry) => entry.revision !== revision && entry.bytes > 0)
                    .map((entry) => (
                      <option key={entry.revision} value={entry.revision}>
                        {sourceName(entry.source)}, {ago(entry.created_at)}
                      </option>
                    ))}
                </Select>
              ) : null}
              {viewing !== null ? (
                <button
                  type="button"
                  className="g-button g-button-28"
                  disabled={coordination.busy}
                  onClick={async () => {
                    if (await coordination.apply({ type: "file_restore", path, revision: viewing }))
                      setViewing(null);
                  }}
                >
                  Restore this version
                </button>
              ) : (
                <>
                  <button
                    type="button"
                    className="g-button g-button-ghost g-button-28 project-danger"
                    disabled={coordination.busy || dirty}
                    title="Delete this file. Its last versions are kept."
                    onClick={async () => {
                      if (
                        await coordination.apply({
                          type: "file_delete",
                          path,
                          expected_revision: file.revision,
                        })
                      ) {
                        setFile(null);
                        setPath(snapshot.files.find((entry) => entry.path !== path)?.path ?? null);
                      }
                    }}
                  >
                    Delete
                  </button>
                  <button
                    type="button"
                    className="g-button g-button-ghost g-button-28"
                    disabled={coordination.busy || (!dirty && !stale)}
                    onClick={() => {
                      // Drops the edit, then reads the file as it is now.
                      setText(file.content);
                      setReload((count) => count + 1);
                    }}
                  >
                    {stale ? "Take the newer text" : "Discard"}
                  </button>
                  <button
                    type="button"
                    className="g-button g-button-primary g-button-28"
                    data-testid="project-file-save"
                    disabled={coordination.busy || !dirty || bytes > CONTEXT_FILE_BYTES}
                    onClick={async () => {
                      if (
                        await coordination.apply({
                          type: "file_save",
                          path,
                          content: text,
                          expected_revision: file.revision,
                        })
                      )
                        setFile({ ...file, content: text });
                    }}
                  >
                    Save
                  </button>
                </>
              )}
            </div>
          </div>
        ) : null}
      </div>
    </section>
  );
}

// ---------------------------------------------------------------------------------- decisions

function Decisions({
  snapshot,
  coordination,
}: {
  readonly snapshot: CoordinatorSnapshot;
  readonly coordination: ProjectCoordination;
}) {
  const [text, setText] = useState("");
  const bytes = bytesOf(text);
  return (
    <section className="project-section" aria-label="Decisions">
      <p className="project-lead">
        What has been settled. Every agent is shown this log before it works, yours apart from the
        coordinator's, and entries are only ever added, so a decision you made once is not lost when
        an agent rewrites its notes. Workers cannot write here.
      </p>
      <form
        className="project-decision-form"
        onSubmit={async (event) => {
          event.preventDefault();
          if (await coordination.apply({ type: "decision_add", text: text.trim() })) setText("");
        }}
      >
        <textarea
          className="g-textarea"
          aria-label="A decision"
          rows={2}
          value={text}
          placeholder="What was decided, why, and what was turned down. For example: TypeScript, not Python, since the rest of the code is."
          onChange={(event) => setText(event.currentTarget.value)}
        />
        <button
          type="submit"
          className="g-button g-button-primary g-button-28"
          data-testid="project-decision-add"
          disabled={coordination.busy || !text.trim() || bytes > DECISION_BYTES}
        >
          Record
        </button>
      </form>
      {snapshot.decisions.length === 0 ? (
        <p className="settings-empty">
          <strong>Nothing decided yet</strong>The coordinator records what you approve; you can
          record a decision here too.
        </p>
      ) : (
        <ol className="project-decisions" data-testid="project-decisions">
          {snapshot.decisions.map((decision) => (
            <li
              key={decision.number}
              className="project-decision"
              data-testid="project-decision"
              data-retracted={decision.retracted ? "" : undefined}
            >
              <span className="project-task-number">D-{decision.number}</span>
              <div>
                <p>{decision.text}</p>
                <small>
                  {sourceName(decision.source)} · {ago(decision.created_at)}
                  {decision.retracted ? " · taken back, agents are not shown it" : ""}
                </small>
              </div>
              {decision.retracted ? (
                <span />
              ) : (
                <button
                  type="button"
                  className="g-button g-button-ghost g-button-28"
                  title="Keep it in the log, struck out, and stop showing it to agents"
                  aria-label={`Take back D-${decision.number}`}
                  disabled={coordination.busy}
                  onClick={() =>
                    void coordination.apply({ type: "decision_retract", number: decision.number })
                  }
                >
                  Take back
                </button>
              )}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------------- routines

function Routines({
  snapshot,
  coordination,
}: {
  readonly snapshot: CoordinatorSnapshot;
  readonly coordination: ProjectCoordination;
}) {
  const [editing, setEditing] = useState<CoordinatorRoutine | "new" | null>(null);
  return (
    <section className="project-section" aria-label="Routines">
      <div className="project-section-head">
        <p className="project-lead">
          A prompt the coordinator is sent again and again, for work that never ends: watching pull
          requests, a daily tidy-up. Routines run only while Ghost is open.
        </p>
        <button
          type="button"
          className="g-button g-button-28"
          data-testid="project-routine-add"
          onClick={() => setEditing("new")}
        >
          <Plus size={14} aria-hidden="true" /> Add routine
        </button>
      </div>
      {snapshot.routines.length === 0 ? (
        <p className="settings-empty">
          <strong>No routines</strong>Ask the coordinator to keep watching something, or add one
          here.
        </p>
      ) : (
        <ul className="project-routines">
          {snapshot.routines.map((routine) => (
            <li key={routine.id} className="project-routine" data-testid="project-routine">
              <div className="project-agent-text">
                <strong>{routine.title}</strong>
                <span className="project-agent-state">
                  {everyWords(routine.every_minutes)}
                  {routine.enabled
                    ? snapshot.paused
                      ? " · waits while the project is paused"
                      : ` · next ${ago(routine.next_at)}`
                    : " · off"}
                  {routine.last_at ? ` · last ran ${ago(routine.last_at)}` : ""}
                </span>
                <p className="project-routine-prompt">{routine.prompt}</p>
              </div>
              <div className="project-agent-actions">
                <input
                  type="checkbox"
                  role="switch"
                  className="g-switch"
                  aria-label={`Run ${routine.title}`}
                  aria-checked={routine.enabled}
                  checked={routine.enabled}
                  onChange={(event) =>
                    void coordination.apply({
                      type: "routine_save",
                      id: routine.id,
                      title: routine.title,
                      prompt: routine.prompt,
                      every_minutes: routine.every_minutes,
                      enabled: event.currentTarget.checked,
                    })
                  }
                />
                <button
                  type="button"
                  className="g-button g-button-ghost g-button-28"
                  onClick={() => setEditing(routine)}
                >
                  Edit
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {editing ? (
        <RoutineDialog
          routine={editing === "new" ? null : editing}
          coordination={coordination}
          onClose={() => setEditing(null)}
        />
      ) : null}
    </section>
  );
}

function RoutineDialog({
  routine,
  coordination,
  onClose,
}: {
  readonly routine: CoordinatorRoutine | null;
  readonly coordination: ProjectCoordination;
  readonly onClose: () => void;
}) {
  const [title, setTitle] = useState(routine?.title ?? "");
  const [prompt, setPrompt] = useState(routine?.prompt ?? "");
  const [minutes, setMinutes] = useState(routine?.every_minutes ?? 60);
  const [failed, setFailed] = useState(false);
  const presets = ROUTINE_PRESETS.some((preset) => preset.minutes === minutes)
    ? ROUTINE_PRESETS
    : [...ROUTINE_PRESETS, { minutes, name: `Every ${minutes} minutes` }];
  return (
    <Modal title={routine ? "Routine" : "New routine"} size="md" onClose={onClose}>
      <form
        className="project-task-form"
        data-testid="project-routine-form"
        onSubmit={async (event) => {
          event.preventDefault();
          setFailed(false);
          const done = await coordination.apply({
            type: "routine_save",
            id: routine?.id ?? null,
            title: title.trim(),
            prompt: prompt.trim(),
            every_minutes: minutes,
            enabled: routine?.enabled ?? true,
          });
          if (done) onClose();
          else setFailed(true);
        }}
      >
        <label className="ui-field">
          <span>Name</span>
          <input
            className="g-input"
            aria-label="Name"
            value={title}
            maxLength={120}
            placeholder="Watch pull requests"
            // biome-ignore lint/a11y/noAutofocus: a dialog opened to name a routine starts in its name.
            autoFocus={!routine}
            onChange={(event) => setTitle(event.currentTarget.value)}
          />
        </label>
        <label className="ui-field">
          <span>What the coordinator is asked each time</span>
          <textarea
            className="g-textarea"
            aria-label="Prompt"
            rows={5}
            value={prompt}
            placeholder="Check the open pull requests of this repository. Fix failing checks, answer review comments, and tell me only what needs me."
            onChange={(event) => setPrompt(event.currentTarget.value)}
          />
        </label>
        <label className="ui-field">
          <span>How often</span>
          <Select
            className="g-select"
            aria-label="How often"
            value={String(minutes)}
            onChange={(event) => setMinutes(Number(event.currentTarget.value))}
          >
            {presets.map((preset) => (
              <option key={preset.minutes} value={String(preset.minutes)}>
                {preset.name}
              </option>
            ))}
          </Select>
        </label>
        {failed ? (
          <p role="alert" className="ui-error">
            {coordination.error ?? "The routine was not saved."}
          </p>
        ) : null}
        <div className="g-dialog-actions project-task-actions">
          {routine ? (
            <button
              type="button"
              className="g-button g-button-ghost project-danger"
              disabled={coordination.busy}
              onClick={async () => {
                if (await coordination.apply({ type: "routine_delete", id: routine.id })) onClose();
              }}
            >
              <Trash2 size={14} aria-hidden="true" /> Remove
            </button>
          ) : null}
          <span className="project-foot-space" />
          <button type="button" className="g-button" onClick={onClose}>
            Cancel
          </button>
          <button
            type="submit"
            className="g-button g-button-primary"
            data-testid="project-routine-save"
            disabled={coordination.busy || !title.trim() || !prompt.trim()}
          >
            {routine ? "Save" : "Add routine"}
          </button>
        </div>
      </form>
    </Modal>
  );
}
