import { FolderOpen, Plus } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type {
  AgentEntry,
  ManagedWorktree,
  ProjectSummary,
  WorkspaceRemovalTarget,
} from "../../bindings";
import type { Transport } from "../../lib/chat/transport";
import { errorMessage } from "../../lib/errors";
import { inTauri } from "../../lib/ipc";
import type { CloneApi } from "../../lib/setup-actions/api";
import type { WorkspaceStore } from "../../lib/workspace";
import { type WorktreeRecoveryApi, worktreeRecoveryApi } from "../../lib/worktreeRecovery";
import { Select } from "../Select";
import { CloneProject } from "../setup/CloneProject";
import { FolderResolver } from "../setup/FolderResolver";
import { projectName } from "./ChatList";
import { Modal } from "./Modal";
import { WorkspaceRemovalDialog } from "./WorkspaceRemovalDialog";
import { WorktreeRecoveryList, worktreesToReview } from "./WorktreeRecoveryList";

/**
 * The colours a project can carry, as stored with the project, each drawn by its Graphite tone
 * (--g-project-*). The stored values are data and must not change.
 */
const COLORS = [
  { value: "#8a98a6", tone: "slate", name: "Slate" },
  { value: "#a59e92", tone: "sand", name: "Sand" },
  { value: "#8c9c88", tone: "sage", name: "Sage" },
  { value: "#a88e80", tone: "clay", name: "Clay" },
] as const;
export function ProjectDialog({
  project,
  store,
  transport,
  agents,
  onClose,
  profileId,
  onLocated,
  onInstructions,
  cloneApi,
  recoveryApi = worktreeRecoveryApi,
}: {
  /** The clone's commands; previews pass a synthetic service. */
  readonly cloneApi?: CloneApi;
  readonly recoveryApi?: WorktreeRecoveryApi;
  readonly project: ProjectSummary | null;
  readonly store: WorkspaceStore;
  readonly transport: Transport;
  readonly agents: readonly AgentEntry[] | null;
  readonly onClose: () => void;
  readonly profileId?: string;
  readonly onLocated?: () => Promise<void>;
  /** Opens this folder's agent instructions (AGENTS.md, CLAUDE.md) in Settings. */
  readonly onInstructions?: (folderId: string) => void;
}) {
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const current = state.data?.projects.find((item) => item.id === project?.id) ?? project;
  const [removal, setRemoval] = useState<{ target: WorkspaceRemovalTarget | null } | null>(null);
  const [name, setName] = useState(project?.name ?? "");
  const [folder, setFolder] = useState<string | null>(null);
  const [color, setColor] = useState<string | null>(project?.color ?? COLORS[0].value);
  const [agent, setAgent] = useState(project?.default_agent ?? "");
  const [model, setModel] = useState(project?.default_model ?? "");
  const [localError, setLocalError] = useState<string | null>(null);
  const [picking, setPicking] = useState(false);
  // This dialog's own save; another screen's change never disables it.
  const [saving, setSaving] = useState(false);
  const submitted = useRef(false);
  const [resolving, setResolving] = useState<string | null>(null);
  const [worktrees, setWorktrees] = useState<ManagedWorktree[]>([]);
  const [recoveryRevision, setRecoveryRevision] = useState(0);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);
  useEffect(() => {
    void recoveryRevision;
    if (!project) return;
    let current = true;
    void recoveryApi.list(project.id).then(
      (records) => {
        if (current) {
          setWorktrees(records);
          setRecoveryError(null);
        }
      },
      (error) => {
        if (current) setRecoveryError(errorMessage(error));
      },
    );
    return () => {
      current = false;
    };
  }, [project, recoveryApi, recoveryRevision]);
  const chooseFolder = async () => {
    setPicking(true);
    setLocalError(null);
    try {
      const path = await transport.pickFolder();
      if (!path) return;
      if (current) {
        const outcome = await store.perform(() => store.api.addFolder(current.id, path));
        if (!outcome.ok) setLocalError(outcome.error);
      } else {
        setFolder(path);
        if (!name.trim()) setName(projectName(path));
      }
    } catch (error) {
      setLocalError(errorMessage(error));
    } finally {
      setPicking(false);
    }
  };
  if (removal)
    return (
      <WorkspaceRemovalDialog
        target={removal.target}
        onClose={onClose}
        onChanged={async () => {
          await store.refresh();
          await onLocated?.();
        }}
      />
    );
  if (resolving && current) {
    return (
      <Modal title="Folder on this device" size="md" onClose={() => setResolving(null)}>
        <FolderResolver
          key={`${profileId}:${resolving}`}
          folderId={resolving}
          pickFolder={() => transport.pickFolder()}
          onLocated={async () => {
            await store.refresh();
            await onLocated?.();
          }}
        />
        <button className="g-button" type="button" onClick={() => setResolving(null)}>
          Back to project
        </button>
      </Modal>
    );
  }
  return (
    <Modal
      title={project ? "Project settings" : "New project"}
      size="md"
      className="project-dialog"
      onClose={onClose}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (submitted.current) return;
          const input = {
            name: name.trim(),
            color: color ?? null,
            default_agent: agent || null,
            default_model: agent && model.trim() ? model.trim() : null,
          };
          submitted.current = true;
          setSaving(true);
          setLocalError(null);
          void store
            .perform(() =>
              project
                ? store.api.updateProject(project.id, input)
                : store.api.createProject({ project: input, folder: folder ?? "" }),
            )
            .then((outcome) => {
              submitted.current = false;
              setSaving(false);
              if (outcome.ok) onClose();
              else setLocalError(outcome.error);
            });
        }}
      >
        {/* The name and its colour share the first line. */}
        <div className="project-identity">
          <label className="ui-field">
            Project name
            <input
              className="g-input"
              // The dialog opens on the name (Modal's showDialog).
              data-autofocus
              value={name}
              maxLength={128}
              required
              onChange={(event) => setName(event.currentTarget.value)}
            />
          </label>
          <fieldset className="chat-color-choices">
            <legend>Project color</legend>
            <div>
              {COLORS.map(({ value, tone, name: label }) => (
                <label key={value} data-tone={tone} title={label}>
                  <input
                    type="radio"
                    name="project-color"
                    aria-label={label}
                    checked={color === value}
                    onChange={() => setColor(value)}
                  />
                  <span aria-hidden="true" />
                </label>
              ))}
            </div>
          </fieldset>
        </div>
        <section className="chat-project-folders" aria-label="Project folders">
          <h3>Folders</h3>
          {current?.folders.length ? (
            <ul className="project-folder-list">
              {current.folders.map((item) => (
                <li key={item.id}>
                  <FolderOpen size={16} aria-hidden="true" />
                  <strong>{item.display_name}</strong>
                  <span className="project-folder-actions">
                    {onInstructions && item.path && !item.unavailable_reason ? (
                      <button
                        type="button"
                        className="g-button g-button-ghost g-button-24"
                        aria-label={`Instructions for ${item.display_name}`}
                        onClick={() => onInstructions(item.id)}
                      >
                        Instructions
                      </button>
                    ) : null}
                    {inTauri() ? (
                      <button
                        type="button"
                        className="g-button g-button-ghost g-button-24"
                        onClick={() => setResolving(item.id)}
                      >
                        {item.path && !item.unavailable_reason
                          ? "Change location"
                          : "Find on this device"}
                      </button>
                    ) : null}
                    <button
                      type="button"
                      className="g-button g-button-ghost g-button-24"
                      onClick={() => setRemoval({ target: { kind: "folder", id: item.id } })}
                    >
                      Remove…
                    </button>
                  </span>
                  {item.path ? (
                    <small className="project-folder-path">{item.path}</small>
                  ) : (
                    <small>Not mapped on this device</small>
                  )}
                  {item.unavailable_reason ? (
                    <small className="ui-error">{item.unavailable_reason}</small>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
          {folder ? (
            <p className="project-folder-chosen">
              <FolderOpen size={16} aria-hidden="true" />
              <span className="ui-mono">{folder}</span>
            </p>
          ) : null}
          <div className="project-folder-add">
            <button
              type="button"
              className="g-button"
              disabled={picking || saving}
              onClick={() => void chooseFolder()}
            >
              <Plus size={14} aria-hidden="true" />
              {picking
                ? "Choosing…"
                : current
                  ? "Add folder"
                  : folder
                    ? "Change folder"
                    : "Choose folder"}
            </button>
            {current ? null : (
              // A new project can start from a repository: Git clones it into a new folder,
              // which becomes this project's folder.
              <CloneProject
                className="g-button chat-project-clone"
                api={cloneApi}
                disabled={picking || saving}
                pickParent={() => transport.pickFolder()}
                onFolder={(path, cloned) => {
                  setFolder(path);
                  if (!name.trim()) setName(cloned);
                }}
              />
            )}
          </div>
        </section>
        {/* What new chats in this project start with, side by side; the note is the model's. */}
        <div className="project-defaults">
          <label className="ui-field">
            Default agent
            <Select
              className="g-select"
              value={agent}
              onChange={(event) => {
                setAgent(event.currentTarget.value);
                setModel("");
              }}
            >
              <option value="">Choose when starting a chat</option>
              {(agents ?? []).map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name}
                </option>
              ))}
            </Select>
          </label>
          <label className="ui-field">
            Default model ID
            <input
              className="g-input"
              value={model}
              disabled={!agent}
              maxLength={256}
              spellCheck={false}
              onChange={(event) => setModel(event.currentTarget.value)}
              aria-describedby="project-default-model-help"
            />
          </label>
          <small id="project-default-model-help" className="ui-muted">
            Leave blank for the agent's default. A specific model ID is checked when a new chat
            starts.
          </small>
        </div>
        {(localError ?? state.error) ? (
          <p role="alert" className="ui-error">
            {localError ?? state.error}
          </p>
        ) : null}
        {project ? (
          <section className="project-removal" aria-label="Workspace removal">
            {recoveryError ? (
              <p role="alert" className="ui-error">
                Could not load retained worktrees: {recoveryError}
              </p>
            ) : null}
            <WorktreeRecoveryList
              records={worktreesToReview(
                worktrees,
                state.data ? new Set(state.data.chats.map((entry) => entry.chat.id)) : null,
              )}
              api={recoveryApi}
              onChanged={() => setRecoveryRevision((value) => value + 1)}
            />
            <button
              className="g-button g-button-ghost g-button-28"
              type="button"
              onClick={() => setRemoval({ target: { kind: "project", id: project.id } })}
            >
              Remove project…
            </button>
          </section>
        ) : null}
        <footer className="project-dialog-foot">
          {/* What was removed before lives beside the way out, apart from the two answers. */}
          <button
            className="g-button g-button-ghost g-button-28 project-removed"
            type="button"
            onClick={() => setRemoval({ target: null })}
          >
            Removed projects and folders
          </button>
          <button type="button" className="g-button g-button-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            type="submit"
            className="g-button g-button-primary"
            disabled={saving || picking || !name.trim() || (!project && !folder)}
          >
            {saving ? "Saving…" : project ? "Save project" : "Create project"}
          </button>
        </footer>
      </form>
    </Modal>
  );
}
