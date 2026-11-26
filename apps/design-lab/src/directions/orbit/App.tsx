import "./tokens.css";
import "./orbit.css";
import { type AgentBrandId, getBrand } from "@ghost/ui";
import { AnimatePresence, motion } from "motion/react";
import {
  type CSSProperties,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { sceneFor } from "../../fixture/scenes";
import type { TimedEvent } from "../../fixture/types";
import {
  AUTONOMY_LABELS,
  type Autonomy,
  type ChatStatus,
  type ChatSummary,
  type DirectionProps,
  findAgent,
  findChat,
  findProject,
  type RightPanel as RightPanelState,
  type TranscriptModel,
  type Turn,
} from "../../kit";
import { EMPTY_TRANSCRIPT, eventsAt, reduceTranscript } from "../../model/transcript";
import { Composer } from "./Composer";
import { useWindowClock } from "./clock";
import { EmptyState } from "./EmptyState";
import { type MarkTimeline, markTimeline } from "./geometry";
import { Header } from "./Header";
import { escapeAction, focusOf, nextAutonomy } from "./keys";
import { handoffTokens, modelNowRow, requestTarget } from "./live";
import {
  failedHistory,
  firstLine,
  type LocalAction,
  localEvents,
  localTurnId,
  SCRIPTED_NOTE,
} from "./local";
import { meta } from "./meta";
import { openState, stateForChat } from "./nav";
import { Onboarding } from "./Onboarding";
import { PermissionCard } from "./Permission";
import { changedFiles, RightPanel } from "./RightPanel";
import { Sheet, type SheetRequest } from "./Sheet";
import { Sidebar } from "./Sidebar";
import type {
  Anchor,
  AnswerKind,
  ComposerState,
  Extra,
  HandoffSummary,
  NewExtra,
  NowRow,
  PermissionAnswer,
  QueuedMessage,
} from "./state";
import { Toast, type ToastMessage } from "./Toast";
import { Transcript } from "./Transcript";
import { formatClock, messageText, plainText, receiptText } from "./text";
import { useReduced } from "./ui";

/*
 * Orbit: mission control for your agents (docs/design/visual-language.md). The window is a warm
 * grey tray (black in dark) with paper panes laid on it, separated by hairlines, not glass. One
 * grotesk for words, its mono for everything a machine made, an orbit for every agent, and one
 * orange that means look here.
 *
 * Each lab state gets a fresh window (keyed below), so local choices such as an open picker or a
 * typed draft never leak from one state into another. What the user does after a recording ends
 * (answers, messages, stops) becomes local actions; local.ts turns them into the events an agent
 * would send next, and the window reduces them together with the recording.
 */
export function App(props: DirectionProps) {
  return <OrbitWindow key={props.state} {...props} />;
}

const ANSWER_TEXT: Readonly<Record<AnswerKind, string>> = {
  allow_once: "Allowed once",
  allow_chat: "Allowed in this chat",
  allow_always: "Always allowed in this project",
  deny: "Denied",
};

const NO_EVENTS: readonly TimedEvent[] = [];

/** How long Esc lets a run wind down before the stop lands, ms. A second Esc stops at once. */
const STOP_WIND_DOWN_MS = 600;

/** The main pane never narrows past this: a wider right panel floats over it instead. */
const MAIN_MIN = 560;

/**
 * Keeps the transcript at its end while it grows, unless the reader scrolled up. Following eases
 * over about 200 ms instead of snapping, so a code block that arrives three lines at a time slides
 * up rather than jumping; the first frame lands at the end at once, so stills never move. `end`
 * changes whenever the end moves; `stick` re-pins when something else moved it (the dock grew).
 */
function useStickToBottom(ref: { readonly current: HTMLElement | null }, end: string) {
  const [pinned, setPinned] = useState(true);
  const pinnedRef = useRef(true);
  const frame = useRef(0);
  const first = useRef(true);
  const setBoth = useCallback((value: boolean) => {
    pinnedRef.current = value;
    setPinned(value);
  }, []);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const onScroll = (): void => {
      if (frame.current !== 0) return;
      setBoth(element.scrollHeight - element.scrollTop - element.clientHeight < 48);
    };
    // A reader taking over (wheel, touch, keys) ends any easing at once.
    const takeOver = (): void => {
      if (frame.current !== 0) {
        cancelAnimationFrame(frame.current);
        frame.current = 0;
      }
    };
    element.addEventListener("scroll", onScroll, { passive: true });
    element.addEventListener("wheel", takeOver, { passive: true });
    element.addEventListener("touchstart", takeOver, { passive: true });
    element.addEventListener("keydown", takeOver);
    return () => {
      element.removeEventListener("scroll", onScroll);
      element.removeEventListener("wheel", takeOver);
      element.removeEventListener("touchstart", takeOver);
      element.removeEventListener("keydown", takeOver);
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, [ref, setBoth]);
  const follow = useCallback(() => {
    const element = ref.current;
    if (!element || !pinnedRef.current) return;
    if (first.current) {
      first.current = false;
      element.scrollTop = element.scrollHeight;
      return;
    }
    if (frame.current !== 0) return;
    const step = (): void => {
      const target = element.scrollHeight - element.clientHeight;
      const gap = target - element.scrollTop;
      if (gap <= 1) {
        element.scrollTop = target;
        frame.current = 0;
        return;
      }
      element.scrollTop += Math.max(1, gap * 0.25);
      frame.current = requestAnimationFrame(step);
    };
    frame.current = requestAnimationFrame(step);
  }, [ref]);
  useLayoutEffect(() => {
    if (end === "") return;
    follow();
  }, [end, follow]);
  const toBottom = useCallback(() => {
    const element = ref.current;
    if (!element) return;
    setBoth(true);
    element.scrollTo({ top: element.scrollHeight, behavior: "smooth" });
  }, [ref, setBoth]);
  return { pinned, toBottom, stick: follow };
}

/** Where a line added now belongs: after the last turn's items so far, or after its receipt. */
function anchorAfter(turns: readonly Turn[]): Anchor {
  const last = turns[turns.length - 1];
  if (!last) return { turnId: null, after: 0 };
  return last.endedAt !== null
    ? { turnId: last.id, after: "end" }
    : { turnId: last.id, after: last.items.length };
}

/** The sidebar status of the open chat, from what its last turn is doing. */
function statusOf(model: TranscriptModel): ChatStatus {
  const turn = model.turns[model.turns.length - 1];
  if (!turn) return "idle";
  if (turn.status === "waiting" || model.pendingPermissions.length > 0) return "needs_approval";
  if (turn.status === "running") return "running";
  if (turn.status === "failed") return "error";
  return "idle";
}

/** The turn whose edits the Changes panel and the header's diffstat both show: the last with any. */
function changesTurn(model: TranscriptModel): Turn | null {
  return (
    [...model.turns].reverse().find((entry) => changedFiles(entry).length > 0) ??
    model.turns[model.turns.length - 1] ??
    null
  );
}

function OrbitWindow({
  state,
  model: recordedModel,
  workspace,
  player,
  theme = "dark",
}: DirectionProps) {
  const reduced = useReduced();
  const clock = useWindowClock(player);
  const scene = useMemo(() => sceneFor(state), [state]);
  const [opened, setOpened] = useState<ChatSummary | null>(null);
  const [composer, setComposer] = useState<ComposerState>(() => ({
    ...workspace.composer,
    finalConfirm: true,
  }));
  const [queue, setQueue] = useState<readonly QueuedMessage[]>([]);
  const [queuePaused, setQueuePaused] = useState(false);
  const [actions, setActions] = useState<readonly LocalAction[]>([]);
  const [extras, setExtras] = useState<readonly Extra[]>([]);
  const [stopping, setStopping] = useState<number | null>(null);
  const [cardOpen, setCardOpen] = useState(true);
  const [panel, setPanel] = useState<RightPanelState | null>(workspace.rightPanel);
  const [panelWidth, setPanelWidth] = useState(380);
  const [autonomyOpen, setAutonomyOpen] = useState(false);
  const [scrolled, setScrolled] = useState(false);
  const [rail, setRail] = useState(false);
  const [sheet, setSheet] = useState<SheetRequest | null>(null);
  const [toast, setToast] = useState<ToastMessage | null>(null);
  const [title, setTitle] = useState<string | null>(null);
  const [archived, setArchived] = useState<ReadonlySet<string>>(() => new Set());
  const [rootWidth, setRootWidth] = useState(1440);
  const nextId = useRef(1);
  const rootRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const dockRef = useRef<HTMLDivElement>(null);
  const paneRef = useRef<HTMLElement>(null);
  const stopTimer = useRef(0);

  const update = useCallback((change: Partial<ComposerState>) => {
    setComposer((previous) => ({ ...previous, ...change }));
  }, []);

  const say = useCallback(
    (text: string, extra: Pick<ToastMessage, "action" | "onAction" | "keys"> = { action: null }) =>
      setToast({ id: nextId.current++, text, ...extra }),
    [],
  );
  const soon = useCallback((what: string) => say(`${what} is not in this lab yet`), [say]);

  /* ---------------------------------------------------------- the recording plus what happened here */

  // A chat opened here has no recording. A failed one opens on its failed turn; any other opens on
  // a stub of its history, and only what happens here fills it.
  const history = useMemo(() => (opened ? failedHistory(opened) : null), [opened]);
  const historyModel = useMemo(
    () => (history ? reduceTranscript(history) : EMPTY_TRANSCRIPT),
    [history],
  );
  const baseModel = opened ? historyModel : recordedModel;
  const recordedEvents = opened ? (history ?? NO_EVENTS) : (scene.recording?.events ?? NO_EVENTS);
  const contextTokens = Math.round(handoffTokens(baseModel) * 1.6);
  const local = useMemo(
    () => (actions.length === 0 ? NO_EVENTS : localEvents(actions, baseModel, contextTokens)),
    [actions, baseModel, contextTokens],
  );
  const localCount = useSyncExternalStore(
    clock.subscribe,
    () => eventsAt(local, clock.getTime()),
    () => eventsAt(local, clock.getTime()),
  );
  const previousModel = useRef<TranscriptModel | null>(null);
  const model = useMemo(() => {
    if (localCount === 0) return baseModel;
    const recorded = recordedEvents.slice(0, baseModel.eventCount);
    return reduceTranscript([...recorded, ...local.slice(0, localCount)], previousModel.current);
  }, [baseModel, recordedEvents, local, localCount]);
  useEffect(() => {
    previousModel.current = model;
  }, [model]);

  const resetLocal = useCallback(() => {
    window.clearTimeout(stopTimer.current);
    setActions([]);
    setExtras([]);
    setStopping(null);
    setQueuePaused(false);
    setCardOpen(true);
    clock.reset();
  }, [clock]);

  // The recording loops: when it starts over, so does everything added to it here.
  const lastCount = useRef(recordedModel.eventCount);
  useEffect(() => {
    if (recordedModel.eventCount < lastCount.current) resetLocal();
    lastCount.current = recordedModel.eventCount;
  }, [recordedModel.eventCount, resetLocal]);

  // Resuming playback from the lab's controls undoes what was done here (a stop, a reply).
  const wasPlaying = useRef(player.playing);
  useEffect(() => {
    if (player.playing && !wasPlaying.current) resetLocal();
    wasPlaying.current = player.playing;
  }, [player.playing, resetLocal]);

  useEffect(() => () => window.clearTimeout(stopTimer.current), []);

  /* --------------------------------------------------------------------------- who and what */

  const onboarding = state === "onboarding";
  const lastTurn = model.turns[model.turns.length - 1] ?? null;
  const empty = model.turns.length === 0 && extras.length === 0 && !opened;
  const chat = opened ?? findChat(workspace, workspace.activeChatId);
  const project =
    findProject(workspace, opened?.projectId ?? workspace.activeProjectId) ??
    findProject(workspace, workspace.activeProjectId);
  const projectName = project?.name ?? "auracode";
  const chatAgent = (lastTurn?.agent ?? chat?.agent ?? null) as AgentBrandId | null;
  const nameOf = useCallback(
    (agentId: string): string => findAgent(workspace, agentId)?.name ?? getBrand(agentId).name,
    [workspace],
  );
  const running =
    lastTurn !== null && (lastTurn.status === "running" || lastTurn.status === "waiting");
  const pending = model.pendingPermissions[0] ?? null;
  const answers = useMemo(() => {
    const map = new Map<string, PermissionAnswer>();
    for (const action of actions) {
      if (action.kind === "answer") {
        map.set(action.request.requestId, { kind: action.answer, at: action.at });
      }
    }
    return map as ReadonlyMap<string, PermissionAnswer>;
  }, [actions]);
  // A call that waited on an answer runs from the answer: its timer and duration start there.
  const runFrom = useMemo(() => {
    const map = new Map<string, number>();
    for (const action of actions) {
      if (action.kind === "answer" && action.answer !== "deny" && action.request.toolCallId) {
        map.set(action.request.toolCallId, action.at);
      }
    }
    return map as ReadonlyMap<string, number>;
  }, [actions]);
  const answeredAction = [...actions].reverse().find((action) => action.kind === "answer");
  const answered = answeredAction?.kind === "answer" ? answeredAction : null;
  const denied = useMemo(
    () =>
      new Set(
        actions.flatMap((action) =>
          action.kind === "answer" && action.answer === "deny" && action.request.toolCallId
            ? [action.request.toolCallId]
            : [],
        ),
      ) as ReadonlySet<string>,
    [actions],
  );

  // The autonomy a request was asked under: what the composer said when it first arrived.
  const [askedUnder] = useState<Autonomy>(() => workspace.composer.autonomy);

  const timeline = useMemo<MarkTimeline>(() => markTimeline(model), [model]);
  const liveAgent = (chatAgent ?? composer.agent) as AgentBrandId;
  const now: NowRow | null = useMemo(() => {
    if (stopping !== null && running) {
      return {
        agent: liveAgent,
        state: "stopping",
        verb: "Stopping",
        object: null,
        since: stopping,
        delayMs: 0,
      };
    }
    return modelNowRow(model, liveAgent);
  }, [stopping, running, model, liveAgent]);

  const addExtra = useCallback((extra: NewExtra) => {
    setExtras((previous) => [...previous, { ...extra, id: `x${nextId.current++}` } as Extra]);
  }, []);

  /* ------------------------------------------------------------------------------ actions */

  const stop = useCallback(() => {
    window.clearTimeout(stopTimer.current);
    setStopping(null);
    if (!lastTurn || (lastTurn.status !== "running" && lastTurn.status !== "waiting")) return;
    const at = clock.getTime();
    clock.halt();
    // A stop never sends the next queued message by itself: the queue waits for you.
    setQueuePaused(true);
    setActions((previous) => [
      ...previous,
      { kind: "stop", id: `s${nextId.current++}`, at, turnId: lastTurn.id },
    ]);
  }, [clock, lastTurn]);

  const beginStop = useCallback(() => {
    if (stopping !== null) {
      stop();
      return;
    }
    setStopping(clock.getTime());
    setQueuePaused(true);
    window.clearTimeout(stopTimer.current);
    stopTimer.current = window.setTimeout(stop, STOP_WIND_DOWN_MS);
  }, [stopping, stop, clock]);

  const sentBefore = actions.some((action) => action.kind === "send");
  const send = useCallback(
    (text: string, options: { at?: number } = {}) => {
      const agentId = composer.agent;
      const agent = findAgent(workspace, agentId);
      const at = options.at ?? clock.getTime();
      const id = `m${nextId.current++}`;
      if (chatAgent !== null && agentId !== chatAgent) {
        addExtra({
          kind: "event",
          anchor: anchorAfter(model.turns),
          icon: null,
          text: "Switched to",
          agent: agentId,
          agentName: nameOf(agentId),
          detail: `handoff ${handoffTokens(model)} tokens`,
        });
      }
      // The lab has no agent behind it: the first scripted reply says so, once, as a system line.
      if (!sentBefore) {
        addExtra({
          kind: "event",
          anchor: { turnId: localTurnId(id), after: 0 },
          icon: "info",
          text: SCRIPTED_NOTE,
        });
      }
      setActions((previous) => [
        ...previous,
        {
          kind: "send",
          id,
          at,
          text,
          agent: agentId,
          agentName: agent?.name ?? nameOf(agentId),
          model: composer.model || null,
          effort: composer.effort,
        },
      ]);
      setQueuePaused(false);
      clock.run();
    },
    [composer, workspace, clock, chatAgent, addExtra, model, nameOf, sentBefore],
  );

  const answerRequest = useCallback(
    (kind: AnswerKind, instructions: string | null) => {
      if (!pending || !lastTurn) return;
      const at = clock.getTime();
      const answer: LocalAction = {
        kind: "answer",
        id: `a${nextId.current++}`,
        at,
        request: pending,
        turnId: lastTurn.id,
        answer: kind,
        instead: instructions !== null,
      };
      setActions((previous) => [...previous, answer]);
      // The answer is logged where it was asked: right before the call it lets through.
      const holder = lastTurn.items.findIndex(
        (item) =>
          item.kind === "tools" && item.calls.some((call) => call.id === pending.toolCallId),
      );
      addExtra({
        kind: "event",
        anchor: { turnId: lastTurn.id, after: holder === -1 ? lastTurn.items.length : holder },
        icon: kind === "deny" ? "x" : "check",
        text: ANSWER_TEXT[kind],
        literal: requestTarget(pending),
        detail: formatClock(at - pending.requestedAt),
      });
      if (instructions) send(instructions, { at: at + 60 });
      clock.run();
      // The answer shows on the card for a beat, then the card folds back into the composer.
      window.setTimeout(() => setCardOpen(false), reduced ? 0 : 650);
    },
    [pending, lastTurn, clock, addExtra, send, reduced],
  );

  const setAutonomy = useCallback(
    (level: Autonomy) => {
      if (level === composer.autonomy) return;
      update({ autonomy: level });
      if (empty) return;
      addExtra({
        kind: "event",
        anchor: anchorAfter(model.turns),
        icon: "level",
        text: `${AUTONOMY_LABELS[level]} from here`,
      });
    },
    [composer.autonomy, update, empty, addExtra, model],
  );

  /** The picker's choice: a level the new agent cannot run under falls to one it can. */
  const commitChoice = useCallback(
    (choice: { agent: AgentBrandId; model: string; effort: string | null }) => {
      const agent = findAgent(workspace, choice.agent);
      const allowed = agent?.autonomy ?? ["full_auto"];
      let autonomy = composer.autonomy;
      if (!allowed.includes(autonomy)) {
        autonomy = allowed[0] ?? "full_auto";
        if (!empty) {
          addExtra({
            kind: "notice",
            anchor: anchorAfter(model.turns),
            text: `${agent?.name ?? choice.agent} never asks before acting, so this chat runs in ${AUTONOMY_LABELS[autonomy]}.`,
            action: "Settings",
          });
        }
      }
      update({ ...choice, autonomy, pickerOpen: false });
    },
    [workspace, composer.autonomy, empty, addExtra, model, update],
  );

  const steer = useCallback(
    (text: string) => {
      if (!lastTurn) return;
      addExtra({
        kind: "steer",
        anchor: anchorAfter(model.turns),
        text,
        clock: formatClock(clock.getTime() - lastTurn.startedAt),
      });
      update({ draft: "" });
    },
    [lastTurn, addExtra, model, clock, update],
  );

  // When a turn ends, the first queued message goes out, unless a stop paused the queue.
  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running && queue.length > 0 && !queuePaused && stopping === null) {
      const [first, ...rest] = queue;
      if (first) {
        setQueue(rest);
        send(first.text);
      }
    }
    wasRunning.current = running;
  }, [running, queue, queuePaused, stopping, send]);

  const sendNext = useCallback(() => {
    const [first, ...rest] = queue;
    if (!first || running) return;
    setQueue(rest);
    setQueuePaused(false);
    send(first.text);
  }, [queue, running, send]);

  /* ---------------------------------------------------------------------------- navigation */

  const openChat = useCallback(
    (target: ChatSummary) => {
      const recorded = stateForChat(target, meta.states ?? []);
      if (recorded && recorded !== state) {
        openState(recorded);
        return;
      }
      if (recorded === state || target.id === workspace.activeChatId) {
        setOpened(null);
        return;
      }
      // No recording: the chat opens here, on its history, with a fresh composer for its agent.
      const agent = findAgent(workspace, target.agent);
      const model = agent?.models.find((entry) => entry.id === agent.defaultModel);
      resetLocal();
      setQueue([]);
      setTitle(null);
      setOpened(target);
      setPanel(null);
      update({
        agent: target.agent,
        model: model?.id ?? "",
        effort: model?.defaultEffort ?? null,
        draft: "",
        pickerOpen: false,
      });
    },
    [state, workspace, update, resetLocal],
  );

  const newChat = useCallback(() => {
    if (state !== "empty") {
      openState("empty");
      return;
    }
    resetLocal();
    setQueue([]);
    setTitle(null);
    setOpened(null);
  }, [state, resetLocal]);

  const archive = useCallback(
    (target: ChatSummary) => {
      setArchived((current) => new Set(current).add(target.id));
      say(`Archived ${target.title}`, {
        action: "Undo",
        keys: ["⌘", "Z"],
        onAction: () =>
          setArchived((current) => {
            const next = new Set(current);
            next.delete(target.id);
            return next;
          }),
      });
    },
    [say],
  );

  /* -------------------------------------------------------------------------------- layout */

  const { pinned, toBottom, stick } = useStickToBottom(
    scrollRef,
    `${model.eventCount}:${extras.length}:${now?.state ?? "none"}`,
  );

  // The transcript keeps clear of the dock, whatever its height, and stays at its end when the
  // dock grows (a segment arriving) while the reader was at the end.
  useLayoutEffect(() => {
    const dock = dockRef.current;
    const pane = paneRef.current;
    if (!dock || !pane) return;
    const apply = (): void => {
      pane.style.setProperty("--o-dock-h", `${dock.offsetHeight}px`);
      stick();
    };
    apply();
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver(apply);
    observer.observe(dock);
    return () => observer.disconnect();
  });

  // How wide the window is, so a right panel that would squeeze the chat floats over it instead.
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const measure = (): void => setRootWidth(Math.round(root.clientWidth) || 1440);
    measure();
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    return () => observer.disconnect();
  }, []);
  const sidebarWidth = rail ? 72 : 264;
  const panelFloats = rootWidth - sidebarWidth - panelWidth - 3 * 8 < MAIN_MIN;

  // The card folded away: the composer takes focus back, so typing goes where it did before.
  const showCard = !opened && (pending !== null || (answered !== null && cardOpen));
  const hadCard = useRef(showCard);
  useEffect(() => {
    if (hadCard.current && !showCard) {
      paneRef.current
        ?.querySelector<HTMLTextAreaElement>("[data-composer]")
        ?.focus({ preventScroll: true });
    }
    hadCard.current = showCard;
  }, [showCard]);

  // The open chat's row follows what happens here: a stop is idle, an answer runs on, a message
  // runs. A chat opened here was seen, so it is no longer done-unseen. Archived chats leave.
  const activeStatus: ChatStatus | null =
    actions.length > 0 && (workspace.activeChatId !== null || opened !== null)
      ? statusOf(model)
      : null;
  const sidebarWorkspace = useMemo(() => {
    const activeChatId = opened?.id ?? workspace.activeChatId;
    if (activeStatus === null && !opened && archived.size === 0) return workspace;
    return {
      ...workspace,
      activeChatId,
      projects: workspace.projects.map((entry) => ({
        ...entry,
        expanded: entry.expanded || entry.id === opened?.projectId,
        chats: entry.chats
          .filter((row) => !archived.has(row.id))
          .map((row) => {
            if (row.id !== activeChatId) return row;
            if (activeStatus !== null) return { ...row, status: activeStatus };
            return row.status === "done_unseen" ? { ...row, status: "idle" as const } : row;
          }),
      })),
    };
  }, [activeStatus, workspace, opened, archived]);

  const openChanges = useCallback((path?: string) => {
    setPanel((current) => ({
      tab: "changes",
      scope: current?.scope ?? "turn",
      expandedFile: path ?? current?.expandedFile ?? null,
    }));
  }, []);

  const copy = useCallback(
    (text: string, done: string) => {
      void navigator.clipboard?.writeText(text).catch(() => {});
      say(done);
    },
    [say],
  );

  // The turn whose edits both the header and the Changes panel show, and what a revert would undo.
  const reviewTurn = changesTurn(model);
  const reviewFiles = changedFiles(reviewTurn);
  const askRevert = useCallback(() => {
    if (!reviewTurn || changedFiles(reviewTurn).length === 0) {
      say("Nothing to revert in this chat");
      return;
    }
    setSheet({
      kind: "revert",
      title: `turn ${model.turns.indexOf(reviewTurn) + 1}`,
      project: projectName,
      files: changedFiles(reviewTurn).length,
      checkpoint: (model.session.nativeSessionId ?? "7f3a2c19").replace(/-/g, "").slice(0, 8),
    });
  }, [reviewTurn, model, projectName, say]);
  const copyPatch = useCallback(() => {
    if (reviewFiles.length === 0) {
      say("No changes to copy yet");
      return;
    }
    const patch = reviewFiles
      .map((file) => {
        const body = file.diff.hunks
          .map((hunk) =>
            [
              hunk.header,
              ...hunk.lines.map(
                (line) =>
                  `${line.op === "add" ? "+" : line.op === "remove" ? "-" : " "}${line.text}`,
              ),
            ].join("\n"),
          )
          .join("\n");
        return `--- a/${file.diff.path}\n+++ b/${file.diff.path}\n${body}`;
      })
      .join("\n");
    copy(patch, "Copied the patch");
  }, [reviewFiles, copy, say]);

  /* ---------------------------------------------------------------------------------- keys */

  // The window's keys: Esc one layer at a time, Cmd Shift A cycles autonomy, Cmd . opens the
  // picker, Cmd J toggles the right panel, Cmd N opens a new chat, Cmd \ folds the sidebar, and
  // the keys the interface shows on its keycaps: Cmd K, Cmd Shift C, Cmd Shift Z and Cmd ,.
  const keyState = useRef({
    composer,
    autonomyOpen,
    pending,
    running,
    stopping,
    beginStop,
    stop,
    liveAgent,
  });
  keyState.current = {
    composer,
    autonomyOpen,
    pending,
    running,
    stopping,
    beginStop,
    stop,
    liveAgent,
  };
  // ⌘⌫ archives the chat that is open, as its menus teach; a new chat has nothing to archive.
  const archiveOpen = useCallback(() => {
    if (chat && !empty) archive(chat);
  }, [chat, empty, archive]);
  const actionsRef = useRef({
    setAutonomy,
    newChat,
    copyPatch,
    askRevert,
    archiveOpen,
    soon,
    say,
    nameOf,
  });
  actionsRef.current = {
    setAutonomy,
    newChat,
    copyPatch,
    askRevert,
    archiveOpen,
    soon,
    say,
    nameOf,
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      const current = keyState.current;
      const act = actionsRef.current;
      const mod = event.metaKey || event.ctrlKey;
      const key = event.key.toLowerCase();
      if (mod && event.shiftKey && key === "a") {
        event.preventDefault();
        // A request on the card is answered on the card: a mode hotkey never answers it.
        if (current.pending) {
          act.say(`Answer ${act.nameOf(current.liveAgent)} first`);
          return;
        }
        const agent = findAgent(workspace, current.composer.agent);
        act.setAutonomy(nextAutonomy(current.composer.autonomy, agent?.autonomy ?? ["full_auto"]));
        return;
      }
      if (mod && event.shiftKey && key === "c") {
        event.preventDefault();
        act.copyPatch();
        return;
      }
      if (mod && event.shiftKey && key === "z") {
        event.preventDefault();
        act.askRevert();
        return;
      }
      if (mod && !event.shiftKey && event.key === "Backspace") {
        // In a field ⌘⌫ deletes to the line's start; only an untouched composer gives it up.
        const focus = focusOf(event.target);
        const blank =
          focus === "composer-empty" &&
          event.target instanceof HTMLTextAreaElement &&
          event.target.value === "";
        if (focus !== "other" && !blank) return;
        event.preventDefault();
        act.archiveOpen();
        return;
      }
      if (mod && !event.shiftKey && key === "k") {
        event.preventDefault();
        act.soon("Search");
        return;
      }
      if (mod && event.key === ",") {
        event.preventDefault();
        act.soon("Settings");
        return;
      }
      if (mod && event.key === ".") {
        event.preventDefault();
        setAutonomyOpen(false);
        update({ pickerOpen: !current.composer.pickerOpen });
        return;
      }
      if (mod && key === "j") {
        event.preventDefault();
        setPanel((open) => (open ? null : { tab: "changes", scope: "turn", expandedFile: null }));
        return;
      }
      if (mod && !event.shiftKey && key === "n") {
        event.preventDefault();
        act.newChat();
        return;
      }
      if (mod && event.key === "\\") {
        event.preventDefault();
        setRail((value) => !value);
        return;
      }
      if (event.key !== "Escape" || event.defaultPrevented) return;
      const action = escapeAction({
        pickerOpen: current.composer.pickerOpen,
        menuOpen: current.autonomyOpen,
        overlayOpen: document.querySelector('[data-orbit-menu], [aria-modal="true"]') !== null,
        permissionPending: current.pending !== null,
        running: current.running,
        stopping: current.stopping !== null,
        focus: focusOf(event.target),
      });
      if (action === "close-menu") {
        event.preventDefault();
        setAutonomyOpen(false);
      } else if (action === "stop") {
        event.preventDefault();
        current.beginStop();
      } else if (action === "force-stop") {
        event.preventDefault();
        current.stop();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [workspace, update]);

  // The empty state shows an agent's arrival only when you chose it there, never on first paint.
  const firstAgent = useRef(composer.agent);
  const [arrived, setArrived] = useState(false);
  if (composer.agent !== firstAgent.current && !arrived) setArrived(true);

  if (onboarding && workspace.discovery) {
    return (
      <div className="o-root" data-theme={theme} data-state={state} data-onboarding="">
        <Onboarding discovery={workspace.discovery} player={clock} theme={theme} />
      </div>
    );
  }

  const firstLocal = actions.find((action) => action.kind === "send");
  const headerTitle =
    title ??
    (opened
      ? opened.title
      : empty
        ? "New chat"
        : (model.session.title ??
          chat?.title ??
          (firstLocal?.kind === "send" ? firstLine(firstLocal.text) : "Chat")));
  // The header's diffstat is the same turn the Changes panel reviews, so the two always agree; it
  // shows as soon as the first edit lands.
  const changes =
    reviewFiles.length > 0
      ? {
          added: reviewFiles.reduce((sum, file) => sum + file.diff.added, 0),
          removed: reviewFiles.reduce((sum, file) => sum + file.diff.removed, 0),
        }
      : null;
  // The meter shows what the agent reported, and nothing before its first report: no made-up 0%.
  const context =
    model.usage?.context_used != null && model.usage.context_size
      ? { used: model.usage.context_used, size: model.usage.context_size }
      : null;
  const blocking = model.limits.find((window) => (window.used_percent ?? 0) >= 100) ?? null;
  const cardPermission = pending ?? (answered ? answered.request : null);
  const answerForCard = cardPermission ? (answers.get(cardPermission.requestId) ?? null) : null;
  const liveTurn = running ? lastTurn : null;
  const handoffSummary: HandoffSummary = {
    title: headerTitle,
    turns: model.turns.length,
    files: reviewFiles.length,
    added: changes?.added ?? 0,
    removed: changes?.removed ?? 0,
    lastAnswer: (() => {
      const answer = [...model.turns]
        .reverse()
        .map(messageText)
        .find((text) => text !== "");
      return answer ? firstLine(plainText(answer), 120) : null;
    })(),
    tokens: handoffTokens(model),
  };
  // A chat opened here that has no recorded turns shows a stub of its history above anything new.
  const historyStub = opened && !history ? opened : null;

  const composerView = (
    <Composer
      workspace={workspace}
      state={composer}
      onChange={update}
      onCommitChoice={commitChoice}
      player={clock}
      timeline={running ? timeline : null}
      chatAgent={empty ? null : chatAgent}
      running={running}
      stopping={stopping !== null}
      queue={queue}
      queuePaused={queuePaused}
      onSendNext={sendNext}
      handoffSummary={handoffSummary}
      plan={model.plan && model.plan.turnId === lastTurn?.id ? model.plan : null}
      planUpdated={model.plan ? formatClock(model.plan.updatedAt) : "0:00"}
      context={context}
      blocking={blocking}
      handoffTokens={handoffTokens(model)}
      autonomyOpen={autonomyOpen}
      onAutonomyOpen={setAutonomyOpen}
      onAutonomy={setAutonomy}
      onSend={(text) => {
        send(text);
        update({ draft: "" });
      }}
      onQueue={(text) => {
        setQueue((previous) => [...previous, { id: nextId.current++, text }]);
        update({ draft: "" });
      }}
      onSteer={steer}
      onStop={stop}
      onEditQueued={(message) => {
        setQueue((previous) => previous.filter((entry) => entry.id !== message.id));
        update({ draft: message.text });
      }}
      onRemoveQueued={(message) =>
        setQueue((previous) => previous.filter((entry) => entry.id !== message.id))
      }
      onKeepAgent={() => {
        const own = chatAgent ? findAgent(workspace, chatAgent) : null;
        if (!own || !chatAgent) return;
        const fallback = own.models.find((entry) => entry.id === own.defaultModel) ?? own.models[0];
        const original = workspace.composer.agent === chatAgent ? workspace.composer : null;
        update({
          agent: chatAgent,
          model: original?.model ?? fallback?.id ?? "",
          effort: original ? original.effort : (fallback?.defaultEffort ?? null),
        });
      }}
      onSwitchAgent={() => update({ pickerOpen: true })}
      autoFocus={empty || opened !== null}
      onSoon={soon}
      onCopy={copy}
    />
  );

  return (
    <div
      ref={rootRef}
      className="o-root"
      data-theme={theme}
      data-state={state}
      data-panel={panel ? "" : undefined}
      data-panel-floats={panel && panelFloats ? "" : undefined}
      data-empty={empty ? "" : undefined}
      data-rail={rail ? "" : undefined}
      style={{ "--o-panel-w": `${panelWidth}px` } as CSSProperties}
    >
      <div className="o-desk" aria-hidden="true" />
      <Sidebar
        workspace={sidebarWorkspace}
        player={clock}
        liveAgent={running ? liveAgent : null}
        timeline={timeline}
        runningSince={lastTurn?.startedAt ?? null}
        rail={rail}
        onToggleRail={() => setRail((value) => !value)}
        onOpenChat={openChat}
        onNewChat={newChat}
        onArchive={archive}
        onDelete={(target) =>
          setSheet({ kind: "delete", title: target.title, project: projectName, chatId: target.id })
        }
        onSoon={soon}
      />
      <main className="o-pane o-main" ref={paneRef}>
        <Header
          agent={empty ? composer.agent : liveAgent}
          agentName={nameOf(empty ? composer.agent : liveAgent)}
          title={headerTitle}
          project={projectName}
          branch="main"
          player={clock}
          timeline={!empty && running ? timeline : null}
          changes={changes}
          panelOpen={panel !== null}
          scrolled={scrolled}
          onTogglePanel={() =>
            setPanel((open) =>
              open ? null : { tab: "changes", scope: "turn", expandedFile: null },
            )
          }
          onOpenChanges={() => openChanges()}
          onRename={(next) => setTitle(next)}
          onArchive={chat && !empty ? () => archive(chat) : null}
          onDelete={() =>
            setSheet({
              kind: "delete",
              title: headerTitle,
              project: projectName,
              chatId: chat?.id ?? null,
            })
          }
        />
        {empty ? (
          <EmptyState
            workspace={workspace}
            agent={composer.agent}
            agentName={nameOf(composer.agent)}
            projectName={projectName}
            switched={arrived}
            onOpenChat={openChat}
          >
            {composerView}
          </EmptyState>
        ) : (
          <>
            <div
              className="o-chat-scroll o-scroll"
              ref={scrollRef}
              data-lab-scroll=""
              onScroll={(event) => setScrolled(event.currentTarget.scrollTop > 0)}
            >
              <Transcript
                model={model}
                workspace={workspace}
                player={clock}
                timeline={timeline}
                liveTurnId={liveTurn?.id ?? null}
                extras={extras}
                now={now}
                hideWaitingCall={showCard && pending !== null}
                denied={denied}
                runFrom={runFrom}
                history={historyStub}
                scrollRef={scrollRef}
                onOpenChanges={openChanges}
                onRetry={(text) => {
                  if (!running) send(text);
                }}
                onEdit={(text) => {
                  update({ draft: text });
                  paneRef.current
                    ?.querySelector<HTMLTextAreaElement>("[data-composer]")
                    ?.focus({ preventScroll: true });
                }}
                onCopy={(turn, as) =>
                  as === "markdown"
                    ? copy(messageText(turn), "Copied as Markdown")
                    : copy(plainText(messageText(turn)), "Copied the answer")
                }
                onCopyReceipt={(turn) =>
                  copy(
                    receiptText(turn, {
                      agent: nameOf(turn.agent ?? liveAgent),
                      title: headerTitle,
                      session: model.session.nativeSessionId,
                    }),
                    "Copied the receipt",
                  )
                }
                onCopyText={copy}
                onContinue={() => {
                  const codex = findAgent(workspace, "codex");
                  const codexModel =
                    codex?.models.find((entry) => entry.id === codex.defaultModel) ?? null;
                  commitChoice({
                    agent: "codex",
                    model: codexModel?.id ?? "",
                    effort: codexModel?.defaultEffort ?? null,
                  });
                  paneRef.current
                    ?.querySelector<HTMLTextAreaElement>("[data-composer]")
                    ?.focus({ preventScroll: true });
                }}
                onSoon={soon}
              />
            </div>
            <div className="o-fade-top" aria-hidden="true" />
            <div className="o-fade-bottom" aria-hidden="true" />
            <div className="o-dock" ref={dockRef}>
              {!pinned && running ? (
                <button type="button" className="o-latest o-t-meta" onClick={toBottom}>
                  <span aria-hidden="true">{"↓"}</span> Latest
                </button>
              ) : null}
              <AnimatePresence initial={false} mode="wait">
                {showCard && cardPermission ? (
                  <motion.div
                    key="card"
                    className="o-card-slot"
                    initial={{ opacity: 0, y: 8 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{
                      opacity: 0,
                      y: 8,
                      transition: { duration: 0.12, ease: [0.4, 0, 1, 1] },
                    }}
                    transition={{ duration: reduced ? 0.12 : 0.26, ease: [0.23, 1, 0.32, 1] }}
                  >
                    <PermissionCard
                      permission={cardPermission}
                      agentName={nameOf(liveAgent)}
                      projectName={projectName}
                      autonomy={askedUnder}
                      player={clock}
                      answer={answerForCard}
                      onAnswer={answerRequest}
                    />
                  </motion.div>
                ) : (
                  <motion.div
                    key="composer"
                    className="o-card-slot"
                    initial={{ opacity: 0, y: 8 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{
                      opacity: 0,
                      y: 8,
                      transition: { duration: 0.12, ease: [0.4, 0, 1, 1] },
                    }}
                    transition={{ duration: reduced ? 0.12 : 0.26, ease: [0.23, 1, 0.32, 1] }}
                  >
                    {composerView}
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
          </>
        )}
        <Toast message={toast} onDone={() => setToast(null)} />
      </main>
      {panel ? (
        <RightPanel
          panel={panel}
          model={model}
          workspace={workspace}
          agent={liveAgent}
          agentName={nameOf(liveAgent)}
          autonomy={composer.autonomy}
          project={projectName}
          width={panelWidth}
          onWidth={setPanelWidth}
          onPanel={setPanel}
          onClose={() => setPanel(null)}
          onRevert={() => askRevert()}
          onCopy={copy}
        />
      ) : null}
      <AnimatePresence>
        {sheet ? (
          <Sheet
            key="sheet"
            request={sheet}
            onCancel={() => setSheet(null)}
            onConfirm={() => {
              setSheet(null);
              if (sheet.kind === "revert") {
                // The revert is logged where it happened; Undo takes it back.
                const id = `x${nextId.current++}`;
                setExtras((previous) => [
                  ...previous,
                  {
                    kind: "event",
                    id,
                    anchor: anchorAfter(model.turns),
                    icon: "level",
                    text: `Reverted ${sheet.title}`,
                    literal: `checkpoint ${sheet.checkpoint}`,
                  },
                ]);
                say(`Reverted ${sheet.title} in ${sheet.project}`, {
                  action: "Undo",
                  keys: ["⌘", "Z"],
                  onAction: () =>
                    setExtras((previous) => previous.filter((extra) => extra.id !== id)),
                });
                return;
              }
              const gone = sheet.chatId;
              if (gone) setArchived((current) => new Set(current).add(gone));
              say(`Deleted ${sheet.title}`, {
                action: "Undo",
                keys: ["⌘", "Z"],
                onAction: () =>
                  setArchived((current) => {
                    const next = new Set(current);
                    if (gone) next.delete(gone);
                    return next;
                  }),
              });
            }}
          />
        ) : null}
      </AnimatePresence>
      <p className="o-sr" role="status">
        {pending
          ? `${nameOf(liveAgent)} needs you: ${pending.title}`
          : running
            ? `${nameOf(liveAgent)} is working`
            : lastTurn?.status === "completed"
              ? `${nameOf(liveAgent)} finished`
              : lastTurn?.status === "failed"
                ? `${nameOf(liveAgent)} stopped with an error`
                : ""}
      </p>
    </div>
  );
}
