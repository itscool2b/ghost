import { ArrowDownToLine } from "lucide-react";
import { type UpdatesApi, updateBusy, updateOffered, useAppUpdate } from "../../lib/updates";
import { Popover } from "./Popover";

const MB = 1_048_576;

/**
 * A newer Ghost, offered at the foot of the sidebar (owner, 18 November 2026: "an update button
 * pops up for every merge, and if u click it updates"). Nothing shows while the app is current.
 * When Rust's own check has found a newer version, an Update pill appears beside the bell; it
 * opens the version, what changed and one button, Update and restart, which downloads the
 * package, checks its signature and installs it. The panel says first what the restart does to
 * work under way, and follows the download and the installation as they run.
 */
export function AppUpdate({
  api,
  working,
}: {
  readonly api: UpdatesApi;
  /** Chats with a turn under way now: they stop for the restart and pick up after it. */
  readonly working: number;
}) {
  const { status, error, busy, update } = useAppUpdate(api);
  if (!status || !updateOffered(status)) return null;
  const { phase } = status;
  const running = busy || updateBusy(phase);
  const word =
    phase === "downloading"
      ? "Downloading"
      : phase === "installing"
        ? "Installing"
        : phase === "restart_required"
          ? "Restart"
          : "Update";
  const version = status.version ? `Ghost ${status.version}` : "A newer Ghost";
  return (
    <Popover
      className="app-update"
      label="Update Ghost"
      triggerLabel={`${word}: ${version} is available`}
      tip="Update Ghost"
      trigger={
        <span className="app-update-pill" data-testid="app-update" data-phase={phase}>
          <ArrowDownToLine size={14} aria-hidden="true" />
          <span className="app-update-word">{word}</span>
        </span>
      }
    >
      <div className="app-update-head">
        <h2 className="app-update-title">{version}</h2>
        <p className="app-update-meta">
          You have {status.current_version || "an older version"}
          {status.channel === "staging" ? " · staging" : ""}
        </p>
      </div>
      {status.notes ? <p className="app-update-notes">{status.notes}</p> : null}
      {phase === "downloading" ? (
        <div className="app-update-progress">
          <progress
            aria-label="Update download"
            value={status.total_bytes ? status.downloaded_bytes : undefined}
            max={status.total_bytes ?? undefined}
          />
          <span>
            {(status.downloaded_bytes / MB).toFixed(1)} MB
            {status.total_bytes ? ` of ${(status.total_bytes / MB).toFixed(1)} MB` : ""}
          </span>
        </div>
      ) : null}
      {phase === "installing" ? (
        <p className="app-update-note" role="status">
          Installing. Ghost restarts by itself.
        </p>
      ) : null}
      {status.message ? (
        <p className="app-update-note" role="status">
          {status.message}
        </p>
      ) : null}
      {error ? (
        <p className="app-update-note app-update-error" role="alert">
          {error}
        </p>
      ) : null}
      {phase === "restart_required" || phase === "installing" ? null : (
        <>
          <p className="app-update-note">
            {working > 0
              ? `${working === 1 ? "1 chat is" : `${working} chats are`} working. The restart stops ${working === 1 ? "it" : "them"}, and ${working === 1 ? "it picks" : "they pick"} up where ${working === 1 ? "it was" : "they were"}. Drafts are saved first.`
              : "Ghost restarts once. Chats pick up where they were, and drafts are saved first."}
          </p>
          <button
            type="button"
            className="g-button g-button-primary g-button-28 app-update-action"
            data-testid="app-update-start"
            disabled={running}
            onClick={() => void update()}
          >
            {phase === "downloading"
              ? "Downloading…"
              : phase === "failed" || phase === "waiting_for_idle"
                ? "Try again"
                : phase === "ready"
                  ? "Install and restart"
                  : "Update and restart"}
          </button>
        </>
      )}
    </Popover>
  );
}
