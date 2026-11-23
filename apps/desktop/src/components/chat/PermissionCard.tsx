import { useEffect } from "react";
import type { PermissionDecision, Value } from "../../bindings";
import type { RequestState } from "../../lib/chat/chatStore";
import { permissionResolutionLabel } from "../../lib/chat/present";
import type { PermissionItem } from "../../lib/chat/transcript";
import {
  longPlanBlob,
  type PlanApproval,
  PlanCard,
  planApproval,
  planFromStored,
} from "./PlanCard";

/** Handing an accepted plan to the chat's Build with (lib/roles.ts). */
export interface PlanBuild {
  /** The role as the button names it: "Codex · gpt-6-astra". */
  readonly label: string;
  readonly run: (requestId: string, approval: PlanApproval) => Promise<void>;
}

import { type BlobProps, EventDetails, Locations, ToolContentView } from "./ToolBlock";

/** The most of a request's raw input the card shows; "Show full details" has the rest. */
const RAW_INPUT_CHARS = 2000;

/** A permission request's raw input, as readable text. */
export function rawInputSummary(input: Value | null): string | null {
  if (input === null) return null;
  const text = typeof input === "string" ? input : JSON.stringify(input, null, 2);
  return text.length > RAW_INPUT_CHARS ? `${text.slice(0, RAW_INPUT_CHARS)}…` : text;
}

/**
 * A flat request input (`{ command: "npm test", cwd: "/work" }`) as key and value rows, which read
 * at a glance where raw JSON does not; anything nested, long or large stays the raw text.
 */
export function inputRows(input: Value | null): [string, string][] | null {
  if (input === null || typeof input !== "object" || Array.isArray(input)) return null;
  const entries = Object.entries(input);
  if (entries.length === 0 || entries.length > 8) return null;
  const rows: [string, string][] = [];
  for (const [key, value] of entries) {
    if (value === null || typeof value === "object") return null;
    const text = String(value);
    if (text.length > 240 || text.includes("\n")) return null;
    rows.push([key, text]);
  }
  return rows;
}

/** A decision as `data-test-decision` carries it: the chosen option's id, or `cancelled`. */
export function decisionMark(decision: PermissionDecision): string {
  return decision.type === "cancelled" ? "cancelled" : decision.option_id;
}

/**
 * The agent asks before it acts: title, input, content, one button per option, or Dismiss when
 * it offers none. Buttons disable while the answer is on its way; a resolved card says who closed
 * it (or the choice), and one the agent no longer waits for closes without a resolution.
 */
export function PermissionCard({
  item,
  request,
  onRespond,
  blobs,
  onLoadBlob,
  agentName = "The agent",
  onAddToPlan,
  onAcceptPlanEdited,
  planBuild,
}: {
  readonly item: PermissionItem;
  readonly request: RequestState | undefined;
  /** The option chosen; `null` dismisses the card (the agent hears `cancelled`). */
  readonly onRespond: (optionId: string | null) => void;
  /** Names the agent on a plan card. */
  readonly agentName?: string;
  /** A plan card's ways back: decline it with words the agent keeps planning with. */
  readonly onAddToPlan?: (
    requestId: string,
    declineOptionId: string,
    text: string,
  ) => Promise<void>;
  /** A plan card's Accept edited plan, where the agent takes an edited plan. */
  readonly onAcceptPlanEdited?: (
    requestId: string,
    acceptOptionId: string,
    plan: string,
  ) => Promise<void>;
  /** A plan card's Accept and build with: the chat's Build with takes the accepted plan. */
  readonly planBuild?: PlanBuild;
} & BlobProps) {
  const { request: ask, resolution } = item;
  // A long plan's text is stored apart from its event: it is read in before the card is drawn,
  // page by page, so the plan is never shown as a tool's cut-off input.
  const planBlob = longPlanBlob(ask);
  const stored = planBlob === null ? undefined : blobs.get(planBlob);
  const storedWaiting = planBlob !== null && !stored?.eof && !stored?.loading && !stored?.error;
  useEffect(() => {
    if (planBlob !== null && storedWaiting) onLoadBlob(planBlob);
  }, [planBlob, storedWaiting, onLoadBlob]);
  const plan = planApproval(ask, stored?.eof ? planFromStored(stored.text) : null);
  if (plan === null && planBlob !== null && !stored?.eof && !stored?.error)
    return (
      <div
        className="chat-block"
        data-kind="permission"
        data-testid="item"
        data-test-kind="permission"
        data-test-request-id={ask.request_id}
        aria-busy="true"
      >
        <p className="chat-resolution" data-testid="plan-loading">
          Loading the plan…
        </p>
      </div>
    );
  if (plan)
    return (
      <PlanCard
        item={item}
        approval={plan}
        request={request}
        agentName={agentName}
        onRespond={onRespond}
        onAddToPlan={
          onAddToPlan
            ? (declineOptionId, text) => onAddToPlan(ask.request_id, declineOptionId, text)
            : undefined
        }
        onAcceptEdited={
          onAcceptPlanEdited
            ? (acceptOptionId, edited) => onAcceptPlanEdited(ask.request_id, acceptOptionId, edited)
            : undefined
        }
        build={
          planBuild && resolution === null
            ? { label: planBuild.label, onBuild: () => planBuild.run(ask.request_id, plan) }
            : undefined
        }
      />
    );
  const rows = inputRows(ask.raw_input);
  const raw = rows === null ? rawInputSummary(ask.raw_input) : null;
  const closed = resolution !== null || request?.gone === true;
  const busy = request?.busy === true;
  return (
    <div
      className="chat-block"
      data-kind="permission"
      data-resolved={closed}
      data-testid="item"
      data-test-kind="permission"
      data-test-request-id={ask.request_id}
      data-test-resolved-by={resolution?.by ?? ""}
      data-test-decision={resolution === null ? "" : decisionMark(resolution.decision)}
      data-test-gone={String(request?.gone === true)}
    >
      <div className="chat-block-head">
        <span className="chat-block-title" data-testid="permission-title">
          {ask.title}
        </span>
      </div>
      {rows !== null ? (
        <dl className="chat-kv">
          {rows.map(([key, value]) => (
            <div key={key}>
              <dt>{key}</dt>
              <dd className="ui-mono">{value}</dd>
            </div>
          ))}
        </dl>
      ) : null}
      {raw !== null ? <pre className="ui-pre">{raw}</pre> : null}
      <Locations locations={ask.locations} />
      {ask.content.map((content, index) => (
        <ToolContentView
          // biome-ignore lint/suspicious/noArrayIndexKey: a request's content is positional.
          key={index}
          content={content}
          disclosureKey={`permission:${ask.request_id}:${index}`}
          blobs={blobs}
          onLoadBlob={onLoadBlob}
        />
      ))}
      {ask.truncated ? (
        <EventDetails truncation={ask.truncated} blobs={blobs} onLoadBlob={onLoadBlob} />
      ) : null}
      {resolution !== null ? (
        <p className="chat-resolution" data-testid="permission-resolution">
          {permissionResolutionLabel(resolution, ask.options)}
        </p>
      ) : request?.gone ? (
        <p className="chat-resolution" data-testid="permission-resolution">
          This request is no longer waiting for an answer.
        </p>
      ) : (
        <div className="ui-row">
          {ask.options.map((option) => (
            <button
              key={option.option_id}
              type="button"
              className={option.kind === "allow_once" ? "g-button g-button-primary" : "g-button"}
              data-testid="permission-option"
              data-test-option-id={option.option_id}
              data-test-option-kind={option.kind}
              disabled={busy}
              onClick={() => onRespond(option.option_id)}
            >
              {option.label}
            </button>
          ))}
          {ask.options.length === 0 ? (
            <button
              type="button"
              className="g-button"
              data-testid="permission-dismiss"
              disabled={busy}
              onClick={() => onRespond(null)}
            >
              Dismiss
            </button>
          ) : null}
        </div>
      )}
      {request?.error ? (
        <span role="alert" className="ui-error" data-testid="permission-error">
          {request.error}
        </span>
      ) : null}
    </div>
  );
}
