import { prefersReducedMotion } from "@ghost/ui";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  chatActivity,
  lastToolKind,
  liveReasoning,
  streamingText,
  thinkingNow,
} from "../../lib/chat/activity";
import type { ChatState } from "../../lib/chat/chatStore";
import { findMatches, highlightRanges } from "../../lib/chat/find";
import { useKeepStepsOpen } from "../../lib/chat/stepsPreference";
import { type RenderedItem, renderCache } from "../../lib/chat/transcript";
import { changedFiles, workedLabel, workFold } from "../../lib/chat/workFold";
import { workingContext, workingSince } from "../../lib/chat/working";
import { formatDuration } from "../../lib/format";
import { useStickToBottom } from "../../lib/hooks";
import type { StickToBottom } from "../../lib/stick";
import { type DisclosureChoices, DisclosureMemory } from "./DisclosureMemory";
import { FindBar } from "./FindBar";
import { ToolGroup } from "./ToolGroup";
import { type TurnActions, TurnView } from "./TurnView";
import { UserMessage } from "./UserMessage";
import { type TranscriptRow, VirtualRows } from "./VirtualRows";
import { ChangedFiles, WorkedFor } from "./WorkedFor";
import { type LiveTail, NowLine } from "./WorkingLine";

/** A disclosure the reader toggled, while its rows glide: until when, and which way. */
interface Reveal {
  readonly kind: "open" | "close";
  readonly until: number;
}

/** Each a little longer than its glide (--g-dur-glide 240ms, --g-dur-panel 180ms). */
const OPEN_MS = 300;
const CLOSE_MS = 240;

type RenderedTool = Extract<RenderedItem, { kind: "tool" }>;
type RenderedThought = Extract<RenderedItem, { kind: "thought" }>;
/** What a stretch of work holds: the agent's calls and its reasoning between them. */
type WorkItem = RenderedTool | RenderedThought;

/**
 * Reasoning with no call in it, as its one row: "Thought for" the whole of it, opening to every
 * part in order. A single piece is itself.
 */
export function mergeThoughts(work: readonly WorkItem[]): RenderedThought | undefined {
  const thoughts = work.filter((entry): entry is RenderedThought => entry.kind === "thought");
  const first = thoughts[0];
  if (first === undefined || thoughts.length === 1) return undefined;
  const timed = thoughts.flatMap((thought) =>
    thought.durationMs === null ? [] : [thought.durationMs],
  );
  return {
    ...first,
    text: thoughts
      .map((thought) => thought.text.trim())
      .filter((text) => text !== "")
      .join("\n\n"),
    completed: thoughts.every((thought) => thought.completed),
    durationMs: timed.length > 0 ? timed.reduce((sum, ms) => sum + ms, 0) : null,
    thinking: thoughts.some((thought) => thought.thinking),
  };
}

/** What each kind of row usually measures, so the rows below hold still while it is measured. */
function estimateOf(item: RenderedItem, grouped: boolean): number {
  switch (item.kind) {
    case "user":
      return 88;
    case "message":
      return 160;
    case "tool":
      return grouped ? 24 : 28;
    case "thought":
      return 28;
    case "permission":
    case "question":
      return 180;
    case "error":
      return 120;
    case "notice":
    case "gap":
      return 32;
  }
}

/**
 * The chat's turns: "Load earlier" at the top while older pages remain, a marker when the oldest
 * loaded turn was cut by a page, then every turn as it renders. Follows the bottom while content
 * grows, unless the reader scrolled up.
 *
 * A turn reads as its prompt, then the agent's text and its stretches of work, each stretch one
 * disclosure however many calls it made, then one tail row: the working line while the turn runs,
 * its footer once it ends, in the same place, so ending a turn moves nothing.
 */
export function Transcript({
  state,
  live,
  actions,
  onLoadEarlier,
  dockedRequests,
  findRequest = 0,
}: {
  readonly state: ChatState;
  /** The session has a process (tools still running may finish). */
  readonly live: boolean;
  readonly actions: TurnActions;
  readonly onLoadEarlier: () => void;
  readonly dockedRequests?: ReadonlySet<string>;
  /** Each new value opens find in chat, or focuses its field again (Ctrl+F). */
  readonly findRequest?: number;
}) {
  const [render] = useState(renderCache);
  const [disclosures] = useState<DisclosureChoices>(() => new Map());
  const [groups, setGroups] = useState<DisclosureChoices>(() => new Map());
  // Finished turns whose "Worked for" row was opened or closed, by segment; every other one
  // follows Settings > Chats > Keep steps open (folded unless it is on).
  const [folds, setFolds] = useState<ReadonlyMap<string, boolean>>(() => new Map());
  const keepStepsOpen = useKeepStepsOpen();
  const [reveals, setReveals] = useState<ReadonlyMap<string, Reveal>>(() => new Map());
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const segments = useMemo(() => render(state.transcript, live), [render, state.transcript, live]);
  // Find in chat: the query, the match shown, and a count of requests to bring it into view.
  const [find, setFind] = useState<{
    readonly query: string;
    readonly index: number;
    readonly nonce: number;
  } | null>(null);
  useEffect(() => {
    if (findRequest > 0) setFind((current) => current ?? { query: "", index: 0, nonce: 0 });
  }, [findRequest]);
  const query = find?.query ?? "";
  const matches = useMemo(() => findMatches(segments, query), [segments, query]);
  const shown =
    find && matches.length > 0
      ? ((find.index % matches.length) + matches.length) % matches.length
      : 0;
  const current = find ? (matches[shown] ?? null) : null;
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef<StickToBottom | null>(null);
  const captureHistory = useRef<(() => void) | null>(null);
  const [pinned, setPinned] = useState(true);
  const followRef = useStickToBottom<HTMLDivElement>(state.transcript, stick, setPinned);
  const scrollRef = useCallback(
    (node: HTMLDivElement | null) => {
      scroller.current = node;
      return followRef(node);
    },
    [followRef],
  );
  const first = segments[0];
  const last = segments.at(-1);
  const status = state.session?.status ?? state.chat?.status ?? "stopped";
  const activity = chatActivity(state);
  const chatAgentId = state.chat?.agent_id ?? "generic";
  const chatAgentName = state.session?.agent_name ?? state.chat?.agent_name ?? "Agent";
  const at = typeof performance === "undefined" ? 0 : performance.now();
  // The turn still running at the end: its tail row shows the working line.
  const tailTurn =
    last !== undefined &&
    last.turnId !== null &&
    last.end === null &&
    (status === "running" || status === "cancelling" || status === "stopping")
      ? last
      : null;
  // A message on its way (its folder checkpoint saving) shows as sent, with "Sending" under it.
  const sending = state.sending;
  const pendingLabel =
    sending !== null
      ? "Sending"
      : tailTurn !== null
        ? null
        : activity === "waiting"
          ? "Waiting on you"
          : status === "starting"
            ? "Starting session"
            : status === "cancelling" || status === "stopping"
              ? "Stopping"
              : status === "running"
                ? "Working"
                : null;
  const rows: TranscriptRow[] = [];
  let focusedGroupKey: string | null = null;
  let focusedGroupPhase = "";
  let leading: ReactNode = null;
  if (state.hasMore || first?.partial || state.skipped > 0)
    leading = (
      <div className="chat-segment">
        {state.hasMore ? (
          <div className="ui-row">
            <button
              type="button"
              className="g-button g-button-ghost"
              data-testid="load-earlier"
              disabled={state.loadingEarlier}
              onClick={() => {
                captureHistory.current?.();
                onLoadEarlier();
              }}
            >
              {state.loadingEarlier ? "Loading…" : "Load earlier"}
            </button>
            {state.earlierError !== null ? (
              <span role="alert" className="ui-error">
                {state.earlierError}
              </span>
            ) : null}
          </div>
        ) : null}
        {first?.partial ? (
          <p className="chat-marker">Earlier part of this turn not loaded</p>
        ) : null}
        {state.skipped > 0 ? (
          <p className="chat-marker">
            {state.skipped === 1
              ? "1 entry was written by a newer Ghost and can't be shown."
              : `${state.skipped} entries were written by a newer Ghost and can't be shown.`}
          </p>
        ) : null}
      </div>
    );
  for (const segment of segments) {
    // Each turn keeps the agent that ran it, after the chat switches agents too.
    const turnAgent = segment.turnId === null ? undefined : state.turnAgents.get(segment.turnId);
    const agentId = turnAgent?.agent_id ?? chatAgentId;
    const agentName =
      turnAgent === undefined || turnAgent.agent_id === chatAgentId
        ? chatAgentName
        : turnAgent.agent_name;
    const active = status === "running" && segment === last && segment.end === null;
    let prompt: TranscriptRow["prompt"];
    // A finished turn's work folds under one "Worked for" row above its final answer.
    const fold = workFold(segment);
    const foldOpen =
      fold !== null &&
      ((folds.get(segment.key) ?? keepStepsOpen) ||
        (current?.folded === true && current.segmentKey === segment.key));
    const addItem = (
      index: number,
      item: RenderedItem,
      inGroup?: { readonly bare: boolean; readonly reveal?: Reveal["kind"] },
      merged?: RenderedItem,
    ) =>
      rows.push({
        key: JSON.stringify([segment.key, item.key]),
        estimate: estimateOf(item, inGroup !== undefined),
        prompt,
        reveal: inGroup?.reveal,
        // The person's own message appears at once; the agent's output fades in.
        arrive: item.kind !== "user",
        retain:
          (item.kind === "question" || item.kind === "permission") &&
          item.resolution === null &&
          actions.requests.get(item.request.request_id)?.gone !== true &&
          (item.kind !== "question" || item.request.prompt.type !== "notify"),
        content: (
          <TurnView
            segment={segment}
            part={index}
            actions={actions}
            agentId={agentId}
            agentName={agentName}
            active={active}
            live={live}
            suppressByline={inGroup !== undefined}
            bareTool={inGroup?.bare === true}
            merged={merged}
          />
        ),
      });
    for (let index = 0; index < segment.items.length; index++) {
      const item = segment.items[index];
      if (!item) continue;
      if (fold !== null && index === fold.from) {
        rows.push({
          key: JSON.stringify([segment.key, "work"]),
          estimate: 32,
          prompt,
          content: (
            <WorkedFor
              label={workedLabel(fold.steps, segment.end?.durationMs ?? null, formatDuration)}
              open={foldOpen}
              agentName={agentName}
              onToggle={() => setFolds((current) => new Map(current).set(segment.key, !foldOpen))}
            />
          ),
        });
        if (!foldOpen) {
          index = fold.to - 1;
          continue;
        }
      }
      if (
        (item.kind === "permission" || item.kind === "question") &&
        dockedRequests?.has(item.request.request_id)
      )
        continue;
      if (item.kind === "user")
        prompt = { key: JSON.stringify([segment.key, item.key]), text: item.text };
      if (item.kind !== "tool" && item.kind !== "thought") {
        addItem(index, item);
        continue;
      }
      // A stretch of work: every call and every piece of reasoning until the agent's next text,
      // card or notice. Reasoning has no row of its own among the calls (owner, 23 November 2026:
      // the "Thought" rows between them were clutter); an opened stretch lists it in its place.
      const work: WorkItem[] = [];
      for (let next = index; next < segment.items.length; next++) {
        const candidate = segment.items[next];
        if (candidate?.kind !== "tool" && candidate?.kind !== "thought") break;
        work.push(candidate);
      }
      const tools = work.filter((entry): entry is RenderedTool => entry.kind === "tool");
      if (tools.length === 0) {
        // Reasoning alone. While the running turn may still add a call to it, the working line
        // speaks for it; once anything else follows, or the turn ends, it is one "Thought" row.
        const trailing = segment === tailTurn && index + work.length >= segment.items.length;
        if (!trailing) addItem(index, item, undefined, mergeThoughts(work));
        index += work.length - 1;
        continue;
      }
      // Keyed by its first call or reasoning, so a stretch keeps its row (and its disclosure) as
      // calls join.
      const key = JSON.stringify([segment.key, "tool-group", item.key]);
      const failed = tools.some((tool) => tool.display === "failed");
      const phase = failed
        ? "failed"
        : tools.some((tool) => tool.display === "pending" || tool.display === "in_progress")
          ? "running"
          : "settled";
      const choice = groups.get(key);
      const ownsFocus = work.some(
        (entry) => JSON.stringify([segment.key, entry.key]) === focusedKey,
      );
      if (ownsFocus) {
        focusedGroupKey = key;
        focusedGroupPhase = phase;
      }
      const expanded =
        ownsFocus || (choice?.phase === phase || choice?.open === true ? choice.open : failed);
      const reveal = reveals.get(key);
      const moving = reveal !== undefined && reveal.until > at ? reveal.kind : undefined;
      rows.push({
        key,
        estimate: 28,
        prompt,
        content: (
          <ToolGroup
            tools={tools}
            expanded={expanded}
            agentName={agentName}
            byline={
              !segment.partial &&
              index === segment.items.findIndex((candidate) => candidate.kind !== "user")
            }
            onToggle={() => {
              setGroups((current) => new Map(current).set(key, { phase, open: !expanded }));
              if (prefersReducedMotion()) return;
              const kind = expanded ? "close" : "open";
              const until = performance.now() + (expanded ? CLOSE_MS : OPEN_MS);
              setReveals((current) => new Map(current).set(key, { kind, until }));
            }}
          />
        ),
      });
      // Closing, the calls stay a moment to glide their height away. Opened, reasoning shows where
      // it has words to read; the reasoning being thought now is the working line's.
      if (expanded || moving === "close")
        work.forEach((entry, offset) => {
          if (entry.kind === "thought" && (entry.thinking || entry.text.trim() === "")) return;
          addItem(index + offset, entry, {
            bare: entry.kind === "tool" && tools.length === 1,
            reveal: expanded ? (moving === "open" ? "open" : undefined) : "close",
          });
        });
      index += work.length - 1;
    }
    const tail: LiveTail | undefined =
      segment !== tailTurn
        ? undefined
        : status !== "running"
          ? { kind: "stopping" }
          : activity === "waiting"
            ? { kind: "waiting" }
            : {
                kind: "working",
                seed: segment.key,
                // While the agent thinks, the words are thinking words, whatever it ran before.
                context: thinkingNow(segment) ? null : workingContext(lastToolKind(segment)),
                visible: !streamingText(segment),
                reasoning: liveReasoning(segment),
                since: workingSince(`${state.chatId}:${segment.key}`, Date.now()),
                model: state.session?.model ?? null,
              };
    // What the turn changed, under its answer, so a folded turn still says what it did.
    const changed = segment.end !== null ? changedFiles(segment) : [];
    if (changed.length > 0)
      rows.push({
        key: JSON.stringify([segment.key, "changed"]),
        estimate: 36,
        prompt,
        content: <ChangedFiles files={changed} onReview={actions.onOpenChanges} />,
      });
    if (segment.end !== null || (segment.plan?.length ?? 0) > 0 || tail !== undefined)
      rows.push({
        key: JSON.stringify([segment.key, "footer"]),
        estimate: 44,
        prompt,
        content: (
          <TurnView
            segment={segment}
            part="footer"
            actions={actions}
            agentId={agentId}
            agentName={agentName}
            tail={tail}
          />
        ),
      });
  }
  if (sending !== null)
    rows.push({
      key: `sending:${sending.prompt_id}`,
      estimate: 72,
      arrive: false,
      content: (
        <div
          className="chat-segment chat-sending"
          data-testid="sending-message"
          data-test-prompt-id={sending.prompt_id}
        >
          <UserMessage
            item={{ kind: "user", key: `sending:${sending.prompt_id}`, text: sending.text }}
            images={{ type: "queued", prompt_id: sending.prompt_id }}
          />
          {sending.parts.some((part) => part.type !== "text") ? (
            <ul aria-label="Message attachments">
              {sending.parts.flatMap((part) =>
                part.type === "image" ? (
                  <li key={`image:${part.blob}`}>Attached image ({part.mime})</li>
                ) : part.type === "mention" ? (
                  <li key={`mention:${part.path}`}>Mentioned file: {part.path}</li>
                ) : (
                  []
                ),
              )}
            </ul>
          ) : null}
        </div>
      ),
    });
  if (pendingLabel)
    rows.push({
      key: `now:${last?.key ?? "new"}:${pendingLabel}`,
      estimate: 28,
      content: (
        <NowRow
          key={`${last?.key ?? "new"}:${pendingLabel}`}
          agentId={chatAgentId}
          agentName={chatAgentName}
          label={pendingLabel}
          activity={activity}
          delay={
            pendingLabel === "Working" &&
            (last?.items.some((item) => item.kind !== "user") ?? false)
          }
        />
      ),
    });
  if (segments.length === 0 && state.phase === "live" && pendingLabel === null)
    rows.push({
      key: "empty",
      estimate: 100,
      content: <p className="chat-centered">Nothing here yet. Send a message to start.</p>,
    });
  // A reveal ends with a render, so the rows it kept or marked settle as they are.
  useEffect(() => {
    if (reveals.size === 0) return;
    const soonest = Math.min(...[...reveals.values()].map((reveal) => reveal.until));
    const timer = setTimeout(
      () =>
        setReveals((current) => {
          const now = performance.now();
          const kept = new Map([...current].filter(([, reveal]) => reveal.until > now));
          return kept.size === current.size ? current : kept;
        }),
      Math.max(0, soonest - performance.now()) + 16,
    );
    return () => clearTimeout(timer);
  }, [reveals]);
  useLayoutEffect(() => {
    if (focusedGroupKey === null) return;
    const key = focusedGroupKey;
    // A new group may form around a focused call. Keep it open when focus moves to its header,
    // so the header does not collapse before the user's subsequent click can toggle it.
    setGroups((current) =>
      current.get(key)?.open === true
        ? current
        : new Map(current).set(key, { phase: focusedGroupPhase, open: true }),
    );
  }, [focusedGroupKey, focusedGroupPhase]);
  const [promptHost, setPromptHost] = useState<HTMLDivElement | null>(null);
  const findNonce = find?.nonce;
  // biome-ignore lint/correctness/useExhaustiveDependencies: a new nonce asks for the match again.
  useLayoutEffect(() => {
    const element = scroller.current;
    const registry = typeof CSS === "undefined" ? undefined : CSS.highlights;
    if (!registry || typeof Highlight === "undefined") return;
    const clear = () => {
      registry.delete("chat-find");
      registry.delete("chat-find-current");
    };
    if (!element || !query.trim() || matches.length === 0) {
      clear();
      return;
    }
    const rowsWith = new Set(matches.map((match) => match.rowKey));
    // Rows come and go as the reader scrolls; each change paints the rendered ones again.
    const paint = (): Range | null => {
      const all: Range[] = [];
      let shownRange: Range | null = null;
      for (const row of element.querySelectorAll<HTMLElement>("[data-row-key]")) {
        const key = row.dataset.rowKey;
        if (!key || !rowsWith.has(key)) continue;
        const content = row.querySelector(".chat-user-text, .chat-message") ?? row;
        const ranges = highlightRanges(content, query);
        all.push(...ranges);
        if (current?.rowKey === key)
          shownRange = ranges[Math.min(current.occurrence, ranges.length - 1)] ?? null;
      }
      registry.set("chat-find", new Highlight(...all));
      if (shownRange) registry.set("chat-find-current", new Highlight(shownRange));
      else registry.delete("chat-find-current");
      return shownRange;
    };
    const shownRange = paint();
    // The row is in view; bring the words themselves there, a long answer's included.
    const frame = requestAnimationFrame(() => {
      const range = paint() ?? shownRange;
      if (!range) return;
      const box = range.getBoundingClientRect();
      const view = element.getBoundingClientRect();
      if (box.top < view.top + 48 || box.bottom > view.bottom - 48)
        element.scrollTop += box.top - view.top - view.height / 2 + box.height / 2;
    });
    let pending = 0;
    const observer = new MutationObserver(() => {
      cancelAnimationFrame(pending);
      pending = requestAnimationFrame(() => void paint());
    });
    observer.observe(element, { childList: true, subtree: true, characterData: true });
    return () => {
      cancelAnimationFrame(frame);
      cancelAnimationFrame(pending);
      observer.disconnect();
      clear();
    };
  }, [query, matches, current, findNonce]);
  const closeFind = () => {
    setFind(null);
    scroller.current?.focus({ preventScroll: true });
  };
  return (
    <div className="chat-transcript-shell">
      <div ref={setPromptHost} />
      {find ? (
        <FindBar
          query={find.query}
          onQuery={(next) =>
            setFind((value) => ({ query: next, index: 0, nonce: (value?.nonce ?? 0) + 1 }))
          }
          index={shown}
          count={matches.length}
          limited={state.hasMore}
          onStep={(step) =>
            setFind((value) =>
              value ? { ...value, index: shown + step, nonce: value.nonce + 1 } : value,
            )
          }
          onClose={closeFind}
          focusRequest={findRequest}
        />
      ) : null}
      <div
        ref={scrollRef}
        className="chat-transcript"
        role="log"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: Scroll regions need keyboard navigation and a focus target after jumping to latest.
        tabIndex={0}
        aria-label="Transcript"
        data-testid="transcript"
      >
        <DisclosureMemory.Provider value={disclosures}>
          <VirtualRows
            promptHost={promptHost}
            rows={rows}
            leading={leading}
            scroller={scroller}
            stick={stick}
            historyRevision={state.historyRevision}
            historyError={state.earlierError}
            captureHistory={captureHistory}
            focusedKey={focusedKey}
            onFocusKey={setFocusedKey}
            revealRow={current && find ? { key: current.rowKey, nonce: find.nonce } : null}
          />
        </DisclosureMemory.Provider>
      </div>
      {!pinned ? (
        <button
          type="button"
          className="chat-latest"
          data-testid="jump-latest"
          onClick={() => {
            scroller.current?.focus({ preventScroll: true });
            stick.current?.pin();
          }}
        >
          <span aria-hidden="true">↓</span> Latest
        </button>
      ) : null}
    </div>
  );
}

/** Gaps shorter than 1.2 seconds remain quiet; a new turn and requests show immediately. */
function NowRow({
  agentId,
  agentName,
  label,
  activity,
  delay,
}: {
  readonly agentId: string;
  readonly agentName: string;
  readonly label: string;
  readonly activity: ReturnType<typeof chatActivity>;
  readonly delay: boolean;
}) {
  const [visible, setVisible] = useState(!delay);
  useEffect(() => {
    if (!delay) return;
    const timeout = setTimeout(() => setVisible(true), 1200);
    return () => clearTimeout(timeout);
  }, [delay]);
  if (!visible) return null;
  return <NowLine agentId={agentId} agentName={agentName} label={label} state={activity} />;
}
