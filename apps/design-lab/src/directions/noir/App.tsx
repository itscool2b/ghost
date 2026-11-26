import "./noir.css";
import {
  AuraField,
  type AuraIntensity,
  brandCssVariables,
  GlassPanel,
  getBrand,
  Orb,
  type OrbState,
} from "@ghost/ui";
import { Check, ChevronDown } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import {
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  type DirectionProps,
  findAgent,
  findChat,
  findModel,
  findProject,
  type PermissionModel,
  type PermissionOption,
  type Project,
} from "../../kit";
import { Composer } from "./Composer";
import { handoffFor } from "./handoff";
import { ownsKeys, PermissionAnswersProvider, rejectOption } from "./Permission";
import { PlanCard } from "./Plan";
import { Sidebar } from "./Sidebar";
import type { ComposerState } from "./state";
import { Transcript } from "./Transcript";
import { captureWithoutBackdrop, Kbd, popIn, useDismiss, useReduced } from "./ui";

/**
 * Noir: smoked glass, editorial calm. A near-black window where the agent's orb is the only
 * light; colour appears as a thin accent (a caret, a rule, a focus ring) or where it means
 * something (a diff, an approval). Instrument Serif for display moments, Geist for the rest.
 *
 * Each lab state gets a fresh window (keyed below), so local choices such as an open picker or a
 * typed draft never leak from one state into another.
 */
export function App(props: DirectionProps) {
  return <NoirWindow key={props.state} {...props} />;
}

function NoirWindow({ state, model, workspace, player }: DirectionProps) {
  const [composer, setComposer] = useState<ComposerState>(() => ({
    ...workspace.composer,
    queue: [],
  }));
  const update = useCallback((change: Partial<ComposerState>) => {
    setComposer((previous) => ({ ...previous, ...change }));
  }, []);

  // Answers to permission requests, given on the card or from the composer.
  const [answers, setAnswers] = useState<ReadonlyMap<string, PermissionOption>>(() => new Map());
  const answer = useCallback((permission: PermissionModel, option: PermissionOption) => {
    setAnswers((previous) => new Map(previous).set(permission.requestId, option));
  }, []);
  const permissionAnswers = useMemo(() => ({ answers, answer }), [answers, answer]);

  const chat = findChat(workspace, workspace.activeChatId);
  const project = findProject(workspace, workspace.activeProjectId);
  const lastTurn = model.turns[model.turns.length - 1] ?? null;
  const chatAgent = lastTurn?.agent ?? chat?.agent ?? composer.agent;
  const pending = model.pendingPermissions.find((request) => !answers.has(request.requestId));
  // The recording ends on its question: once it is answered here, the agent goes back to work.
  const resumed = model.activity === "waiting" && pending === undefined;
  const activity: OrbState = resumed ? "thinking" : model.activity;
  const intensity: AuraIntensity = resumed ? "active" : model.aura;
  const busy =
    lastTurn !== null && (lastTurn.status === "running" || lastTurn.status === "waiting");
  // The orb and the aura show the chat's state while the composer still points at its agent.
  const sameAgent = composer.agent === chatAgent;
  const orbState = sameAgent ? activity : "idle";
  const aura = sameAgent ? intensity : "calm";
  const brand = getBrand(composer.agent);
  const empty = model.turns.length === 0;
  const title = model.session.title ?? chat?.title ?? null;
  const [capture] = useState(captureWithoutBackdrop);

  // While the transcript's own headline is on screen, the title bar leaves the title to it. The
  // transcript decides before its first paint; transitions start a frame after that.
  const [headline, setHeadline] = useState<boolean | null>(null);
  const [headSettled, setHeadSettled] = useState(false);
  useEffect(() => {
    if (headline === null || headSettled) return;
    const frame = requestAnimationFrame(() => setHeadSettled(true));
    return () => cancelAnimationFrame(frame);
  }, [headline, headSettled]);

  const stop = useCallback(() => {
    if (player.playing) player.pause();
  }, [player]);

  // Esc stops a running turn, unless a request, a card or a field has a use for the key.
  const canStop = busy && pending === undefined;
  useEffect(() => {
    if (!canStop) return;
    const onKey = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || event.defaultPrevented || ownsKeys(event.target)) return;
      stop();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canStop, stop]);

  // The transcript keeps clear of the composer dock, whatever its height. A callback ref, so the
  // measurement follows the dock if it is ever replaced (the new-chat hero has its own).
  const dockObserver = useRef<ResizeObserver | null>(null);
  const dockRef = useCallback((dock: HTMLDivElement | null) => {
    dockObserver.current?.disconnect();
    dockObserver.current = null;
    const main = dock?.closest<HTMLElement>(".n-main");
    if (!dock || !main) return;
    const apply = (): void => main.style.setProperty("--n-dock-h", `${dock.offsetHeight}px`);
    apply();
    if (typeof ResizeObserver !== "function") return;
    dockObserver.current = new ResizeObserver(apply);
    dockObserver.current.observe(dock);
  }, []);

  // Switching agents in a chat: the next message carries a visible handoff, until one is queued.
  const handoffQueued = composer.queue.some((message) => message.handoffFrom !== null);
  const handoff = useMemo(
    () =>
      empty || sameAgent || handoffQueued ? null : handoffFor(model, chatAgent, composer.agent),
    [empty, sameAgent, handoffQueued, model, chatAgent, composer.agent],
  );
  const keepAgent = useCallback(() => {
    const own = workspace.composer.agent === chatAgent ? workspace.composer : null;
    const info = findAgent(workspace, chatAgent);
    const fallback = findModel(info, info?.defaultModel ?? null) ?? info?.models[0] ?? null;
    update({
      agent: chatAgent,
      model: own?.model ?? fallback?.id ?? "",
      effort: own ? own.effort : (fallback?.defaultEffort ?? null),
    });
  }, [workspace, chatAgent, update]);

  // Typing while a request waits answers it: the request is turned down and the text goes instead.
  const instead = useCallback(
    (text: string) => {
      if (pending) {
        const option = rejectOption(pending);
        if (option) answer(pending, option);
      }
      setComposer((previous) => ({
        ...previous,
        draft: "",
        queue: [...previous.queue, { text, handoffFrom: null }].slice(-3),
      }));
    },
    [pending, answer],
  );

  // The plan sits in the dock while the agent works through it; done, it joins its turn.
  const plan = model.plan;
  const planInDock = plan !== null && busy && !plan.done && plan.turnId === lastTurn?.id;

  // One polite line for assistive tech: the transcript's own words arrive too fast to announce.
  const agentName = getBrand(chatAgent).name;
  const announcement = empty
    ? ""
    : pending
      ? `${agentName} needs approval: ${pending.title}`
      : busy
        ? `${agentName} is working`
        : lastTurn?.status === "completed"
          ? `${agentName} finished`
          : lastTurn?.status === "failed"
            ? `${agentName} stopped with an error`
            : lastTurn?.status === "cancelled"
              ? `${agentName} stopped`
              : "";

  const composerView = (
    <Composer
      workspace={workspace}
      state={composer}
      update={update}
      orbState={orbState}
      busy={busy}
      onStop={stop}
      escStops={canStop}
      pending={pending ?? null}
      onInstead={instead}
      handoff={handoff}
      onKeepAgent={keepAgent}
      hero={empty}
    />
  );

  return (
    <PermissionAnswersProvider value={permissionAnswers}>
      <div
        className="noir"
        data-state={state}
        data-empty={empty ? "" : undefined}
        data-picker={composer.pickerOpen ? "" : undefined}
        data-capture={capture ? "" : undefined}
        style={brandCssVariables(brand) as CSSProperties}
      >
        <div className="n-backdrop" aria-hidden="true">
          <div className="n-grain" />
        </div>
        <Sidebar
          workspace={workspace}
          currentAgent={composer.agent}
          activeStatus={resumed ? "running" : null}
        />
        <main className="n-main">
          <div className="n-aura" aria-hidden="true">
            <AuraField brand={brand} intensity={aura} />
          </div>
          <header className="n-head">
            {empty ? (
              <h1 className="n-head-title" data-muted="">
                New chat
              </h1>
            ) : (
              <h1
                className="n-head-title"
                data-hidden={headline === false ? undefined : ""}
                data-settled={headSettled ? "" : undefined}
              >
                <span className="n-head-project">{project?.name}</span>
                <span className="n-head-sep" aria-hidden="true">
                  /
                </span>
                <span className="n-head-chat">{title}</span>
              </h1>
            )}
          </header>
          {empty ? (
            <NewChat
              agent={composer.agent}
              projects={workspace.projects}
              initialProject={workspace.activeProjectId}
              dockRef={dockRef}
            >
              {composerView}
            </NewChat>
          ) : (
            <>
              <Transcript
                model={model}
                workspace={workspace}
                player={player}
                autonomy={composer.autonomy}
                activity={activity}
                title={title}
                project={project?.name ?? null}
                onHeadline={setHeadline}
              />
              <div className="n-dock" ref={dockRef}>
                <AnimatePresence initial={false}>
                  {planInDock && plan ? <PlanCard key="plan" plan={plan} /> : null}
                </AnimatePresence>
                {composerView}
              </div>
            </>
          )}
          <p className="n-sr" role="status">
            {announcement}
          </p>
        </main>
      </div>
    </PermissionAnswersProvider>
  );
}

/**
 * A new chat: the orb, one serif line that names the project (a menu switches it), the composer,
 * and the three keys worth knowing.
 */
function NewChat({
  agent,
  projects,
  initialProject,
  dockRef,
  children,
}: {
  readonly agent: string;
  readonly projects: readonly Project[];
  readonly initialProject: string;
  readonly dockRef: (dock: HTMLDivElement | null) => void;
  readonly children: ReactNode;
}) {
  const [projectId, setProjectId] = useState(initialProject);
  // Where the project menu opens: under the project's name, in the new chat's own px.
  const [menuAt, setMenuAt] = useState<{ readonly x: number; readonly y: number } | null>(null);
  const project = projects.find((entry) => entry.id === projectId) ?? projects[0] ?? null;
  const rootRef = useRef<HTMLDivElement>(null);
  const nameRef = useRef<HTMLButtonElement>(null);
  const toggle = (): void => {
    const root = rootRef.current;
    const name = nameRef.current;
    if (menuAt || !root || !name) {
      setMenuAt(null);
      return;
    }
    const box = root.getBoundingClientRect();
    const scale = box.width / root.offsetWidth || 1;
    const rect = name.getBoundingClientRect();
    setMenuAt({
      x: ((rect.left + rect.right) / 2 - box.left) / scale,
      y: (rect.bottom - box.top) / scale,
    });
  };
  return (
    <div className="n-empty" ref={rootRef}>
      <div className="n-empty-orb">
        <Orb brand={agent} state="idle" size={56} />
      </div>
      <h2 className="n-empty-title">
        What should we build in{" "}
        <button
          ref={nameRef}
          type="button"
          className="n-empty-project"
          aria-haspopup="menu"
          aria-expanded={menuAt !== null}
          title="Change project"
          onClick={toggle}
        >
          <em>{project?.name ?? "this project"}</em>
          <ChevronDown
            className="n-empty-project-chev"
            size={12}
            strokeWidth={1.8}
            aria-hidden="true"
          />
        </button>
        ?
      </h2>
      <AnimatePresence>
        {menuAt ? (
          <ProjectMenu
            key="projects"
            at={menuAt}
            projects={projects}
            value={project?.id ?? null}
            onChoose={(id) => {
              setProjectId(id);
              setMenuAt(null);
              rootRef.current
                ?.querySelector<HTMLTextAreaElement>("[data-composer-field]")
                ?.focus({ preventScroll: true });
            }}
            onClose={() => setMenuAt(null)}
          />
        ) : null}
      </AnimatePresence>
      <div className="n-empty-composer" ref={dockRef}>
        {children}
      </div>
      <p className="n-empty-hint">
        <span className="n-empty-hint-item">
          <Kbd>/</Kbd> commands
        </span>
        <span className="n-empty-hint-item">
          <Kbd>@</Kbd> files
        </span>
        <span className="n-empty-hint-item">
          <Kbd>{"⇧⏎"}</Kbd> new line
        </span>
      </p>
    </div>
  );
}

/** The projects a new chat can start in. */
function ProjectMenu({
  at,
  projects,
  value,
  onChoose,
  onClose,
}: {
  readonly at: { readonly x: number; readonly y: number };
  readonly projects: readonly Project[];
  readonly value: string | null;
  readonly onChoose: (id: string) => void;
  readonly onClose: () => void;
}) {
  const reduced = useReduced();
  const ref = useRef<HTMLDivElement>(null);
  useDismiss(ref, onClose);
  // Opened, focus moves to the current project, so arrows and Enter work at once.
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
  }, []);
  const motionProps = popIn(reduced);
  const move = (event: ReactKeyboardEvent<HTMLDivElement>): void => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const items = [...(ref.current?.querySelectorAll<HTMLElement>("[role='menuitemradio']") ?? [])];
    const index = items.indexOf(document.activeElement as HTMLElement);
    const next =
      items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length];
    event.preventDefault();
    next?.focus();
  };
  return (
    <motion.div
      ref={ref}
      className="n-project-menu"
      role="menu"
      aria-label="Projects"
      style={{ left: at.x, top: at.y + 8 }}
      onKeyDown={move}
      initial={motionProps.initial}
      animate={motionProps.animate}
      exit={motionProps.exit}
      transition={motionProps.transition}
    >
      <GlassPanel elevation={3} blur squircle radius={14} className="n-overlay-panel n-menu-panel">
        {projects.map((entry) => (
          <button
            key={entry.id}
            type="button"
            role="menuitemradio"
            aria-checked={entry.id === value}
            className="n-menu-item n-project-option"
            onClick={() => onChoose(entry.id)}
          >
            <span className="n-menu-label">{entry.name}</span>
            {entry.id === value ? (
              <Check className="n-menu-check" size={14} strokeWidth={1.8} aria-hidden="true" />
            ) : null}
          </button>
        ))}
      </GlassPanel>
    </motion.div>
  );
}
