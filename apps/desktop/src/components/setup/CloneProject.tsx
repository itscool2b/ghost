import { Keycap, PixelLoader } from "@o7/ui/graphite";
import { ArrowUpRight, Check, FolderOpen, GitBranch } from "lucide-react";
import {
  Children,
  type ReactNode,
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { createPortal } from "react-dom";
import { type CloneApi, cloneApi } from "../../lib/setup-actions/api";
import {
  type CloneController,
  type CloneDraft,
  type CloneView,
  CloneController as Controller,
  folderName,
  progressLine,
} from "../../lib/setup-actions/clone";
import { DialogClose, dialogClassName, showDialog } from "../chat/Modal";

/** One clone form for its lifetime: checked by Rust as it changes, stopped when it closes. */
export function useClone(api: CloneApi, draft?: CloneDraft) {
  const [controller] = useState(() => new Controller(api, draft));
  useEffect(() => {
    controller.open();
    return () => controller.close();
  }, [controller]);
  const view = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );
  return { controller, view };
}

/** The two-tone heading for the clone form in setup, following the clone. */
export function cloneHeading(view: CloneView): [string, string] {
  const repository = view.record?.repository ?? view.check?.repository ?? "the repository";
  switch (view.phase) {
    case "running":
      return [`Cloning ${repository}.`, "Cancel leaves nothing behind."];
    case "failed":
      return ["The clone stopped.", "Nothing was added."];
    case "done":
      return [`Cloned ${repository}.`, "Adding it as a project."];
    default:
      return ["Clone a repository.", "Git copies it into a new folder."];
  }
}

const GIT_DOWNLOAD = "https://git-scm.com/downloads";

/**
 * The clone itself, in setup's column or in a dialog: the address, where it goes, the exact
 * command, then Git's progress, and a failure with Git's own last words. The one primary is Clone
 * (Enter); nothing runs before it.
 */
export function CloneBody({
  controller,
  view,
  pickParent,
  layout,
  onBack,
  onCloned,
  fixedRemote = false,
}: {
  readonly controller: CloneController;
  readonly view: CloneView;
  readonly pickParent: () => Promise<string | null>;
  readonly layout: "inline" | "dialog";
  /** Leaves the form: Back in setup, Cancel in a dialog. */
  readonly onBack: () => void;
  /** Called once when Git finishes, with the new folder. */
  readonly onCloned: (path: string, name: string) => void;
  readonly fixedRemote?: boolean;
}) {
  const ids = useId();
  const status = useRef<HTMLElement>(null);
  const reported = useRef<string | null>(null);
  const [picking, setPicking] = useState(false);
  const record = view.record;
  const review = view.check?.review ?? null;
  useEffect(() => {
    if (view.phase === "done" && record && reported.current !== record.id) {
      reported.current = record.id;
      onCloned(record.destination, folderName(record.destination));
    }
  }, [view.phase, record, onCloned]);
  // Keyboard focus follows the clone: onto its progress, then onto the failure.
  useEffect(() => {
    if (view.phase === "running" || view.phase === "failed") status.current?.focus();
  }, [view.phase]);
  // Keys while Git runs or after it stopped: Escape cancels a running clone (a dialog's own
  // Escape does the same) and goes back to the form after a failure; Enter tries again.
  useEffect(() => {
    if (view.phase !== "running" && view.phase !== "failed") return;
    const key = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing) return;
      if (event.target instanceof Element && event.target.closest("button, a, input")) return;
      if (view.phase === "running" && event.key === "Escape" && layout === "inline")
        controller.cancel();
      else if (view.phase === "failed" && event.key === "Enter") {
        event.preventDefault();
        controller.retry();
      } else if (view.phase === "failed" && event.key === "Escape" && layout === "inline")
        controller.back();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [layout, view.phase, controller]);
  const choose = () => {
    setPicking(true);
    void pickParent()
      .then((path) => {
        if (path) controller.setParent(path);
      })
      .finally(() => setPicking(false));
  };
  const dialog = layout === "dialog";
  const leave = (
    <button
      type="button"
      className={dialog ? "g-button" : "g-button g-button-ghost g-button-36 clone-back"}
      onClick={onBack}
    >
      {dialog ? "Cancel" : "Back"}
    </button>
  );

  if (view.phase === "running" && record) {
    const { stage, detail } = progressLine(record);
    const percent = record.progress?.percent ?? 0;
    const stopping = record.state === "cleanup_pending";
    return (
      <section
        ref={status}
        tabIndex={-1}
        className="clone-running"
        aria-label={`Cloning ${record.repository}`}
      >
        <div className="clone-progress">
          <div className="clone-status" role="status">
            <PixelLoader size={16} tempo="tool" />
            <span className="clone-stage">{stopping ? record.detail : stage}</span>
            <span className="clone-percent">{percent}%</span>
          </div>
          <div
            className="clone-bar"
            role="progressbar"
            aria-label="Clone progress"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
          >
            <span style={{ transform: `scaleX(${percent / 100})` }} />
          </div>
          <p className="clone-meta">
            {detail ? <span>{detail} received</span> : null}
            <span className="setup-path">{record.destination}</span>
          </p>
        </div>
        {review ? <CommandWell command={review.command} dim /> : null}
        <Actions dialog={dialog}>
          <button
            type="button"
            className={dialog ? "g-button" : "g-button g-button-36"}
            disabled={stopping}
            onClick={() => controller.cancel()}
          >
            {stopping ? "Stopping…" : "Cancel clone"}
            <span className="setup-enter" aria-hidden="true">
              <Keycap>Esc</Keycap>
            </span>
          </button>
        </Actions>
      </section>
    );
  }

  if (view.phase === "failed") {
    return (
      <section ref={status} tabIndex={-1} className="clone-failed" aria-label="Clone stopped">
        <div className="setup-error clone-error" role="alert">
          <p>{record?.detail ?? view.error}</p>
          {record?.output.length ? (
            <pre className="clone-output">{record.output.join("\n")}</pre>
          ) : null}
        </div>
        {review ? <CommandWell command={review.command} dim /> : null}
        <Actions dialog={dialog}>
          <button
            type="button"
            className={dialog ? "g-button" : "g-button g-button-ghost g-button-36 clone-back"}
            onClick={() => controller.back()}
          >
            Edit
          </button>
          <button
            type="button"
            className={
              dialog ? "g-button g-button-primary" : "g-button g-button-primary g-button-36"
            }
            onClick={() => controller.retry()}
          >
            Try again
            <span className="setup-enter" aria-hidden="true">
              <Keycap>↵</Keycap>
            </span>
          </button>
        </Actions>
      </section>
    );
  }

  if (view.phase === "done" && record) {
    return (
      <p role="status" className="setup-project-saved">
        <Check size={14} aria-hidden="true" /> Cloned {record.repository} into{" "}
        <span className="setup-path">{record.destination}</span>
      </p>
    );
  }

  const remoteProblem = controller.problem("remote");
  // No folder chosen yet is the next step, not a mistake: it reads as a calm hint until one is.
  const awaitingParent = !view.parent && controller.problem("parent") !== null;
  const parentProblem = awaitingParent ? null : controller.problem("parent");
  const placeProblem = parentProblem ?? controller.problem("name");
  const gitProblem = controller.problem("git");
  const separator = view.parent?.includes("\\") ? "\\" : "/";
  return (
    <form
      className="clone-form"
      data-layout={layout}
      onSubmit={(event) => {
        event.preventDefault();
        // React carries events through portals: a form this dialog opened over (New project)
        // must not submit too.
        event.stopPropagation();
        void controller.start();
      }}
      onKeyDown={(event) => {
        if (!dialog && event.key === "Escape") {
          event.preventDefault();
          onBack();
        }
      }}
    >
      {view.notice ? (
        <p role="status" className="clone-notice">
          {view.notice}
        </p>
      ) : null}
      <CloneRecoveryNotices controller={controller} view={view} fixedRemote={fixedRemote} />
      <div className="setup-field">
        <label htmlFor={`${ids}-remote`}>Repository</label>
        <input
          id={`${ids}-remote`}
          className="g-input clone-remote"
          value={view.remote}
          placeholder="https://github.com/owner/repository"
          spellCheck={false}
          autoComplete="off"
          autoCapitalize="off"
          // Setup and the dialogs open on this field: pasting the address is the first step.
          // biome-ignore lint/a11y/noAutofocus: The form opens on the person's request.
          autoFocus={!fixedRemote}
          data-autofocus={fixedRemote ? undefined : true}
          readOnly={fixedRemote}
          maxLength={2048}
          aria-invalid={remoteProblem ? true : undefined}
          aria-describedby={remoteProblem ? `${ids}-remote-problem` : undefined}
          onChange={(event) => controller.setRemote(event.currentTarget.value)}
        />
        {remoteProblem ? (
          <p id={`${ids}-remote-problem`} className="clone-problem">
            {remoteProblem}
          </p>
        ) : null}
      </div>
      <fieldset className="setup-field clone-fieldset">
        <legend>Clone into</legend>
        <div className="clone-place">
          <button
            type="button"
            className="clone-parent"
            disabled={picking}
            title={view.parent ?? undefined}
            aria-label={view.parent ? `Folder ${view.parent}, change` : "Choose a folder"}
            aria-invalid={parentProblem ? true : undefined}
            aria-describedby={awaitingParent ? `${ids}-place-hint` : undefined}
            onClick={choose}
          >
            <FolderOpen size={16} aria-hidden="true" />
            <span>{view.parent ? folderName(view.parent) : "Choose a folder"}</span>
          </button>
          <span className="clone-separator" aria-hidden="true">
            {separator}
          </span>
          <input
            className="g-input clone-name"
            aria-label="Folder name"
            value={view.name}
            placeholder={view.check?.name ?? "folder"}
            spellCheck={false}
            autoComplete="off"
            maxLength={100}
            aria-invalid={controller.problem("name") ? true : undefined}
            aria-describedby={placeProblem ? `${ids}-place-problem` : undefined}
            onChange={(event) => controller.setName(event.currentTarget.value)}
          />
        </div>
        {placeProblem ? (
          <p id={`${ids}-place-problem`} className="clone-problem">
            {placeProblem}
          </p>
        ) : awaitingParent ? (
          <p id={`${ids}-place-hint`} className="clone-hint">
            {controller.problem("parent")}
          </p>
        ) : null}
      </fieldset>
      {review ? (
        <div className="clone-review">
          <CommandWell command={review.command} />
          <p className="setup-footnote">
            Runs <span className="setup-path">{review.program}</span> with your own Git settings and
            sign-in. Cancel removes the unfinished folder.
          </p>
        </div>
      ) : gitProblem ? (
        <p className="clone-problem clone-git">
          {gitProblem}{" "}
          <a href={GIT_DOWNLOAD} target="_blank" rel="noreferrer">
            Get Git <ArrowUpRight size={12} aria-hidden="true" />
          </a>
        </p>
      ) : null}
      {view.error ? (
        <p role="alert" className="setup-alert">
          {view.error}
        </p>
      ) : null}
      <Actions dialog={dialog}>
        {leave}
        <button
          type="submit"
          className={dialog ? "g-button g-button-primary" : "g-button g-button-primary g-button-36"}
          disabled={!controller.canStart}
        >
          {view.starting ? "Starting…" : "Clone"}
          <span className="setup-enter" aria-hidden="true">
            <Keycap>↵</Keycap>
          </span>
        </button>
      </Actions>
    </form>
  );
}

function CloneRecoveryNotices({
  controller,
  view,
  fixedRemote,
}: {
  controller: CloneController;
  view: CloneView;
  fixedRemote: boolean;
}) {
  const recoveries = view.recoveries.filter(
    (record) => !fixedRemote || record.remote === view.remote,
  );
  return (
    <>
      {view.recoveryError ? (
        <p role="alert" className="setup-alert">
          {view.recoveryError}
        </p>
      ) : null}
      {recoveries.length ? (
        <section aria-label="Interrupted clones" className="setup-field">
          <h3>Previous clones</h3>
          {recoveries.map((record) => (
            <div key={record.id} className="setup-field">
              <p>{record.detail}</p>
              {record.destination ? <p className="setup-path">{record.destination}</p> : null}
              <div className="g-dialog-actions">
                {record.can_open ? (
                  <button
                    type="button"
                    className="g-button"
                    disabled={!controller.canRecover}
                    onClick={() => void controller.recover(record.id, "open")}
                  >
                    Open folder
                  </button>
                ) : null}
                {record.can_add ? (
                  <button
                    type="button"
                    className="g-button"
                    disabled={!controller.canRecover}
                    onClick={() => void controller.recover(record.id, "add")}
                  >
                    Add existing project
                  </button>
                ) : null}
                {record.remote ? (
                  <button
                    type="button"
                    className="g-button"
                    disabled={!controller.canRecover}
                    onClick={() => controller.retryElsewhere(record)}
                  >
                    Retry in another folder
                  </button>
                ) : null}
                <button
                  type="button"
                  className="g-button"
                  disabled={!controller.canRecover}
                  onClick={() => void controller.recover(record.id, "dismiss")}
                >
                  Dismiss notice
                </button>
              </div>
            </div>
          ))}
          <p className="setup-footnote">
            Dismissing a notice removes its record only. Your folders stay where they are.
          </p>
        </section>
      ) : null}
    </>
  );
}

/**
 * A dialog's actions sit right with the primary last (4.6); setup's sit at the column's edge with
 * the primary first, as every setup step's one pill does.
 */
function Actions({ dialog, children }: { dialog: boolean; children: ReactNode }) {
  return dialog ? (
    <div className="g-dialog-actions">{children}</div>
  ) : (
    <div className="setup-actions">{Children.toArray(children).reverse()}</div>
  );
}

/** The exact command, as a person would type it, in a code well. */
function CommandWell({ command, dim = false }: { command: string; dim?: boolean }) {
  return (
    <div className="clone-command">
      <code className="clone-command-text" data-dim={dim || undefined}>
        {command}
      </code>
    </div>
  );
}

/**
 * A button that opens the clone in a dialog, for New project and a folder missing on this device.
 * The new folder goes to `onFolder` as soon as Git finishes.
 */
export function CloneProject({
  pickParent,
  onFolder,
  disabled = false,
  api = cloneApi,
  remote,
  label = "Clone a repository",
  className = "g-button g-button-ghost g-button-28 setup-clone",
}: {
  readonly pickParent: () => Promise<string | null>;
  readonly onFolder: (path: string, name: string) => void;
  readonly disabled?: boolean;
  readonly api?: CloneApi;
  /** A fixed address: cloning a project's known repository. */
  readonly remote?: string;
  readonly label?: string;
  readonly className?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className={className} disabled={disabled} onClick={() => setOpen(true)}>
        <GitBranch size={14} aria-hidden="true" />
        {label}
      </button>
      {open ? (
        <CloneDialog
          api={api}
          pickParent={pickParent}
          remote={remote}
          onClose={() => setOpen(false)}
          onFolder={(path, name) => {
            setOpen(false);
            onFolder(path, name);
          }}
        />
      ) : null}
    </>
  );
}

function CloneDialog({
  api,
  pickParent,
  remote,
  onClose,
  onFolder,
}: {
  readonly api: CloneApi;
  readonly pickParent: () => Promise<string | null>;
  readonly remote?: string;
  readonly onClose: () => void;
  readonly onFolder: (path: string, name: string) => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const title = useId();
  const { controller, view } = useClone(api, remote === undefined ? undefined : { remote });
  useEffect(() => {
    const element = dialog.current;
    if (element) showDialog(element);
    return () => element?.close();
  }, []);
  const running = view.phase === "running";
  return createPortal(
    <dialog
      ref={dialog}
      className={dialogClassName("md", "setup-dialog setup-clone-dialog")}
      aria-labelledby={title}
      onCancel={(event) => {
        event.preventDefault();
        // Only this dialog: React would carry the cancel on to a dialog it was opened over.
        event.stopPropagation();
        // Escape while Git runs stops the clone first; the dialog stays to show it.
        if (running) controller.cancel();
        else onClose();
      }}
    >
      <header className="g-dialog-header">
        <h2 id={title} className="g-dialog-title">
          {running && view.record ? `Cloning ${view.record.repository}` : "Clone a repository"}
        </h2>
        <DialogClose label="Close clone" onClick={onClose} />
      </header>
      {view.phase === "form" ? (
        <p className="setup-dialog-lead">Git copies the repository into a new folder.</p>
      ) : null}
      <CloneBody
        controller={controller}
        view={view}
        pickParent={pickParent}
        layout="dialog"
        onBack={onClose}
        onCloned={onFolder}
        fixedRemote={remote !== undefined}
      />
    </dialog>,
    document.body,
  );
}
