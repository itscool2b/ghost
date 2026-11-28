import { GitFork, ReceiptText, RotateCcw } from "lucide-react";
import { Fragment, memo, useRef } from "react";
import type { PlanEntry, UiAnswer } from "../../bindings";
import type { RequestState } from "../../lib/chat/chatStore";
import { costLabel, type ErrorActions, tokensLabel } from "../../lib/chat/present";
import {
  type RenderedItem,
  type RenderedSegment,
  stopReasonLabel,
} from "../../lib/chat/transcript";
import { formatDuration } from "../../lib/format";
import { Answer } from "../Markdown";
import type { ConversationIntent } from "./ConversationDialog";
import { CopyAction } from "./CopyAction";
import { ErrorCard } from "./ErrorCard";
import { PermissionCard, type PlanBuild } from "./PermissionCard";
import { QuestionCard } from "./QuestionCard";
import { ReadAloud } from "./ReadAloud";
import { ThoughtRow } from "./ThoughtRow";
import { type BlobProps, ToolBlock } from "./ToolBlock";
import { UserMessage } from "./UserMessage";
import { type LiveTail, LiveTailLine } from "./WorkingLine";

/** What the items of a turn act on. */
export interface TurnActions extends BlobProps {
  readonly onContinue?: (intent: ConversationIntent) => void;
  readonly requests: ReadonlyMap<string, RequestState>;
  readonly onRespondPermission: (requestId: string, optionId: string | null) => void;
  /** A plan card's Request changes and Send back edited (PlanCard.tsx). */
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
  /** The agent reads a note with the declined plan itself (`capabilities.plan_notes`). */
  readonly planNotes?: boolean;
  /** A plan card's Accept and build with, when the chat's Build with can take the plan. */
  readonly planBuild?: PlanBuild;
  /** The chat's agent, as plan cards name it. */
  readonly agentName?: string;
  readonly onRespondUi: (requestId: string, answer: UiAnswer | null) => void;
  /** The actions of an error card. */
  readonly errorActions: (
    key: string,
    kind: Parameters<typeof ErrorCard>[0]["item"]["errorKind"],
  ) => ErrorActions;
  readonly onStart: (fresh: boolean) => void;
  readonly onShowLog: () => void;
  /** Reviews recreating the chat's deleted worktree folder (Git workspace's recreate flow). */
  readonly onRecreateFolder?: () => void;
  /** The session has no process: a turn interrupted when Ghost closed offers Resume session. */
  readonly canResume: boolean;
  /** Opens the Changes panel: the Review beside a finished turn's changed files. */
  readonly onOpenChanges?: () => void;
  /** Rewind on a sent message: Changes against the files before that turn, all selected. */
  readonly onRewind?: (turnId: string) => void;
}

/** One turn (or the events between turns): its items in order, its plan, and its footer. */
export const TurnView = memo(function TurnView({
  segment,
  actions,
  agentId = "generic",
  agentName = "Agent",
  active = false,
  live = true,
  part,
  suppressByline = false,
  bareTool = false,
  tail,
  merged,
}: {
  readonly segment: RenderedSegment;
  readonly actions: TurnActions;
  readonly agentId?: string;
  readonly agentName?: string;
  readonly active?: boolean;
  readonly live?: boolean;
  /** Virtual rows render one stable item or the footer; standalone previews render the whole turn. */
  readonly part?: number | "footer";
  /** The item is a call inside an opened stretch of calls: the stretch's header names the agent. */
  readonly suppressByline?: boolean;
  /** The call is its stretch's only one: the stretch's header is its head, so it shows its output. */
  readonly bareTool?: boolean;
  /** The turn still runs: its footer's place shows the working line, or what the agent waits on. */
  readonly tail?: LiveTail;
  /** Shown at `part` instead of the segment's own item: reasoning in a row merged into one. */
  readonly merged?: RenderedItem;
}) {
  // The footer fades in where the working line was, never when its row merely scrolls back in.
  const wasLive = useRef(false);
  if (tail) wasLive.current = true;
  const firstAgentItem = segment.items.findIndex((item) => item.kind !== "user");
  const selected = typeof part === "number" ? (merged ?? segment.items[part]) : undefined;
  const entries =
    part === undefined
      ? segment.items.map((item, index) => ({ item, index }))
      : selected && typeof part === "number"
        ? [{ item: selected, index: part }]
        : [];
  const footer = part === undefined || part === "footer";
  return (
    // biome-ignore lint/a11y/useSemanticElements: A transcript group contains messages, not form controls.
    <section
      role="group"
      className="chat-segment"
      data-kind={segment.turnId === null ? "outside" : "turn"}
      aria-label={segment.turnId === null ? "Between turns" : "Turn"}
      data-testid={part === undefined || part === 0 ? "segment" : "segment-part"}
      data-part={part ?? "whole"}
      data-item={selected?.kind}
      data-in-group={suppressByline ? "" : undefined}
      data-test-kind={segment.turnId === null ? "outside" : "turn"}
      data-test-turn-id={segment.turnId ?? ""}
      data-test-segment-key={segment.key}
      data-test-prompt={segment.items.find((item) => item.kind === "user")?.text ?? ""}
      data-test-stop-reason={segment.end?.stopReason ?? ""}
      data-test-recovered={String(segment.end?.recovered ?? false)}
    >
      {entries.map(({ item, index }) => {
        // Grok names no one above its answers: the agent's name is said once, to screen readers,
        // where its part of the turn begins. The header and the composer show who is answering.
        const byline =
          index === firstAgentItem &&
          segment.turnId !== null &&
          !segment.partial &&
          !suppressByline ? (
            <span className="sr-only">{agentName}</span>
          ) : null;
        const content = (() => {
          switch (item.kind) {
            case "user":
              return (
                <UserMessage
                  key={item.key}
                  item={item}
                  images={
                    item.steering
                      ? { type: "steering", message_id: item.key.slice("user:".length) }
                      : segment.turnId
                        ? { type: "turn", turn_id: segment.turnId }
                        : undefined
                  }
                  settled={segment.end !== null || !live}
                  onEdit={
                    segment.turnId && actions.onContinue && !active
                      ? () =>
                          actions.onContinue?.({ type: "edit", turn_id: segment.turnId as string })
                      : undefined
                  }
                  onRewind={
                    segment.turnId && actions.onRewind && !active
                      ? () => actions.onRewind?.(segment.turnId as string)
                      : undefined
                  }
                />
              );
            case "message":
              return (
                <div
                  key={item.key}
                  className="chat-message"
                  data-testid="item"
                  data-test-kind="message"
                  data-test-completed={String(item.completed)}
                >
                  <Answer text={item.text} />
                </div>
              );
            case "tool":
              return (
                <ToolBlock
                  key={item.key}
                  tool={item}
                  display={item.display}
                  bare={bareTool}
                  blobs={actions.blobs}
                  onLoadBlob={actions.onLoadBlob}
                />
              );
            case "thought":
              return <ThoughtRow key={item.key} thought={item} thinking={item.thinking} />;
            case "permission":
              return (
                <PermissionCard
                  key={item.key}
                  item={item}
                  request={actions.requests.get(item.request.request_id)}
                  onRespond={(optionId) =>
                    actions.onRespondPermission(item.request.request_id, optionId)
                  }
                  agentName={actions.agentName}
                  onAddToPlan={actions.onAddToPlan}
                  onAcceptPlanEdited={actions.onAcceptPlanEdited}
                  planNotes={actions.planNotes}
                  planBuild={actions.planBuild}
                  blobs={actions.blobs}
                  onLoadBlob={actions.onLoadBlob}
                />
              );
            case "question":
              return (
                <QuestionCard
                  key={item.key}
                  item={item}
                  request={actions.requests.get(item.request.request_id)}
                  onRespond={(answer) => actions.onRespondUi(item.request.request_id, answer)}
                />
              );
            case "notice":
              return (
                <p
                  key={item.key}
                  className="chat-notice"
                  data-level={item.level}
                  data-testid="item"
                  data-test-kind="notice"
                  data-test-level={item.level}
                >
                  {item.message}
                </p>
              );
            case "error":
              return (
                <ErrorCard
                  key={item.key}
                  item={item}
                  actions={actions.errorActions(item.key, item.errorKind)}
                  onRetry={() => actions.onStart(false)}
                  onStartFresh={() => actions.onStart(true)}
                  onRecreate={actions.onRecreateFolder}
                  onShowLog={actions.onShowLog}
                />
              );
            case "gap":
              return (
                <p key={item.key} className="chat-marker" data-testid="item" data-test-kind="gap">
                  Recent output isn't saved yet; it appears here when saving resumes.
                </p>
              );
            default: {
              const unreachable: never = item;
              throw new Error(`unhandled item: ${JSON.stringify(unreachable)}`);
            }
          }
        })();
        return (
          <Fragment key={item.key}>
            {byline}
            {content}
          </Fragment>
        );
      })}
      {footer && segment.plan !== null && segment.plan.length > 0 ? (
        <Plan entries={segment.plan} />
      ) : null}
      {footer && tail ? (
        <LiveTailLine tail={tail} agentId={agentId} agentName={agentName} />
      ) : footer ? (
        <Footer
          segment={segment}
          agentName={agentName}
          canResume={actions.canResume}
          onStart={actions.onStart}
          onContinue={actions.onContinue}
          arrived={wasLive.current}
        />
      ) : null}
    </section>
  );
});

function Plan({ entries }: { readonly entries: readonly PlanEntry[] }) {
  return (
    <section aria-label="Plan" data-testid="plan">
      <ul className="chat-checklist">
        {entries.map((entry, index) => (
          <li
            // biome-ignore lint/suspicious/noArrayIndexKey: a plan update replaces the whole list.
            key={index}
            data-status={entry.status}
            data-testid="plan-entry"
            data-test-status={entry.status}
          >
            <input
              type="checkbox"
              checked={entry.status === "completed"}
              readOnly
              aria-label={entry.status.replace("_", " ")}
            />{" "}
            {entry.content}
          </li>
        ))}
      </ul>
    </section>
  );
}

/** Why the turn ended, its tokens with their scope, how long it took, and the session's cost. */
function Footer({
  segment,
  agentName,
  canResume,
  onStart,
  onContinue,
  arrived = false,
}: {
  readonly segment: RenderedSegment;
  readonly agentName: string;
  readonly canResume: boolean;
  readonly onStart: (fresh: boolean) => void;
  readonly onContinue?: (intent: ConversationIntent) => void;
  /** The turn just ended in view: the footer takes the working line's place with a fade. */
  readonly arrived?: boolean;
}) {
  const { end } = segment;
  if (end === null) return null;
  const reason = stopReasonLabel(end);
  const tokens = end.usage !== null ? tokensLabel(end.usage) : null;
  const duration = end.durationMs !== null ? formatDuration(end.durationMs) : null;
  const cost = costLabel(segment.usage);
  const parts = [reason, tokens, duration, cost !== null ? `${cost} session` : null].filter(
    (part): part is string => part !== null,
  );
  const response = segment.items
    .filter((item) => item.kind === "message")
    .map((item) => item.text)
    .join("\n\n");
  const receipt = [agentName, ...parts].join("\n");
  // Grok's row: the actions as quiet icons, then the time. A turn that ended normally says only
  // its time; any other ending says how it ended. Tokens and cost wait in the receipt and the hint.
  const meta = [end.stopReason === "end_turn" ? null : reason, duration].filter(Boolean);
  return (
    <div
      className="chat-footer"
      data-testid="turn-footer"
      data-reason={end.stopReason}
      data-arrived={arrived ? "" : undefined}
    >
      {response ? <CopyAction text={response} label="Copy response as Markdown" /> : null}
      {response ? <ReadAloud id={segment.key} text={response} /> : null}
      {segment.turnId && onContinue ? (
        <>
          <button
            type="button"
            className="chat-action"
            aria-label="Retry"
            title="Retry"
            onClick={() => onContinue({ type: "retry", turn_id: segment.turnId as string })}
          >
            <RotateCcw size={16} />
          </button>
          <button
            type="button"
            className="chat-action"
            aria-label="Fork"
            title="Fork into a new chat"
            onClick={() => onContinue({ type: "fork", turn_id: segment.turnId as string })}
          >
            <GitFork size={16} />
          </button>
        </>
      ) : null}
      <CopyAction text={receipt} label="Copy turn receipt" icon={<ReceiptText size={16} />} />
      {meta.length ? (
        <span className="chat-footer-meta" title={parts.join(" · ")}>
          {meta.join(" · ")}
        </span>
      ) : (
        <span className="sr-only">{reason}</span>
      )}
      {end.recovered && end.stopReason === "interrupted" && canResume ? (
        <button
          type="button"
          className="g-button g-button-ghost g-button-24"
          data-testid="turn-resume"
          onClick={() => onStart(false)}
        >
          Resume session
        </button>
      ) : null}
    </div>
  );
}
