import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { ChatSummary, WorkspaceChat } from "../../bindings";
import { hasProcess } from "../../lib/chat/transcript";
import type { Transport } from "../../lib/chat/transport";
import { busyElsewhere, moveChat } from "../../lib/chatMove";
import { errorMessage } from "../../lib/errors";
import { blockersFromChats, type FolderBusyApi } from "../../lib/folderBusy";
import type { WorkspaceStore } from "../../lib/workspace";
import { Select } from "../Select";
import { FolderBusy } from "./FolderBusy";
import { Modal } from "./Modal";

/**
 * The busy-folder warning New chat shows, for a move: the chats already working in the folder,
 * an offer to stop them, and one explicit checkbox. Nothing overrides the warning until it is
 * checked; stopping them clears it.
 */
export function MoveBusyWarning({
  busy,
  acknowledged,
  onAcknowledge,
  onStopped = () => {},
  projectOf = () => null,
  api,
}: {
  readonly busy: readonly ChatSummary[];
  readonly acknowledged: boolean;
  readonly onAcknowledge: (value: boolean) => void;
  /** The chats were stopped: check the folder again. */
  readonly onStopped?: () => void;
  readonly projectOf?: (chatId: string) => string | null;
  readonly api?: Pick<FolderBusyApi, "stop">;
}) {
  if (busy.length === 0) return null;
  return (
    <div data-testid="move-busy">
      <FolderBusy
        heading="This folder has active work"
        blockers={blockersFromChats(busy, projectOf)}
        onStopped={onStopped}
        api={api}
      >
        <label>
          <input
            type="checkbox"
            data-testid="move-busy-acknowledge"
            checked={acknowledged}
            onChange={(event) => onAcknowledge(event.currentTarget.checked)}
          />
          Move this chat into it anyway. Agents may edit the same files.
        </label>
      </FolderBusy>
    </div>
  );
}

export function ChatActionsDialog({
  entry,
  chat,
  store,
  transport,
  onChanged,
  onClose,
}: {
  readonly entry: WorkspaceChat;
  readonly chat: ChatSummary;
  readonly store: WorkspaceStore;
  readonly transport: Transport;
  readonly onChanged: (chat: ChatSummary) => void;
  readonly onClose: () => void;
}) {
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const [title, setTitle] = useState(chat.title ?? "");
  const [projectId, setProjectId] = useState(entry.project_id);
  const [folderId, setFolderId] = useState("");
  const [busy, setBusy] = useState<readonly ChatSummary[]>([]);
  const [acknowledged, setAcknowledged] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);
  // This dialog's own work only: another screen's change never disables it. Stop goes straight
  // to the session, beside whatever else runs.
  const [saving, setSaving] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const live = hasProcess(chat.status);
  const folders =
    state.data?.projects
      .find((project) => project.id === projectId)
      ?.folders.filter((folder) => folder.path !== null) ?? [];
  // Checked again once the chats in its way were stopped.
  const [recheck, setRecheck] = useState(0);
  const projectOf = (chatId: string) => {
    const saved = state.data?.chats.find((item) => item.chat.id === chatId);
    return state.data?.projects.find((project) => project.id === saved?.project_id)?.name ?? null;
  };
  // A new destination folder starts unacknowledged: the warning is about that folder only.
  useEffect(() => {
    void recheck;
    let current = true;
    setBusy([]);
    setAcknowledged(false);
    setCheckError(null);
    if (folderId)
      void store.api.preview(folderId).then(
        (preview) => {
          if (current) setBusy(busyElsewhere(preview, chat.id));
        },
        (error) => {
          if (current) setCheckError(errorMessage(error));
        },
      );
    return () => {
      current = false;
    };
  }, [folderId, store, chat.id, recheck]);
  const running = useRef(false);
  const run = <T,>(operation: () => Promise<T>, done: (value: T) => void) => {
    if (running.current) return;
    running.current = true;
    setSaving(true);
    setError(null);
    void store.perform(operation).then((outcome) => {
      running.current = false;
      setSaving(false);
      if (outcome.ok) done(outcome.value);
      else setError(outcome.error);
    });
  };
  const save = (operation: () => Promise<WorkspaceChat>) =>
    run(operation, (result) => {
      onChanged(result.chat);
      onClose();
    });
  const unchanged = projectId === entry.project_id && (!folderId || folderId === entry.folder_id);
  const move = () =>
    run(
      () => moveChat(store.api, chat.id, { projectId, folderId: folderId || null }, acknowledged),
      (outcome) => {
        if (outcome.kind === "busy") {
          setBusy(outcome.busy);
          return;
        }
        onChanged(outcome.chat.chat);
        onClose();
      },
    );
  const stop = () => {
    if (stopping) return;
    setStopping(true);
    setError(null);
    transport.stop(chat.id).then(
      () => setStopping(false),
      (reason: unknown) => {
        setStopping(false);
        setError(errorMessage(reason));
      },
    );
  };
  return (
    <Modal title="Chat options" onClose={onClose}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          save(() => store.api.rename(chat.id, title));
        }}
      >
        {/* The name with its one action beside it; the dialog opens on the name. */}
        <div className="dialog-inline-field">
          <label className="ui-field">
            Chat name
            <input
              className="g-input"
              data-autofocus
              value={title}
              maxLength={512}
              required
              onChange={(event) => setTitle(event.currentTarget.value)}
            />
          </label>
          <button type="submit" className="g-button" disabled={saving || !title.trim()}>
            Rename chat
          </button>
        </div>
        <div className="dialog-section">
          <button
            type="button"
            className="g-button"
            disabled={saving}
            onClick={() => save(() => store.api.pin(chat.id, !entry.pinned))}
          >
            {entry.pinned ? "Unpin chat" : "Pin chat"}
          </button>
          <button
            type="button"
            className="g-button"
            disabled={saving}
            onClick={() => save(() => store.api.archive(chat.id, !entry.archived))}
          >
            {entry.archived ? "Restore chat" : "Archive chat"}
          </button>
          <p>Archived chats stay saved and can be restored from the sidebar.</p>
        </div>
        <div className="dialog-section">
          <label className="ui-field">
            Move to project
            <Select
              className="g-select"
              value={projectId}
              onChange={(event) => {
                setProjectId(event.currentTarget.value);
                setFolderId("");
              }}
            >
              {state.data?.projects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name}
                </option>
              ))}
            </Select>
          </label>
          {folders.length > 0 ? (
            <label className="ui-field">
              Folder
              <Select
                className="g-select"
                data-testid="move-folder"
                value={folderId}
                onChange={(event) => setFolderId(event.currentTarget.value)}
              >
                <option value="">Its current folder</option>
                {folders.map((folder) => (
                  <option key={folder.id} value={folder.id}>
                    {folder.display_name} ({folder.path})
                  </option>
                ))}
              </Select>
            </label>
          ) : null}
          <p>
            {folderId
              ? "The chat works in this folder from its next turn. Files are not moved."
              : "The chat keeps its working folder. Files stay where they are."}
          </p>
          <MoveBusyWarning
            busy={busy}
            acknowledged={acknowledged}
            onAcknowledge={setAcknowledged}
            projectOf={projectOf}
            onStopped={() => setRecheck((value) => value + 1)}
          />
          {checkError ? (
            <p role="alert" className="ui-error">
              {checkError}
            </p>
          ) : null}
          {live ? (
            <>
              <p>Stop the session before moving this chat.</p>
              <button
                type="button"
                className="g-button"
                disabled={stopping || chat.status === "stopping"}
                onClick={stop}
              >
                Stop session
              </button>
            </>
          ) : null}
          <button
            type="button"
            className="g-button"
            disabled={saving || live || unchanged || (busy.length > 0 && !acknowledged)}
            onClick={move}
          >
            Move chat
          </button>
        </div>
        {error ? (
          <p role="alert" className="ui-error">
            {error}
          </p>
        ) : null}
      </form>
    </Modal>
  );
}
