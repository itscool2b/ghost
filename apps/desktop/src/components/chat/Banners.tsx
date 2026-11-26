import {
  DatabaseBackup,
  HardDrive,
  Info,
  NotebookPen,
  Scale,
  TriangleAlert,
  X,
} from "lucide-react";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import type { EnvStatus, StorageStatus } from "../../bindings";
import { type DismissState, SHOWN } from "../../lib/dismiss";
import { PRIVACY_URL, TERMS_URL } from "../../lib/legal";
import type { AgentEditState } from "../../lib/projectMemory";
import { dismissRecovery } from "../RecoveryNotice";
import type { NoticeItem } from "./Notifications";

/** An agent's direct edit to project memory, as the window's toast shows it. */
export interface MemoryEditBanner {
  readonly key: string;
  /** "Project memory updated". */
  readonly title: string;
  /** "Claude Code · Fix the login flow · Ghost", on one line. */
  readonly detail: string;
  /** The whole sentence, for the tooltip a shortened line leaves out. */
  readonly text: string;
  readonly state: AgentEditState;
  /** Opens Customize > Project memory on the edit, with its changes. */
  readonly onReview?: () => void;
  readonly onUndo: () => void;
  readonly onDismiss: () => void;
}

/** A memory edit's toast leaves by itself after this long, unless it is pointed at or focused. */
export const MEMORY_TOAST_MS = 12_000;
/** An undone edit's toast, which only confirms, leaves sooner. */
export const UNDONE_TOAST_MS = 6_000;

/** The sentence a storage problem shows, or `null` while saving runs. */
export function storageBanner(storage: StorageStatus | null): string | null {
  if (storage === null) return null;
  switch (storage.state.state) {
    case "ok":
      return null;
    case "paused":
      return storage.state.reason === "disk_full"
        ? "The disk is full. Ghost paused saving and the agents are waiting. Free some space; nothing is lost unless you quit now."
        : "Ghost's database is busy (another program is using it). Ghost paused saving and the agents are waiting; nothing is lost unless you quit now.";
    case "failed":
      return storage.state.message;
  }
}

/** The sentence an environment fallback shows, or `null` when the login shell's env is in use. */
export function envBanner(env: EnvStatus | null): string | null {
  if (env?.state.state !== "fallback") return null;
  return `Ghost couldn't read your shell's environment (${env.state.reason}), so agents start with a basic one and may not find their tools.`;
}

/**
 * One notice: a mark, a short title, one quieter line, its actions, and an X when it can go. It is
 * a row in the notifications panel (`Notifications`), or a toast at the pane's top right when it
 * cannot wait (`Banners`).
 */
export function Toast({
  level,
  mark,
  title,
  detail,
  actions,
  onDismiss,
  dismissLabel = "Dismiss",
  dismissDisabled = false,
  tooltip,
  testId,
  live = "status",
  autoDismissMs = null,
  onExpire,
}: {
  readonly level?: "warning" | "error";
  readonly mark: ReactNode;
  readonly title: ReactNode;
  readonly detail?: ReactNode;
  readonly actions?: ReactNode;
  readonly onDismiss?: () => void;
  readonly dismissLabel?: string;
  readonly dismissDisabled?: boolean;
  readonly tooltip?: string;
  readonly testId?: string;
  /** How a screen reader hears it arrive: politely, or at once for a failure. */
  readonly live?: "status" | "alert";
  /** Leaves by itself after this long while nobody points at or focuses it; `null` stays. */
  readonly autoDismissMs?: number | null;
  /** What leaving by itself does, when it is not the X's dismissal (a toast that moves on). */
  readonly onExpire?: () => void;
}) {
  const text = useId();
  const [held, setHeld] = useState({ pointer: false, focus: false });
  const paused = held.pointer || held.focus;
  // The window re-renders often; the countdown follows only what it waits for, not each new
  // callback, and calls the newest one.
  const leave = useRef(onExpire ?? onDismiss);
  leave.current = onExpire ?? onDismiss;
  const leaves = leave.current !== undefined;
  useEffect(() => {
    if (autoDismissMs === null || !leaves || paused) return;
    const timer = setTimeout(() => leave.current?.(), autoDismissMs);
    return () => clearTimeout(timer);
  }, [autoDismissMs, leaves, paused]);
  return (
    <li
      className="chat-banner ui-toast"
      data-level={level}
      data-testid={testId}
      title={tooltip}
      onPointerEnter={() => setHeld((now) => ({ ...now, pointer: true }))}
      onPointerLeave={() => setHeld((now) => ({ ...now, pointer: false }))}
      onFocus={() => setHeld((now) => ({ ...now, focus: true }))}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null))
          setHeld((now) => ({ ...now, focus: false }));
      }}
    >
      <span className="ui-toast-mark" aria-hidden="true">
        {mark}
      </span>
      <div className="ui-toast-body">
        <div id={text} role={live}>
          <p className="ui-toast-title">{title}</p>
          {detail ? <p className="ui-toast-detail">{detail}</p> : null}
        </div>
        {actions ? <div className="ui-toast-actions">{actions}</div> : null}
      </div>
      {onDismiss ? (
        <button
          type="button"
          className="g-button g-button-ghost g-button-24 g-button-icon ui-toast-close"
          aria-label={dismissLabel}
          aria-describedby={text}
          title={dismissLabel}
          disabled={dismissDisabled}
          onClick={onDismiss}
        >
          <X size={14} aria-hidden="true" />
        </button>
      ) : null}
    </li>
  );
}

/** Saving paused or failed: a notice that cannot wait, also kept in the panel. */
function StorageToast({
  storage,
  text,
}: {
  readonly storage: StorageStatus;
  readonly text: string;
}) {
  const failed = storage.state.state === "failed";
  return (
    <Toast
      level={failed ? "error" : "warning"}
      live={failed ? "alert" : "status"}
      mark={<HardDrive size={16} />}
      title={failed ? "Saving failed" : "Saving paused"}
      detail={text}
    />
  );
}

/**
 * What cannot wait, over the pane's top right under its header: saving paused or failed (the
 * agents wait on it), and an agent's fresh edit to project memory while its Undo matters. Each
 * edit's toast moves on by itself (`onExpire`) and the edit stays in the notifications panel;
 * without `onExpire` leaving by itself dismisses it. Everything else waits in the panel only.
 */
export function Banners({
  storage,
  memoryEdits = [],
  onExpire,
  legal = null,
}: {
  readonly storage: StorageStatus | null;
  readonly memoryEdits?: readonly MemoryEditBanner[];
  readonly onExpire?: (key: string) => void;
  /** The Terms of Service and Privacy Policy, until this copy accepts them (`lib/legal.ts`). */
  readonly legal?: LegalBanner | null;
}) {
  const storageText = storageBanner(storage);
  if ((storage === null || storageText === null) && memoryEdits.length === 0 && legal === null)
    return null;
  return (
    <ul className="chat-banners ui-toasts" aria-label="Notices">
      {legal !== null ? <LegalToast legal={legal} /> : null}
      {storage !== null && storageText !== null ? (
        <StorageToast storage={storage} text={storageText} />
      ) : null}
      {memoryEdits.map((edit) => (
        <MemoryEditBannerView
          key={edit.key}
          edit={edit}
          onExpire={onExpire ? () => onExpire(edit.key) : undefined}
        />
      ))}
    </ul>
  );
}

/** The Terms of Service and Privacy Policy notice: what it says and its OK. */
export interface LegalBanner {
  readonly title: string;
  readonly detail: string;
  readonly onAccept: () => void;
}

/**
 * Shown once to a copy that has not accepted the current Terms of Service and Privacy Policy
 * (both promise "an in-app notice" for material changes). It stays until OK; the links open in
 * the browser.
 */
function LegalToast({ legal }: { readonly legal: LegalBanner }) {
  return (
    <Toast
      testId="legal-banner"
      mark={<Scale size={16} />}
      title={legal.title}
      detail={legal.detail}
      actions={
        <>
          <a
            className="g-button g-button-24"
            href={TERMS_URL}
            target="_blank"
            rel="noreferrer noopener"
          >
            Terms of Service
          </a>
          <a
            className="g-button g-button-24"
            href={PRIVACY_URL}
            target="_blank"
            rel="noreferrer noopener"
          >
            Privacy Policy
          </a>
          <button
            type="button"
            className="g-button g-button-24 g-button-primary"
            onClick={legal.onAccept}
          >
            OK
          </button>
        </>
      }
    />
  );
}

/**
 * Every notice for the notifications panel, in order: saving, the environment fallback with
 * Retry, the database repair notice (dismissable for good), startup notices (dismissable for this
 * run), agents' memory edits with Review and Undo, then agents' new releases (`updates`).
 */
export function noticeItems({
  storage,
  env,
  onRetryEnv,
  recoveryNotice,
  onRecoveryDismissed,
  notices,
  onDismissNotice,
  memoryEdits = [],
  updates = [],
}: {
  readonly storage: StorageStatus | null;
  readonly env: EnvStatus | null;
  readonly onRetryEnv: () => void;
  readonly recoveryNotice: string | null;
  readonly onRecoveryDismissed: () => void;
  readonly notices: readonly string[];
  readonly onDismissNotice: (notice: string) => void;
  readonly memoryEdits?: readonly MemoryEditBanner[];
  readonly updates?: readonly NoticeItem[];
}): NoticeItem[] {
  const items: NoticeItem[] = [];
  const storageText = storageBanner(storage);
  if (storage !== null && storageText !== null)
    items.push({
      key: `storage:${storage.state.state}`,
      level: storage.state.state === "failed" ? "error" : "warning",
      node: <StorageToast storage={storage} text={storageText} />,
    });
  const envText = envBanner(env);
  if (envText !== null)
    items.push({
      key: "env",
      level: "warning",
      node: (
        <Toast
          level="warning"
          mark={<TriangleAlert size={16} />}
          title="Agents start with a basic environment"
          detail={envText}
          actions={
            <button type="button" className="g-button g-button-24" onClick={onRetryEnv}>
              Retry
            </button>
          }
        />
      ),
    });
  if (recoveryNotice !== null)
    items.push({
      key: "recovery",
      level: "warning",
      node: <RecoveryBanner message={recoveryNotice} onDismissed={onRecoveryDismissed} />,
    });
  for (const notice of notices)
    items.push({
      key: `notice:${notice}`,
      node: (
        <Toast mark={<Info size={16} />} title={notice} onDismiss={() => onDismissNotice(notice)} />
      ),
    });
  for (const edit of memoryEdits)
    items.push({ key: `memory:${edit.key}`, node: <MemoryEditBannerView edit={edit} lasting /> });
  items.push(...updates);
  return items;
}

/**
 * An agent's direct edit to project memory: a calm toast that never blocks the chat and moves on
 * by itself, or with `lasting` a row in the notifications panel that stays until its X
 * (owner's plan, 4 November 2026). Open shows the memory as it is now in Settings; Undo restores
 * what the edit replaced.
 */
export function MemoryEditBannerView({
  edit,
  lasting = false,
  onExpire,
}: {
  readonly edit: MemoryEditBanner;
  readonly lasting?: boolean;
  readonly onExpire?: () => void;
}) {
  const { state } = edit;
  const busy = state.status === "undoing";
  return (
    <Toast
      testId="memory-edit-banner"
      mark={<NotebookPen size={16} />}
      title={edit.title}
      tooltip={edit.text}
      detail={
        <>
          <span className="ui-toast-line">{edit.detail}</span>
          {state.status === "failed" ? (
            <span role="alert" className="ui-error">
              {state.message}
            </span>
          ) : null}
        </>
      }
      autoDismissMs={
        lasting
          ? null
          : state.status === "shown"
            ? MEMORY_TOAST_MS
            : state.status === "undone"
              ? UNDONE_TOAST_MS
              : null
      }
      onExpire={onExpire}
      onDismiss={edit.onDismiss}
      dismissDisabled={busy}
      actions={
        edit.onReview || state.status === "shown" || busy ? (
          <>
            {edit.onReview && state.status !== "undone" ? (
              <button
                type="button"
                className="g-button g-button-24"
                disabled={busy}
                onClick={edit.onReview}
              >
                Open
              </button>
            ) : null}
            {state.status === "shown" || busy ? (
              <button
                type="button"
                className="g-button g-button-24"
                disabled={busy}
                onClick={edit.onUndo}
              >
                {busy ? "Undoing…" : "Undo"}
              </button>
            ) : null}
          </>
        ) : null
      }
    />
  );
}

/**
 * The database repair notice: it shows on every launch until dismissed here, because whatever the
 * repair could not save is only in the damaged copy it names. The shell must confirm first.
 */
export function RecoveryBanner({
  message,
  onDismissed,
}: {
  readonly message: string;
  readonly onDismissed: () => void;
}) {
  const [state, setState] = useState<DismissState>(SHOWN);
  return (
    <RecoveryBannerView
      message={message}
      state={state}
      onDismiss={() => {
        void dismissRecovery(setState, onDismissed);
      }}
    />
  );
}

/** What `RecoveryBanner` shows in `state`. */
export function RecoveryBannerView({
  message,
  state,
  onDismiss,
}: {
  readonly message: string;
  readonly state: DismissState;
  readonly onDismiss: () => void;
}) {
  if (state.status === "dismissed") return null;
  return (
    <Toast
      level="warning"
      mark={<DatabaseBackup size={16} />}
      title="Ghost repaired its database"
      detail={
        <>
          {message}
          {state.status === "shown" && state.problem !== null ? (
            <span role="alert" className="ui-error">
              {" "}
              {state.problem}
            </span>
          ) : null}
        </>
      }
      onDismiss={onDismiss}
      dismissLabel={state.status === "dismissing" ? "Dismissing…" : "Dismiss"}
      dismissDisabled={state.status === "dismissing"}
    />
  );
}
