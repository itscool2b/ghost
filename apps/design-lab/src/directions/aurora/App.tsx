import "./aurora.css";
import {
  type AgentBrandId,
  AuraField,
  type AuraIntensity,
  brandCssVariables,
  getBrand,
  type OrbState,
  springs,
  usePrefersReducedMotion,
} from "@ghost/ui";
import { GitCompareArrows, ListEnd, PanelRight, X } from "lucide-react";
import { AnimatePresence, animate, motion } from "motion/react";
import {
  type CSSProperties,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AUTONOMY_LABELS,
  type Autonomy,
  type ChatStatus,
  type DirectionProps,
  findAgent,
  findChat,
  findProject,
  formatDuration,
  type PermissionOption,
  type TranscriptModel,
  type Workspace,
} from "../../kit";
import { Composer, type ComposerProps } from "./Composer";
import { Glass } from "./Glass";
import { buildHandoff, type SentMessage } from "./Handoff";
import { Hero } from "./Hero";
import { optionForKey } from "./Permission";
import { recall, type Selection, type SelectionMemory } from "./Picker";
import { PlanCard } from "./Plan";
import { Sidebar } from "./Sidebar";
import { type PendingAnswer, Transcript, type TranscriptKeys } from "./Transcript";
import { DiffStat, IconButton, LiveContext } from "./ui";

/** True when keys belong to a text field (typing Enter there must never answer a card). */
function typingInto(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName))
  );
}

/** Lines added and removed across every edit in the chat so far, for the header's changes chip. */
function changesOf(model: TranscriptModel): { files: number; added: number; removed: number } {
  const files = new Set<string>();
  let added = 0;
  let removed = 0;
  for (const turn of model.turns) {
    for (const item of turn.items) {
      if (item.kind !== "tools") continue;
      for (const path of item.files) files.add(path);
      added += item.added;
      removed += item.removed;
    }
  }
  return { files: files.size, added, removed };
}

function withChatStatus(
  workspace: Workspace,
  chatId: string | null,
  status: ChatStatus,
): Workspace {
  if (chatId === null) return workspace;
  return {
    ...workspace,
    projects: workspace.projects.map((project) =>
      project.chats.some((chat) => chat.id === chatId)
        ? {
            ...project,
            chats: project.chats.map((chat) => (chat.id === chatId ? { ...chat, status } : chat)),
          }
        : project,
    ),
  };
}

/** What the window says when an agent cannot keep the autonomy the user chose. */
function autonomyNoteFor(agentName: string, chosen: Autonomy, actual: Autonomy, only: boolean) {
  if (actual === "full_auto" && only) {
    return `${agentName} always runs in Full auto. It edits and runs commands without asking.`;
  }
  return `${agentName} cannot use ${AUTONOMY_LABELS[chosen]}, so it runs in ${AUTONOMY_LABELS[actual]}.`;
}

/** The first line of a message, short enough for a title. */
function titleOf(text: string): string {
  const line = text.split("\n")[0]?.trim() ?? "";
  return line.length > 60 ? `${line.slice(0, 58).trimEnd()}…` : line;
}

/**
 * Where a message left from, measured just before it went: the hero (its orb and composer fly
 * into the conversation), or a queued row (its bubble rises from the dock into the transcript).
 */
type Launch =
  | { readonly kind: "hero"; readonly orb: DOMRect; readonly composer: DOMRect }
  | { readonly kind: "queue"; readonly row: DOMRect; readonly id: string };

interface Queued {
  readonly id: string;
  readonly text: string;
}

let nextLocalId = 0;
function localId(prefix: string): string {
  nextLocalId += 1;
  return `${prefix}-${nextLocalId}`;
}

/**
 * Aurora: immersive light behind heavy glass. The active agent's colour is a slow light field behind
 * the whole window; the UI is three layers of glass over it (the frosted sidebar, the floating
 * transcript sheet, the floating composer and its popovers). Thinking brightens the field, waiting
 * turns it amber, and choosing another agent cross-fades it.
 */
export function App({ state, model, workspace, player }: DirectionProps) {
  const [live, setLive] = useState(false);
  useEffect(() => setLive(true), []);
  const reduced = usePrefersReducedMotion();

  const chat = findChat(workspace, workspace.activeChatId);
  const project = findProject(workspace, workspace.activeProjectId);
  const lastTurn = model.turns[model.turns.length - 1] ?? null;

  /* ---------------------------------------------------------------- what the next message uses */

  const [selection, setSelection] = useState<Selection>(() => ({
    agent: workspace.composer.agent,
    model: workspace.composer.model,
    effort: workspace.composer.effort,
  }));
  const [memory, setMemory] = useState<SelectionMemory>(
    () =>
      new Map([
        [
          workspace.composer.agent,
          { model: workspace.composer.model, effort: workspace.composer.effort },
        ],
      ]),
  );
  // The level the user chose, and the one the selected agent can actually run with.
  const [preferred, setPreferred] = useState<Autonomy>(workspace.composer.autonomy);
  const agentInfo = findAgent(workspace, selection.agent);
  const supported = agentInfo?.autonomy ?? ["full_auto"];
  const autonomy: Autonomy = supported.includes(preferred)
    ? preferred
    : (supported[0] ?? "full_auto");
  const agentName = agentInfo?.name ?? getBrand(selection.agent).name;

  const choose = useCallback((next: Selection) => {
    setSelection(next);
    setMemory((previous) =>
      new Map(previous).set(next.agent, { model: next.model, effort: next.effort }),
    );
  }, []);
  const chooseAgent = useCallback(
    (agent: AgentBrandId) => {
      const next = recall(workspace, memory, agent);
      if (next) choose(next);
    },
    [workspace, memory, choose],
  );

  // The chip pulses once whenever the level changes without the user picking it.
  const [pulse, setPulse] = useState(0);
  const previousLevels = useRef({ autonomy, preferred });
  useEffect(() => {
    const before = previousLevels.current;
    previousLevels.current = { autonomy, preferred };
    if (autonomy !== before.autonomy && preferred === before.preferred) {
      setPulse((value) => value + 1);
    }
  }, [autonomy, preferred]);

  const [pickerOpen, setPickerOpen] = useState(workspace.composer.pickerOpen);
  const [autonomyOpen, setAutonomyOpen] = useState(false);
  const [preview, setPreview] = useState<AgentBrandId | null>(null);
  const onPreview = useCallback((agent: AgentBrandId | null) => setPreview(agent), []);
  const [typing, setTyping] = useState(false);
  const [prefill, setPrefill] = useState<{ text: string; nonce: number } | null>(null);
  const fill = useCallback(
    (text: string) => setPrefill((previous) => ({ text, nonce: (previous?.nonce ?? 0) + 1 })),
    [],
  );

  /* ---------------------------------------------------------- a request answered in this window */

  const pending = model.pendingPermissions[0] ?? null;
  const [answer, setAnswer] = useState<PendingAnswer | null>(null);
  const pendingId = pending?.requestId ?? null;
  useEffect(() => {
    setAnswer((current) => (current && current.requestId === pendingId ? current : null));
  }, [pendingId]);
  const answered = pending !== null && answer !== null && answer.requestId === pending.requestId;
  const allowed = answered && answer.option.kind.startsWith("allow");
  const rejected = answered && !allowed;
  const onAnswer = useCallback(
    (option: PermissionOption) => {
      if (pendingId !== null) setAnswer({ requestId: pendingId, option });
    },
    [pendingId],
  );
  const unanswered = answered ? null : pending;

  // The card's keys from anywhere but a text field (the composer handles its own).
  useEffect(() => {
    if (!unanswered) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.defaultPrevented || typingInto(event.target) || event.metaKey || event.ctrlKey) {
        return;
      }
      const option =
        event.key === "Enter"
          ? optionForKey(unanswered.options, event.altKey ? "always" : "allow")
          : event.key === "Escape"
            ? optionForKey(unanswered.options, "reject")
            : null;
      if (!option) return;
      event.preventDefault();
      onAnswer(option);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [unanswered, onAnswer]);

  /* ----------------------------------------------------- messages sent and queued in this window */

  const [sent, setSent] = useState<readonly SentMessage[]>([]);
  const [stopped, setStopped] = useState<ReadonlySet<string>>(() => new Set());
  const [queue, setQueue] = useState<readonly Queued[]>([]);
  // The recording starting over (a loop, a seek back) clears what this window added to it.
  const eventCount = useRef(model.eventCount);
  useEffect(() => {
    if (model.eventCount < eventCount.current) {
      setSent([]);
      setStopped(new Set());
      setQueue([]);
    }
    eventCount.current = model.eventCount;
  }, [model.eventCount]);

  const lastSent = sent[sent.length - 1] ?? null;
  const sentRunning = lastSent !== null && !stopped.has(lastSent.id);
  const recordedRunning =
    lastTurn !== null &&
    (lastTurn.status === "running" || (lastTurn.status === "waiting" && !rejected));
  const running = recordedRunning || sentRunning;
  // The agent the chat last went to: the next message to anyone else carries a handoff.
  const lastAgent = lastSent?.agent ?? lastTurn?.agent ?? null;
  const switching = lastAgent !== null && lastAgent !== selection.agent;
  const handoff = useMemo(
    () => (switching && lastAgent ? buildHandoff(model, sent, lastAgent, selection.agent) : null),
    [switching, lastAgent, model, sent, selection.agent],
  );
  const autonomyNote =
    autonomy !== preferred && lastAgent !== selection.agent
      ? autonomyNoteFor(agentName, preferred, autonomy, supported.length === 1)
      : null;

  const empty = model.turns.length === 0 && sent.length === 0;
  const [heroLeaving, setHeroLeaving] = useState(false);
  const onHeroLeft = useCallback(() => setHeroLeaving(false), []);
  const launch = useRef<Launch | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  const sendNow = useCallback(
    (text: string, id = localId("sent")) => {
      if (empty && !reduced) {
        // Measure the hero before it goes: the orb and the composer fly from here.
        const root = rootRef.current;
        const orb = root?.querySelector("[data-hero-orb]")?.getBoundingClientRect();
        const composer = root?.querySelector(".au-hero .au-composer-wrap")?.getBoundingClientRect();
        if (orb && composer) launch.current = { kind: "hero", orb, composer };
      }
      if (empty) setHeroLeaving(true);
      setSent((previous) => [
        ...previous,
        {
          id,
          text,
          agent: selection.agent,
          model: selection.model,
          effort: selection.effort,
          autonomy,
          handoff,
          fromHero: empty,
        },
      ]);
      setPickerOpen(false);
      setAutonomyOpen(false);
    },
    [empty, reduced, selection, autonomy, handoff],
  );
  const onSend = useCallback(
    (text: string) => {
      if (running) setQueue((previous) => [...previous, { id: localId("queued"), text }]);
      else sendNow(text);
    },
    [running, sendNow],
  );
  // A queued message goes as soon as nothing runs, rising from its row into the transcript.
  useEffect(() => {
    const [first] = queue;
    if (running || !first) return;
    const id = localId("sent");
    const row = rootRef.current?.querySelector(".au-queue-row")?.getBoundingClientRect();
    if (row && !reduced) launch.current = { kind: "queue", row, id };
    setQueue((previous) => previous.slice(1));
    sendNow(first.text, id);
  }, [running, queue, sendNow, reduced]);
  const onStop = useCallback(() => {
    // Only what this window started can stop; the recording plays on regardless.
    if (lastSent && sentRunning) setStopped((previous) => new Set(previous).add(lastSent.id));
  }, [lastSent, sentRunning]);
  // Escape stops from anywhere, as it does in the composer (menus and cards take it first).
  useEffect(() => {
    if (!sentRunning || unanswered || pickerOpen || autonomyOpen) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || event.defaultPrevented || typingInto(event.target)) return;
      event.preventDefault();
      onStop();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sentRunning, unanswered, pickerOpen, autonomyOpen, onStop]);

  // Flights, once the sent message is on screen: the first message's (the hero orb into the
  // gutter, the composer down to its dock) and a queued one's (its bubble up from the dock).
  useLayoutEffect(() => {
    const from = launch.current;
    if (!from || sent.length === 0) return;
    launch.current = null;
    const root = rootRef.current;
    const sheet = root?.querySelector<HTMLElement>(".au-sheet");
    if (!sheet) return;
    // Rects are in screen px; the window may be scaled as a whole, transforms are in layout px.
    const scale = sheet.getBoundingClientRect().width / sheet.offsetWidth || 1;
    if (from.kind === "queue") {
      const bubble = root?.querySelector<HTMLElement>(
        `[data-sent-id="${from.id}"] .au-user-bubble`,
      );
      if (!bubble) return;
      const to = bubble.getBoundingClientRect();
      void animate(
        bubble,
        {
          x: [(from.row.right - to.right) / scale, 0],
          y: [(from.row.top - to.top) / scale, 0],
        },
        springs.smooth,
      );
      return;
    }
    const dock = root?.querySelector<HTMLElement>(".au-dock .au-composer-wrap");
    const gutter = root?.querySelector<HTMLElement>(".au-scroll .au-gutter-orb");
    if (!dock || !gutter) return;
    const to = dock.getBoundingClientRect();
    void animate(
      dock,
      {
        x: [(from.composer.left - to.left) / scale, 0],
        y: [(from.composer.top - to.top) / scale, 0],
      },
      springs.smooth,
    );
    const orb = gutter.getBoundingClientRect();
    const centre = (rect: DOMRect) => [rect.left + rect.width / 2, rect.top + rect.height / 2];
    const [fromX = 0, fromY = 0] = centre(from.orb);
    const [toX = 0, toY = 0] = centre(orb);
    void animate(
      gutter,
      {
        x: [(fromX - toX) / scale, 0],
        y: [(fromY - toY) / scale, 0],
        scale: [from.orb.width / orb.width, 1],
      },
      springs.smooth,
    );
  }, [sent]);

  /* ----------------------------------------------------------------------- light and activity */

  // The recorded turn, played forward with the answer given here; then whatever this window sent.
  const recordedActivity: OrbState = answered ? (allowed ? "tool" : "idle") : model.activity;
  const activity: OrbState = sentRunning ? "thinking" : recordedActivity;
  const aura: AuraIntensity = sentRunning
    ? "active"
    : answered
      ? allowed
        ? "active"
        : "calm"
      : model.aura;
  const runningAgent = sentRunning
    ? (lastSent?.agent ?? null)
    : recordedRunning
      ? (lastTurn?.agent ?? null)
      : null;
  const auraAgent = runningAgent ?? preview ?? selection.agent;
  const brand = getBrand(auraAgent);
  const changes = useMemo(() => changesOf(model), [model]);

  const chatStatus: ChatStatus | null = sentRunning
    ? "running"
    : answered
      ? allowed
        ? "running"
        : "idle"
      : null;
  const sidebarWorkspace = useMemo(
    () =>
      chatStatus === null
        ? workspace
        : withChatStatus(workspace, workspace.activeChatId, chatStatus),
    [workspace, chatStatus],
  );

  /* ------------------------------------------------------------------- what screen readers hear */

  const [announcement, setAnnouncement] = useState("");
  const heard = useRef<{ request: string | null; turn: string | null } | null>(null);
  const turnEnd =
    lastTurn?.footer && lastTurn.agent
      ? `${getBrand(lastTurn.agent).name} ${lastTurn.footer.stopReason === "end_turn" ? "finished" : "stopped"} in ${formatDuration(lastTurn.footer.durationMs)}`
      : null;
  const request = unanswered
    ? `${getBrand(lastTurn?.agent ?? "generic").name} needs your approval: ${unanswered.title}`
    : null;
  useEffect(() => {
    // What was already on screen when the window opened is not news.
    if (heard.current === null) {
      heard.current = { request, turn: turnEnd };
      return;
    }
    if (request && request !== heard.current.request) setAnnouncement(request);
    else if (turnEnd && turnEnd !== heard.current.turn) setAnnouncement(turnEnd);
    heard.current = { request, turn: turnEnd };
  }, [request, turnEnd]);

  /* --------------------------------------------------------------------------------- layout */

  // The dock's height feeds the transcript's end padding and fade, written straight to the sheet
  // (no React render per pixel while the plan grows or the composer gains a line).
  const dockRef = useCallback((dock: HTMLDivElement | null) => {
    const sheet = dock?.closest<HTMLElement>(".au-sheet");
    if (!dock || !sheet) return;
    const apply = (): void => sheet.style.setProperty("--au-dock-h", `${dock.offsetHeight}px`);
    apply();
    const observer = new ResizeObserver(apply);
    observer.observe(dock);
    return () => observer.disconnect();
  }, []);
  const keys = useRef<TranscriptKeys | null>(null);

  const composerProps: Omit<ComposerProps, "hero"> = {
    workspace,
    selection,
    autonomy,
    running,
    orbState: running && runningAgent === selection.agent ? activity : "idle",
    pickerOpen,
    onPickerOpen: setPickerOpen,
    autonomyOpen,
    onAutonomyOpen: setAutonomyOpen,
    memory,
    onCommit: choose,
    onAutonomy: setPreferred,
    onPreview,
    pending: unanswered,
    onAnswer,
    onSend,
    onStop,
    onTyping: setTyping,
    handoff,
    autonomyNote,
    pulse,
    keys,
    prefill,
  };

  const title =
    chat?.title ?? model.session.title ?? (sent[0] ? titleOf(sent[0].text) : "New chat");
  const overlay = pickerOpen || autonomyOpen;
  const plan =
    model.plan && lastTurn && model.plan.turnId === lastTurn.id && recordedRunning
      ? model.plan
      : null;

  return (
    <LiveContext.Provider value={live}>
      <div
        ref={rootRef}
        className="au-root"
        data-state={state}
        data-activity={activity}
        data-aura={aura}
        style={brandCssVariables(brand) as CSSProperties}
      >
        <div className="au-backdrop" aria-hidden="true">
          <div className="au-tint" />
          <AuraField brand={brand} intensity={aura} className="au-aura" />
          <div className="au-backlight" />
          <div className="au-backlight-alert" />
          <div className="au-vignette" />
          <div className="au-grain" />
        </div>

        <Sidebar
          workspace={sidebarWorkspace}
          activeAgent={selection.agent}
          onSelectAgent={chooseAgent}
        />

        <main className="au-main">
          <Glass
            as="section"
            level={2}
            radius={18}
            className="au-sheet"
            data-overlay={overlay ? "" : undefined}
            aria-label={title}
          >
            <header className="au-header">
              <div className="au-header-title">
                <span className="au-title">{title}</span>
                {project ? (
                  <span className="au-crumb" style={{ "--au-dot": project.color } as CSSProperties}>
                    {project.name}
                  </span>
                ) : null}
              </div>
              <div className="au-header-actions">
                <AnimatePresence initial={false}>
                  {changes.files > 0 ? (
                    <motion.button
                      key="changes"
                      type="button"
                      className="au-changes"
                      data-tip={`${changes.files === 1 ? "1 file" : `${changes.files} files`} changed`}
                      initial={{ opacity: 0, scale: 0.9 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.9 }}
                    >
                      <GitCompareArrows size={14} strokeWidth={1.75} />
                      <DiffStat added={changes.added} removed={changes.removed} />
                    </motion.button>
                  ) : null}
                </AnimatePresence>
                <IconButton label="Show side panel">
                  <PanelRight size={16} strokeWidth={1.75} />
                </IconButton>
              </div>
            </header>

            {empty ? null : (
              <>
                <Transcript
                  model={model}
                  workspace={workspace}
                  player={player}
                  activity={recordedActivity}
                  pendingAnswer={answered ? answer : null}
                  typing={typing}
                  onAnswer={onAnswer}
                  sent={sent}
                  stoppedIds={stopped}
                  keysRef={keys}
                />
                <div className="au-dock" ref={dockRef} data-overlay={overlay ? "" : undefined}>
                  <AnimatePresence initial={false}>
                    {plan ? (
                      <motion.div
                        key="plan"
                        className="au-dock-plan"
                        initial={{ opacity: 0, y: 14 }}
                        animate={{ opacity: 1, y: 0 }}
                        // The turn ended: the plan stays a moment, then leaves for the footer.
                        exit={{
                          opacity: 0,
                          y: 10,
                          transition: { delay: reduced ? 0 : 1.5, duration: 0.3 },
                        }}
                        transition={springs.smooth}
                      >
                        <PlanCard plan={plan} />
                      </motion.div>
                    ) : null}
                  </AnimatePresence>
                  <AnimatePresence initial={false}>
                    {queue.length > 0 ? (
                      <motion.div
                        key="queue"
                        className="au-queue-list"
                        initial={{ opacity: 0, y: 12 }}
                        animate={{ opacity: 1, y: 0 }}
                        exit={{ opacity: 0, y: -8 }}
                        transition={springs.smooth}
                      >
                        {queue.map((item) => (
                          <Glass key={item.id} level={2} radius={16} className="au-queue-row">
                            <ListEnd size={15} strokeWidth={1.75} />
                            <span className="au-queue-label">Queued</span>
                            <span className="au-queue-text">{item.text}</span>
                            <button
                              type="button"
                              className="au-queue-edit"
                              onClick={() => {
                                setQueue((previous) => previous.filter((q) => q.id !== item.id));
                                fill(item.text);
                              }}
                            >
                              Edit
                            </button>
                            <IconButton
                              label="Remove from queue"
                              tip="top"
                              onClick={() =>
                                setQueue((previous) => previous.filter((q) => q.id !== item.id))
                              }
                            >
                              <X size={14} strokeWidth={1.75} />
                            </IconButton>
                          </Glass>
                        ))}
                      </motion.div>
                    ) : null}
                  </AnimatePresence>
                  <Composer {...composerProps} autoFocus={sent[0]?.fromHero === true} />
                </div>
              </>
            )}
            {empty || heroLeaving ? (
              <Hero
                agent={preview ?? selection.agent}
                agentName={getBrand(preview ?? selection.agent).name}
                project={project}
                composer={composerProps}
                onSuggest={fill}
                leaving={heroLeaving}
                onLeft={onHeroLeft}
              />
            ) : null}
          </Glass>
        </main>
        <p className="au-sr" aria-live="polite">
          {announcement}
        </p>
      </div>
    </LiveContext.Provider>
  );
}
