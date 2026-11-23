import { Check, ListChecks, ShieldCheck, ShieldQuestion } from "lucide-react";
import type { Autonomy, SessionControlChange, SessionLists, SessionStatus } from "../../bindings";
import { AUTONOMIES } from "../../lib/chat/present";
import { movePickerFocus } from "./PickerChoices";
import { closePopover, Popover } from "./Popover";

const ACCESS_LABEL: Record<Autonomy, string> = {
  ask: "Ask for approval",
  auto_edit: "Allow edits",
  full_auto: "Full access",
};
/** What a change kept for the next start says while the agent is not running. */
const APPLIES_AT_START = "Applies when the agent starts.";
const ACCESS_DESCRIPTION: Record<Autonomy, string> = {
  ask: "Review actions the agent asks permission to perform.",
  auto_edit: "Allow file edits. Other actions follow the agent's approval policy.",
  full_auto: "Allow the agent to work without approval prompts where supported.",
};

export function AccessPicker({
  lists,
  autonomy,
  saving,
  controlSaving = null,
  transitionBlocked = false,
  status,
  error,
  onSetAutonomy,
  onSetControl,
}: {
  readonly lists: SessionLists | null;
  readonly autonomy: Autonomy;
  readonly saving: Autonomy | null;
  readonly controlSaving?: SessionControlChange | null;
  readonly transitionBlocked?: boolean;
  readonly status: SessionStatus;
  readonly error: string | null;
  readonly onSetAutonomy: (value: Autonomy) => void;
  readonly onSetControl?: (change: SessionControlChange) => void;
}) {
  const native = onSetControl ? lists?.controls : null;
  const access = native?.access;
  const workflow = native?.workflow;
  const supported = lists?.capabilities?.autonomy_levels ?? [];
  const fallback = AUTONOMIES.filter((value) => supported.includes(value));
  const hasNativeAccess = Boolean(access?.choices?.length);
  const choices =
    hasNativeAccess && access?.choices
      ? access.choices
      : fallback.map((value) => ({
          value,
          name: ACCESS_LABEL[value],
          description: ACCESS_DESCRIPTION[value],
          legacy_autonomy: value,
        }));
  // Only a running agent can say what it offers. A chat whose agent is not running (one opened
  // after a restart, a new chat whose agent is starting) names the access its start asks for;
  // a stopped one takes a change for that start. A starting agent already has its setup, so a
  // change waits until it is ready.
  const running = status === "ready" || status === "running" || status === "cancelling";
  const waiting = status === "stopped" || status === "failed" || status === "auth_required";
  // The lists came from the running agent: they carry its capabilities. Lists fetched before it
  // started (the status turns ready a moment before they are fetched again) say nothing of it.
  const listed = running && lists?.capabilities != null;
  const current = hasNativeAccess ? access?.effective : (lists?.autonomy.requested ?? autonomy);
  const pending =
    controlSaving?.type === "access" ? controlSaving.value : (access?.pending ?? saving);
  const workflowPending = controlSaving?.type === "workflow" || workflow?.pending != null;
  const displayed = pending ?? current;
  // A native setting no known choice names stays unnamed rather than shown as another level.
  const requestedNative = access?.requested && !access.requested.startsWith("legacy:");
  const startLabel = requestedNative
    ? "Access"
    : ACCESS_LABEL[lists?.autonomy.requested ?? autonomy];
  // A level on its way that the agent's own choices do not name (chosen from the levels shown
  // before they arrived) is named as the level.
  const pendingLevel = pending?.startsWith("legacy:")
    ? ACCESS_LABEL[pending.slice("legacy:".length) as Autonomy]
    : undefined;
  const label =
    choices.find((choice) => choice.value === displayed)?.name ??
    pendingLevel ??
    (!listed
      ? startLabel
      : hasNativeAccess || choices.length === 0
        ? "Access unavailable"
        : ACCESS_LABEL[(displayed as Autonomy) ?? autonomy]);
  const planning = workflow?.effective === "plan";
  const displayedWorkflow =
    controlSaving?.type === "workflow"
      ? controlSaving.value
      : (workflow?.pending ?? workflow?.effective);
  const workflowLabel =
    workflow?.choices?.find((choice) => choice.value === displayedWorkflow)?.name ??
    (listed ? "Mode unavailable" : "Mode");
  const startNote =
    choices.length === 0
      ? null
      : waiting
        ? APPLIES_AT_START
        : status === "starting"
          ? "Access opens once the agent has started."
          : null;
  // The session says a change kept for the start "applies when the agent starts"; the line above
  // says so for as long as it is true, and not once the start is under way.
  const accessNote = [access?.note, lists?.autonomy.note].find(
    (note) => note && note !== APPLIES_AT_START,
  );
  // Executing is the ordinary state, so its control is a quiet icon; Plan names itself, lit.
  const planShown = displayedWorkflow === "plan";
  return (
    <div
      className="chat-access-controls"
      data-testid="picker-autonomy"
      data-test-requested={lists?.autonomy.requested ?? autonomy}
      data-test-native-mode={access?.native_mode ?? lists?.autonomy.native_mode ?? ""}
      data-test-saving={pending !== null}
      data-test-effective={current ?? ""}
    >
      <Popover
        label="Access level"
        triggerLabel={`${label}. Access level`}
        className="chat-access-popover"
        trigger={
          <>
            {current === "full_auto" || current === "legacy:full_auto" ? (
              <ShieldCheck size={14} />
            ) : (
              <ShieldQuestion size={14} />
            )}
            <span>{label}</span>
            {pending ? <span className="chat-control-pending">Saving…</span> : null}
          </>
        }
      >
        <header className="chat-control-heading">
          <strong>Access level</strong>
          <p>Controls the permissions used by this agent.</p>
        </header>
        <fieldset className="picker-choices" aria-label="Access level" onKeyDown={movePickerFocus}>
          {choices.map((choice) => (
            <button
              key={choice.value}
              type="button"
              aria-pressed={current === choice.value}
              disabled={
                !(status === "ready" || waiting) ||
                transitionBlocked ||
                pending !== null ||
                controlSaving !== null ||
                (hasNativeAccess && planning)
              }
              data-testid="picker-access-choice"
              data-picker-value={choice.value}
              data-legacy-autonomy={choice.legacy_autonomy ?? undefined}
              onClick={(event) => {
                if (hasNativeAccess && onSetControl)
                  onSetControl({ type: "access", value: choice.value });
                else if (choice.legacy_autonomy) onSetAutonomy(choice.legacy_autonomy);
                closePopover(event.currentTarget, true);
              }}
            >
              <span className="picker-choice-copy">
                <strong>{choice.name}</strong>
                <small>{choice.description}</small>
              </span>
              {current === choice.value ? <Check size={14} aria-label="Selected" /> : null}
            </button>
          ))}
        </fieldset>
        {!choices.length ? (
          <p className="chat-picker-empty">
            {listed
              ? "This connection does not offer access controls."
              : "Access controls load when the agent starts."}
          </p>
        ) : null}
        <div className="chat-picker-note">
          {hasNativeAccess && planning ? <p>Approve execution before changing access.</p> : null}
          {status === "running" || transitionBlocked ? (
            <p>Change access when the agent is idle with no queued messages or pending requests.</p>
          ) : null}
          {startNote ? <p>{startNote}</p> : null}
          {accessNote ? <p>{accessNote}</p> : null}
          {(access?.native_mode ?? lists?.autonomy.native_mode) ? (
            <p>Native setting: {access?.native_mode ?? lists?.autonomy.native_mode}</p>
          ) : null}
          {current?.includes("full_auto") ? (
            <p>Automatic checkpoints are optional in Settings &gt; General.</p>
          ) : null}
          {error ? (
            <p className="ui-error" role="alert" data-testid="picker-error">
              {error}
            </p>
          ) : null}
        </div>
      </Popover>
      <Popover
        label="Working mode"
        className={`chat-workflow-popover${planShown ? " chat-workflow-plan" : ""}`}
        triggerLabel={`${workflowLabel}. Working mode`}
        tip={planShown ? undefined : `Working mode: ${workflowLabel}`}
        trigger={
          <>
            <ListChecks size={14} />
            {planShown ? <span>{workflowLabel}</span> : null}
            {workflowPending ? <span className="chat-control-pending">Saving…</span> : null}
          </>
        }
      >
        <header className="chat-control-heading">
          <strong>Working mode</strong>
          <p>Use the agent's native planning workflow.</p>
        </header>
        <fieldset className="picker-choices" aria-label="Working mode" onKeyDown={movePickerFocus}>
          {workflow?.choices?.map((choice) => (
            <button
              key={choice.value}
              type="button"
              aria-pressed={workflow?.effective === choice.value}
              disabled={
                !(status === "ready" || waiting) ||
                workflowPending ||
                controlSaving !== null ||
                transitionBlocked
              }
              onClick={(event) => {
                onSetControl?.({
                  type: "workflow",
                  value: choice.value,
                  implementation_approved: planning && choice.value === "execute",
                });
                closePopover(event.currentTarget, true);
              }}
            >
              <span className="picker-choice-copy">
                <strong>
                  {planning && choice.value === "execute" ? "Approve and execute" : choice.name}
                </strong>
                <small>{choice.description}</small>
              </span>
              {workflow?.effective === choice.value ? (
                <Check size={14} aria-label="Selected" />
              ) : null}
            </button>
          ))}
        </fieldset>
        {!workflow?.choices?.length ? (
          <p className="chat-picker-empty">
            {listed
              ? "Native Plan mode is not available on this connection."
              : "Modes load when the agent starts."}
          </p>
        ) : null}
        {workflow?.note ? <p className="chat-picker-note">{workflow?.note}</p> : null}
        {status === "running" ? (
          <p className="chat-picker-note">Change mode after the current turn finishes.</p>
        ) : null}
        {waiting && workflow?.choices?.length ? (
          <p className="chat-picker-note">{APPLIES_AT_START}</p>
        ) : null}
        {status === "starting" && workflow?.choices?.length ? (
          <p className="chat-picker-note">Change mode once the agent has started.</p>
        ) : null}
        {transitionBlocked ? (
          <p className="chat-picker-note">
            Resolve pending requests and queued messages before changing mode.
          </p>
        ) : null}
      </Popover>
    </div>
  );
}
