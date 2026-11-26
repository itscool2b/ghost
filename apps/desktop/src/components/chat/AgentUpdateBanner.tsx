import { AgentMark } from "@ghost/ui/graphite";
import { X } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { AgentMaintenance } from "../../bindings";
import type { AgentUpdate } from "../../lib/agentUpdates";
import { errorMessage } from "../../lib/errors";
import type { TerminalTransport } from "../../lib/terminal";
import { TerminalSheet } from "../TerminalSheet";

/** What the notice needs from the shell: the vendor's update command and a fresh detection. */
export interface AgentUpdateActions {
  maintenance(agentId: string): Promise<AgentMaintenance>;
  update(agentId: string): Promise<void>;
  /** Detects every agent again, so a finished update shows its new version. */
  refresh(): Promise<void>;
  /** Settings > Agents, where every agent's update and installation details are. */
  openAgents?: () => void;
  /**
   * The same update command in Ghost's terminal sheet, for when the background update failed
   * (owner, 7 November 2026: "it says go through with it in the terminal for debugging ... we
   * should make a flow to be able to do that in this ADE directly").
   */
  terminal?: TerminalTransport;
}

type Run =
  | { readonly state: "idle" }
  | { readonly state: "updating" }
  | { readonly state: "failed"; readonly message: string }
  /** The vendor's update finished, yet detection still finds the version it had. */
  | { readonly state: "unchanged" };

/**
 * One agent's newer release, a row in the notifications panel: it stays until its X, and while the
 * agent's chats have a turn under way it waits for them instead of offering Update. An agent whose
 * installation Ghost cannot update (another tool owns it) says why and points to Settings >
 * Agents. A failed update offers its command in Ghost's own terminal, where its output and
 * prompts show, and `onFailed` turns the bell red while it stands.
 */
export function AgentUpdateBanner({
  update,
  working,
  actions,
  onDismiss,
  onFailed,
}: {
  readonly update: AgentUpdate;
  /** The agent's chats with a turn under way now. */
  readonly working: number;
  readonly actions: AgentUpdateActions;
  readonly onDismiss: () => void;
  /** Whether the last update failed, told when that changes and `false` when the notice goes. */
  readonly onFailed?: (failed: boolean) => void;
}) {
  const text = useId();
  const [maintenance, setMaintenance] = useState<AgentMaintenance | "failed" | null>(null);
  const [run, setRun] = useState<Run>({ state: "idle" });
  const [terminal, setTerminal] = useState(false);
  const terminalExit = useRef<number | null>(null);
  const failedChanged = useRef(onFailed);
  failedChanged.current = onFailed;
  const failed = run.state === "failed";
  useEffect(() => {
    failedChanged.current?.(failed);
  }, [failed]);
  useEffect(() => () => failedChanged.current?.(false), []);

  // What this installation can do, asked again for each release and installation. A new version
  // means the last run's outcome no longer applies.
  // biome-ignore lint/correctness/useExhaustiveDependencies: Another version or path asks again.
  useEffect(() => {
    let live = true;
    setMaintenance(null);
    setRun({ state: "idle" });
    actions.maintenance(update.agentId).then(
      (answer) => {
        if (live) setMaintenance(answer);
      },
      () => {
        if (live) setMaintenance("failed");
      },
    );
    return () => {
      live = false;
    };
  }, [actions, update.agentId, update.version, update.path]);

  /** A finished update: the notice goes once detection sees the new version. */
  const verify = async () => {
    setRun({ state: "updating" });
    // Until the refresh answers it reads as still updating; if the old version is still what
    // Ghost finds, the update went to another copy than the one it runs.
    await actions.refresh().catch(() => {});
    setRun({ state: "unchanged" });
  };

  const start = async () => {
    setRun({ state: "updating" });
    try {
      await actions.update(update.agentId);
    } catch (reason) {
      setRun({ state: "failed", message: errorMessage(reason) });
      return;
    }
    await verify();
  };

  const updatable =
    maintenance !== null && maintenance !== "failed" && !maintenance.update_unavailable;
  const blocked =
    maintenance === "failed"
      ? "Ghost couldn't check how to update it."
      : (maintenance?.update_unavailable ?? null);
  const waiting =
    working > 0
      ? `Update once its ${working === 1 ? "working chat finishes" : `${working} working chats finish`}.`
      : null;
  const detail =
    run.state === "unchanged"
      ? `The update finished, but Ghost still finds ${update.version}${
          update.path ? ` at ${update.path}` : ""
        }. The newer release may not be out for this installation yet, or another copy was updated.`
      : (blocked ?? waiting);
  const canRun = updatable && working === 0 && run.state !== "unchanged";
  const inTerminal = canRun && run.state === "failed" && actions.terminal !== undefined;

  // The agent's mark, what is new, why it waits (if it does), then its actions.
  return (
    <li
      className="chat-banner ui-toast"
      data-level={run.state === "failed" ? "error" : undefined}
      data-testid="agent-update-banner"
      data-agent={update.agentId}
    >
      <span className="ui-toast-mark" aria-hidden="true">
        <AgentMark agent={update.agentId} size={20} />
      </span>
      <div className="ui-toast-body">
        <div id={text} role="status">
          <p className="ui-toast-title">
            {update.name} {update.latest} is available
          </p>
          <p className="ui-toast-detail">
            You have {update.version}.{detail ? ` ${detail}` : null}
          </p>
        </div>
        {run.state === "failed" ? (
          <p role="alert" className="ui-error ui-toast-detail">
            The update failed. {run.message}
            {inTerminal ? " Run it in the terminal to see why." : null}
          </p>
        ) : null}
        <div className="ui-toast-actions">
          {inTerminal ? (
            <button
              type="button"
              className="g-button g-button-primary g-button-24"
              aria-describedby={text}
              onClick={() => {
                terminalExit.current = null;
                setTerminal(true);
              }}
            >
              Run in terminal
            </button>
          ) : null}
          {canRun ? (
            <button
              type="button"
              className={`g-button ${inTerminal ? "" : "g-button-primary "}g-button-24`}
              aria-describedby={text}
              disabled={run.state === "updating"}
              onClick={() => void start()}
            >
              {run.state === "updating"
                ? "Updating…"
                : run.state === "failed"
                  ? "Try again"
                  : "Update"}
            </button>
          ) : null}
          {(blocked !== null || run.state === "unchanged") && actions.openAgents ? (
            <button
              type="button"
              className="g-button g-button-24"
              aria-describedby={text}
              onClick={actions.openAgents}
            >
              Agents settings
            </button>
          ) : null}
        </div>
      </div>
      <button
        type="button"
        className="g-button g-button-ghost g-button-24 g-button-icon ui-toast-close"
        aria-label={`Dismiss the ${update.name} update notice`}
        title="Dismiss until Ghost starts again"
        onClick={onDismiss}
      >
        <X size={14} aria-hidden="true" />
      </button>
      {terminal && actions.terminal ? (
        <TerminalSheet
          agentId={update.agentId}
          agentName={update.name}
          purpose="update"
          transport={actions.terminal}
          autoStart
          onFinished={(code) => {
            terminalExit.current = code;
          }}
          onClose={() => {
            setTerminal(false);
            // Checked once the person has read the output: a passing update makes this notice go.
            if (terminalExit.current === 0) void verify();
          }}
        />
      ) : null}
    </li>
  );
}
