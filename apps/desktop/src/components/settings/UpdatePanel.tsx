import { useEffect, useRef, useState } from "react";
import type { UpdateStatus } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import { UPDATE_LABEL, type UpdatesApi, updateBusy } from "../../lib/updates";
import { versionWithBuild } from "../../lib/version";

/**
 * The Updates row of About: the phase as its label, the version under it, and the one next step
 * at right. Progress, release notes, the install question and errors open under the row.
 */
export function UpdatePanel({ api }: { readonly api: UpdatesApi }) {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const alive = useRef(false);
  const operation = useRef(false);
  const epoch = useRef(0);

  useEffect(() => {
    alive.current = true;
    const request = ++epoch.current;
    api.status().then(
      (next) => {
        if (alive.current && epoch.current === request) setStatus(next);
      },
      (failure) => {
        if (alive.current && epoch.current === request) setError(errorMessage(failure));
      },
    );
    return () => {
      alive.current = false;
      epoch.current++;
    };
  }, [api]);

  const phase = status?.phase;
  useEffect(() => {
    if (!busy && (!phase || !updateBusy(phase))) return;
    let canceled = false;
    let reading = false;
    const timer = setInterval(() => {
      if (reading) return;
      reading = true;
      const request = epoch.current;
      api
        .status()
        .then(
          (next) => {
            if (!canceled && epoch.current === request) setStatus(next);
          },
          () => undefined,
        )
        .finally(() => {
          reading = false;
        });
    }, 500);
    return () => {
      canceled = true;
      clearInterval(timer);
    };
  }, [api, busy, phase]);

  const run = async (action: "status" | "check" | "download" | "install") => {
    if (operation.current) return;
    operation.current = true;
    epoch.current++;
    setBusy(true);
    setError(null);
    setConfirm(false);
    try {
      const next = await api[action]();
      if (alive.current) setStatus(next);
    } catch (failure) {
      if (alive.current) setError(errorMessage(failure));
    } finally {
      epoch.current++;
      operation.current = false;
      if (alive.current) setBusy(false);
    }
  };
  const working = busy || (phase !== undefined && updateBusy(phase));
  const action = "g-button g-button-outline g-button-28";
  return (
    <section className="settings-row" aria-label="Application updates" data-setting="Updates">
      <div className="settings-row-text">
        <div>
          <strong role="status">
            {status ? UPDATE_LABEL[status.phase] : "Reading update status…"}
          </strong>
          <p>
            {status?.version ? (
              <>
                Version <span className="settings-mono">{versionWithBuild(status.version)}</span>
              </>
            ) : status?.phase === "disabled" ? (
              "This copy does not update itself."
            ) : status?.current_version ? (
              <>
                You have{" "}
                <span className="settings-mono">{versionWithBuild(status.current_version)}</span>.{" "}
                {status.automatic
                  ? "o7 looks for a newer version a few times a day."
                  : "o7 looks for a newer version only when you ask."}
              </>
            ) : (
              "o7 looks for a newer version a few times a day."
            )}
          </p>
        </div>
      </div>
      {status && phase !== "disabled" && phase !== "restart_required" ? (
        phase === "available" ? (
          <button
            type="button"
            className={action}
            disabled={working}
            onClick={() => void run("download")}
          >
            Download
          </button>
        ) : phase === "ready" || phase === "waiting_for_idle" ? (
          <button
            type="button"
            className={action}
            disabled={working || confirm}
            onClick={() => setConfirm(true)}
          >
            Install update
          </button>
        ) : (
          <button
            type="button"
            className={action}
            disabled={working}
            onClick={() => void run("check")}
          >
            {working ? "Checking…" : "Check for updates"}
          </button>
        )
      ) : null}
      {status?.channel === "staging" ? (
        <p className="settings-note settings-span" role="status">
          Staging build. Updates are restricted to release qualification builds.
        </p>
      ) : null}
      {status?.message ? <p className="settings-note settings-span">{status.message}</p> : null}
      {phase === "downloading" && status ? (
        <div className="settings-download settings-span">
          <progress
            aria-label="Update download"
            value={status.total_bytes ? status.downloaded_bytes : undefined}
            max={status.total_bytes ?? undefined}
          />
          <span>
            {(status.downloaded_bytes / 1_048_576).toFixed(1)} MB
            {status.total_bytes
              ? ` of ${(status.total_bytes / 1_048_576).toFixed(1)} MB`
              : " downloaded"}
          </span>
        </div>
      ) : null}
      {status?.notes ? (
        <details className="settings-disclosure settings-notes settings-span">
          <summary>Release notes</summary>
          <p>{status.notes}</p>
        </details>
      ) : null}
      {confirm ? (
        <div className="settings-confirm settings-span">
          <strong>Install and restart o7?</strong>
          <p>
            Installing stops every chat's agent and closes terminals, then restarts. Chats pick up
            where they were, and drafts are saved first. It waits if a Git change is still running.
          </p>
          <div>
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              onClick={() => setConfirm(false)}
            >
              Keep working
            </button>
            <button
              type="button"
              className="g-button g-button-primary g-button-28"
              onClick={() => void run("install")}
            >
              Install and restart
            </button>
          </div>
        </div>
      ) : null}
      {error ? (
        <div className="settings-error settings-span" role="alert">
          <p>{error}</p>
          <button
            type="button"
            className="g-button g-button-28"
            disabled={working}
            onClick={() => void run("status")}
          >
            Reload update status
          </button>
        </div>
      ) : null}
    </section>
  );
}
