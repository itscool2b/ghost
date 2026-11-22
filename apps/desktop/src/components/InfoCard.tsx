import { Fragment, type ReactNode, useState } from "react";
import type { AppInfo } from "../bindings";
import { errorMessage } from "../lib/errors";
import {
  fileManagerName,
  glassLabel,
  osName,
  outcomeLabel,
  startupTimings,
} from "../lib/format";
import { revealDataDir } from "../lib/ipc";
import { RecoveryNotice } from "./RecoveryNotice";

/**
 * What this launch is running on: versions, device, profile, database, glass, data folder, and
 * the notices. `onRecoveryDismissed` runs once the shell has acknowledged the database repair.
 */
export function InfoCard({
  info,
  onRecoveryDismissed,
}: {
  readonly info: AppInfo;
  readonly onRecoveryDismissed: () => void;
}) {
  return (
    <section className="diagnostics-panel info-card">
      <h2 className="panel-title">This launch</h2>
      <dl className="facts">
        <dt>o7</dt>
        <dd>{info.app_version}</dd>
        <dt>Tauri</dt>
        <dd>{info.tauri_version}</dd>
        <dt>System</dt>
        <dd>
          {osName(info.os)} {info.os_version} <span className="muted">{info.arch}</span>
        </dd>
        <dt>Device</dt>
        <dd>{info.device?.name ?? <span className="muted">Unavailable</span>}</dd>
        <dt>Profile</dt>
        <dd>
          {info.profile ? (
            <>
              {info.profile.display_name} <span className="muted">{info.profile.kind}</span>
            </>
          ) : (
            <span className="muted">Unavailable</span>
          )}
        </dd>
        <dt>Database</dt>
        <dd>
          {info.db ? (
            <>
              Schema v{info.db.schema_version}{" "}
              <span className="muted">{outcomeLabel(info.db.outcome)}</span>{" "}
              <span className="muted" data-testid="db-timings">
                {startupTimings(info.db)}
              </span>
            </>
          ) : (
            <span className="muted">Unavailable</span>
          )}
        </dd>
        <dt>Glass</dt>
        <dd>{glassLabel(info.glass)}</dd>
        <dt>Data</dt>
        <dd>
          {info.data_dir ? (
            <DataPath path={info.data_dir} os={info.os} />
          ) : (
            <span className="muted">Unknown</span>
          )}
        </dd>
      </dl>
      {info.notices.length > 0 || info.recovery_notice !== null ? (
        <Notices notices={info.notices}>
          {info.recovery_notice !== null ? (
            <RecoveryNotice message={info.recovery_notice} onDismissed={onRecoveryDismissed} />
          ) : null}
        </Notices>
      ) : null}
    </section>
  );
}

/** A path that wraps after its separators rather than in the middle of a folder name. */
function BreakablePath({ path }: { readonly path: string }) {
  return path.split(/(?<=[\\/])/).map((part, index) => (
    // biome-ignore lint/suspicious/noArrayIndexKey: segments are positional within one path.
    <Fragment key={index}>
      {part}
      <wbr />
    </Fragment>
  ));
}

/**
 * The data folder's path with a "Show in Finder" button. The button reveals the folder the shell
 * started with (`reveal_data_dir` takes no path); `path` is only what is displayed.
 */
export function DataPath({
  path,
  os,
  prominent = false,
}: {
  readonly path: string;
  readonly os: string;
  /** A full-size button (the error screen) instead of a quiet inline one (the info card). */
  readonly prominent?: boolean;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <span className="data-path" data-prominent={prominent || undefined}>
      <code title={path}>
        <BreakablePath path={path} />
      </code>
      <button
        type="button"
        className={prominent ? "g-button g-button-28" : "g-button g-button-ghost g-button-28"}
        onClick={() => {
          setProblem(null);
          revealDataDir().catch((error: unknown) => setProblem(errorMessage(error)));
        }}
      >
        Show in {fileManagerName(os)}
      </button>
      {problem ? (
        <span role="alert" className="data-path-problem">
          {problem}
        </span>
      ) : null}
    </span>
  );
}

/** The notices list. `children` go first: notices with controls of their own (`RecoveryNotice`). */
export function Notices({
  notices,
  children,
}: {
  readonly notices: readonly string[];
  readonly children?: ReactNode;
}) {
  return (
    <ul className="notices" aria-label="Notices">
      {children}
      {notices.map((notice) => (
        <li key={notice} className="notice">
          <span className="notice-mark" aria-hidden="true" />
          <span className="notice-text">{notice}</span>
        </li>
      ))}
    </ul>
  );
}
