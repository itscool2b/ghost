import { Check, CornerUpLeft, Pencil } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import type { PermissionOption, PermissionRequest } from "../../bindings";
import type { RequestState } from "../../lib/chat/chatStore";
import {
  planEdited,
  planFeedback,
  planMeta,
  planParts,
  planRevision,
  planSections,
} from "../../lib/chat/plan";
import { permissionResolutionLabel } from "../../lib/chat/present";
import type { PermissionItem } from "../../lib/chat/transcript";
import { errorMessage } from "../../lib/errors";
import { Answer } from "../Markdown";
import { commandKey, Keys } from "./Keys";

export { planFeedback } from "../../lib/chat/plan";

/** The plan an agent asks to leave plan mode with, and the options that accept or decline it. */
export interface PlanApproval {
  readonly plan: string;
  readonly accept: PermissionOption;
  readonly decline: PermissionOption;
}

/**
 * A request to leave plan mode with a written plan (Claude Code's ExitPlanMode): a mode switch
 * whose input carries the plan, offering both an allow and a reject option. Anything else stays
 * an ordinary permission card.
 */
export function planApproval(request: PermissionRequest): PlanApproval | null {
  if (request.kind !== "switch_mode") return null;
  const input = request.raw_input;
  const plan =
    input && typeof input === "object" && !Array.isArray(input) && "plan" in input
      ? input.plan
      : null;
  if (typeof plan !== "string" || !plan.trim()) return null;
  const accept = request.options.find((option) => option.kind === "allow_once");
  const decline = request.options.find((option) => option.kind === "reject_once");
  return accept && decline ? { plan, accept, decline } : null;
}

/** What the card is doing: showing the plan, taking edits to its text, or taking a note. */
type Mode = "review" | "editing" | "noting";

/**
 * A plan, presented as the document it is and not as a tool's request (owner, 22 November 2026:
 * "make this whole UI for presenting the plan more sexy, don't make it look like a tool call.
 * Have options to accept or decline the plan with edits"). Its own title leads in title type, one
 * line says whose it is and how many steps, its sections are a row of chips that jump to them,
 * and the plan is set as prose. Four ways on:
 *
 * - **Accept plan** lets the agent implement it (or hands it to the chat's Build with).
 * - **Edit** opens the plan's own text. An agent that takes an edited plan is told the edited
 *   text is the approved plan (Accept edited plan); any agent can be sent it back to keep
 *   planning with (Send back edited).
 * - **Request changes** declines it with a note the agent keeps planning with.
 * - **Decline** declines it and says nothing more.
 */
export function PlanCard({
  item,
  approval,
  request,
  agentName,
  onRespond,
  onAddToPlan,
  onAcceptEdited,
  build,
}: {
  readonly item: PermissionItem;
  readonly approval: PlanApproval;
  readonly request: RequestState | undefined;
  readonly agentName: string;
  readonly onRespond: (optionId: string | null) => void;
  /** Declines the plan and sends `text` with it; resolves once the text is on its way. */
  readonly onAddToPlan?: (declineOptionId: string, text: string) => Promise<void>;
  /**
   * Accepts the plan as the person edited it, where the agent takes one
   * (`capabilities.plan_edits`); resolves once the agent has it.
   */
  readonly onAcceptEdited?: (acceptOptionId: string, plan: string) => Promise<void>;
  /**
   * The chat's Build with, when accepting can hand the plan to it (lib/roles.ts): its name,
   * and the hand-over, which resolves once the builder has the plan.
   */
  readonly build?: { readonly label: string; readonly onBuild: () => Promise<void> };
}) {
  const titleId = useId();
  const noteId = useId();
  const body = useRef<HTMLDivElement>(null);
  const card = useRef<HTMLElement>(null);
  const [mode, setMode] = useState<Mode>("review");
  const [draft, setDraft] = useState(approval.plan);
  const [note, setNote] = useState("");
  const [working, setWorking] = useState<"hand" | "send" | "accept" | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const { request: ask, resolution } = item;
  const closed = resolution !== null || request?.gone === true;
  const busy = request?.busy === true || working !== null;
  const accepted =
    resolution?.decision.type === "selected" &&
    resolution.decision.option_id === approval.accept.option_id;
  // An accepted plan shows as the person left it: their edits are the plan that was approved.
  const approvedEdit =
    accepted && resolution?.decision.type === "selected"
      ? (resolution.decision.plan ?? null)
      : null;
  const shown = approvedEdit ?? approval.plan;
  const parts = planParts(shown);
  const sections = planSections(parts.body);
  const edited = planEdited(approval.plan, draft);

  /** Runs one of the card's ways on; a failure is said under the card and the card stays. */
  const run = async (what: "hand" | "send" | "accept", action: () => Promise<void>) => {
    setWorking(what);
    setFailure(null);
    try {
      await action();
      setMode("review");
      setNote("");
    } catch (reason) {
      setFailure(errorMessage(reason));
    } finally {
      setWorking(null);
    }
  };
  const hand = () => {
    if (build) void run("hand", build.onBuild);
  };
  const sendNote = () => {
    if (onAddToPlan && note.trim())
      void run("send", () => onAddToPlan(approval.decline.option_id, planFeedback(note)));
  };
  const sendEdited = () => {
    if (onAddToPlan && edited)
      void run("send", () => onAddToPlan(approval.decline.option_id, planRevision(draft)));
  };
  const acceptEdited = () => {
    if (onAcceptEdited && edited)
      void run("accept", () => onAcceptEdited(approval.accept.option_id, draft.trim()));
  };
  const leave = () => {
    setMode("review");
    setFailure(null);
  };
  /** Ctrl or Command with Enter takes the filled action of whatever the card is doing. */
  const keys = (event: KeyboardEvent) => {
    if (closed || busy) return;
    if (event.key === "Escape" && mode !== "review") {
      event.preventDefault();
      event.stopPropagation();
      leave();
      return;
    }
    if (event.key !== "Enter" || !(event.metaKey || event.ctrlKey) || event.altKey) return;
    event.preventDefault();
    if (mode === "noting") sendNote();
    else if (mode === "editing") (onAcceptEdited ? acceptEdited : sendEdited)();
    else if (build) hand();
    else onRespond(approval.accept.option_id);
  };
  // The shortcuts answer wherever focus is in the sheet that holds the card: a plan that takes
  // the composer's place gets the sheet's focus, not the card's own.
  const latest = useRef(keys);
  latest.current = keys;
  useEffect(() => {
    const host = card.current?.closest<HTMLElement>(".chat-permission-dock") ?? card.current;
    if (!host) return;
    const listen = (event: KeyboardEvent) => latest.current(event);
    host.addEventListener("keydown", listen);
    return () => host.removeEventListener("keydown", listen);
  }, []);
  const jump = (index: number) => {
    // The plan's title is the card's own heading, so every heading in the body is a section.
    body.current
      ?.querySelectorAll("h1, h2, h3, h4, h5, h6")
      [index]?.scrollIntoView({ block: "start", behavior: "smooth" });
  };
  const outcome =
    resolution === null
      ? request?.gone
        ? "This plan is no longer waiting for an answer."
        : null
      : accepted
        ? approvedEdit === null
          ? "Plan accepted."
          : "Plan accepted with your edits."
        : resolution.decision.type === "selected" &&
            resolution.decision.option_id === approval.decline.option_id
          ? "Sent back to keep planning."
          : permissionResolutionLabel(resolution, ask.options);
  const canChange = onAddToPlan !== undefined || onAcceptEdited !== undefined;
  return (
    <section
      ref={card}
      className="chat-plan"
      aria-labelledby={titleId}
      data-kind="plan-approval"
      data-mode={closed ? undefined : mode}
      data-resolved={closed}
      data-testid="item"
      data-test-kind="permission"
      data-test-request-id={ask.request_id}
      data-test-resolved-by={resolution?.by ?? ""}
      data-test-decision={
        resolution === null
          ? ""
          : resolution.decision.type === "cancelled"
            ? "cancelled"
            : resolution.decision.option_id
      }
      data-test-gone={String(request?.gone === true)}
    >
      <header className="chat-plan-head">
        <div className="chat-plan-heading">
          <h3 id={titleId} className="chat-plan-title" data-testid="permission-title">
            {parts.title ?? "The plan"}
          </h3>
          <p className="chat-plan-meta">{planMeta(agentName, shown)}</p>
        </div>
        {!closed && mode === "review" && canChange ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-28 chat-plan-edit"
            data-testid="plan-edit"
            disabled={busy}
            onClick={() => {
              setDraft(approval.plan);
              setMode("editing");
            }}
          >
            <Pencil size={14} aria-hidden="true" />
            Edit
          </button>
        ) : null}
      </header>
      {sections.length > 1 && mode !== "editing" ? (
        <nav className="chat-plan-sections" aria-label="Sections of the plan">
          {sections.map((name, index) => (
            <button
              // biome-ignore lint/suspicious/noArrayIndexKey: Two sections may share a name; their order is their identity.
              key={`${index}:${name}`}
              type="button"
              className="chat-plan-section"
              onClick={() => jump(index)}
            >
              {name}
            </button>
          ))}
        </nav>
      ) : null}
      {mode === "editing" && !closed ? (
        <textarea
          className="chat-plan-editor"
          aria-label="The plan, as Markdown"
          data-testid="plan-editor"
          value={draft}
          disabled={busy}
          spellCheck={false}
          // biome-ignore lint/a11y/noAutofocus: the person just asked to edit this text.
          autoFocus
          onChange={(event) => setDraft(event.currentTarget.value)}
        />
      ) : (
        <div ref={body} className="chat-plan-body" data-testid="plan-body">
          <Answer text={parts.body || shown} />
        </div>
      )}
      {outcome !== null ? (
        <p
          className="chat-plan-outcome"
          data-accepted={accepted || undefined}
          data-testid="permission-resolution"
        >
          {accepted ? (
            <Check size={14} aria-hidden="true" />
          ) : (
            <CornerUpLeft size={14} aria-hidden="true" />
          )}
          {outcome}
        </p>
      ) : (
        <footer className="chat-plan-foot">
          {mode === "noting" ? (
            <form
              className="chat-plan-note"
              onSubmit={(event) => {
                event.preventDefault();
                sendNote();
              }}
            >
              <label htmlFor={noteId}>What should change?</label>
              <textarea
                id={noteId}
                className="g-textarea"
                rows={3}
                value={note}
                disabled={busy}
                placeholder={`${agentName} keeps planning with your note.`}
                // biome-ignore lint/a11y/noAutofocus: the person just asked to write this note.
                autoFocus
                onChange={(event) => setNote(event.currentTarget.value)}
              />
              <div className="chat-plan-actions">
                <button
                  type="submit"
                  className="g-button g-button-primary"
                  data-testid="plan-send-note"
                  disabled={busy || !note.trim()}
                >
                  {working === "send" ? "Sending…" : "Send back"}
                  <Keys keys={[commandKey(), "↵"]} />
                </button>
                <button
                  type="button"
                  className="g-button g-button-ghost"
                  disabled={busy}
                  onClick={leave}
                >
                  Cancel
                </button>
              </div>
            </form>
          ) : mode === "editing" ? (
            <>
              <div className="chat-plan-actions">
                {onAcceptEdited ? (
                  <button
                    type="button"
                    className="g-button g-button-primary"
                    data-testid="plan-accept-edited"
                    disabled={busy || !edited}
                    onClick={acceptEdited}
                  >
                    {working === "accept" ? "Accepting…" : "Accept edited plan"}
                    <Keys keys={[commandKey(), "↵"]} />
                  </button>
                ) : null}
                {onAddToPlan ? (
                  <button
                    type="button"
                    className={onAcceptEdited ? "g-button" : "g-button g-button-primary"}
                    data-testid="plan-send-edited"
                    disabled={busy || !edited}
                    onClick={sendEdited}
                  >
                    {working === "send" ? "Sending…" : "Send back edited"}
                    {onAcceptEdited ? null : <Keys keys={[commandKey(), "↵"]} />}
                  </button>
                ) : null}
                <button
                  type="button"
                  className="g-button g-button-ghost"
                  disabled={busy}
                  onClick={leave}
                >
                  Discard edits
                </button>
              </div>
              <p className="chat-plan-hint">
                {onAcceptEdited
                  ? `Accepting tells ${agentName} your version is the approved plan, and it starts on it.`
                  : `${agentName} cannot be handed an edited plan to start on.`}
                {onAddToPlan
                  ? ` Sending it back shows ${agentName} your version, and it keeps planning.`
                  : ""}
              </p>
            </>
          ) : (
            <>
              <div className="chat-plan-actions">
                {build ? (
                  <button
                    type="button"
                    className="g-button g-button-primary"
                    data-testid="plan-build"
                    disabled={busy}
                    onClick={hand}
                  >
                    {working === "hand" ? "Handing over…" : `Accept and build with ${build.label}`}
                    <Keys keys={[commandKey(), "↵"]} />
                  </button>
                ) : null}
                <button
                  type="button"
                  className={build ? "g-button" : "g-button g-button-primary"}
                  data-testid="permission-option"
                  data-test-option-id={approval.accept.option_id}
                  data-test-option-kind={approval.accept.kind}
                  disabled={busy}
                  onClick={() => onRespond(approval.accept.option_id)}
                >
                  Accept plan
                  {build ? null : <Keys keys={[commandKey(), "↵"]} />}
                </button>
                {onAddToPlan ? (
                  <button
                    type="button"
                    className="g-button"
                    data-testid="plan-add"
                    disabled={busy}
                    onClick={() => setMode("noting")}
                  >
                    Request changes
                  </button>
                ) : null}
                <button
                  type="button"
                  className="g-button g-button-ghost chat-plan-decline"
                  data-testid="permission-option"
                  data-test-option-id={approval.decline.option_id}
                  data-test-option-kind={approval.decline.kind}
                  disabled={busy}
                  onClick={() => onRespond(approval.decline.option_id)}
                >
                  Decline
                </button>
              </div>
              <p className="chat-plan-hint">
                {build
                  ? `Accept and build hands the plan to ${build.label}, this chat's Build with. Accept plan lets ${agentName} implement it.`
                  : `Accepting lets ${agentName} start implementing.`}
                {canChange ? " Edit changes the plan's own words before you answer." : ""}
              </p>
            </>
          )}
        </footer>
      )}
      {request?.error ? (
        <span role="alert" className="ui-error" data-testid="permission-error">
          {request.error}
        </span>
      ) : null}
      {failure ? (
        <span role="alert" className="ui-error" data-testid="plan-build-error">
          {failure}
        </span>
      ) : null}
    </section>
  );
}
