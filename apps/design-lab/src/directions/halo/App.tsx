import "./halo.css";
import {
  AuraField,
  type AuraIntensity,
  brandCssVariables,
  getBrand,
  type OrbState,
  usePrefersReducedMotion,
} from "@ghost/ui";
import { LucideProvider } from "lucide-react";
import { motion } from "motion/react";
import {
  type CSSProperties,
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  type DirectionProps,
  findChat,
  findProject,
  formatDuration,
  type PermissionModel,
  plural,
  type TranscriptModel,
} from "../../kit";
import { optionForShortcut } from "./blocks";
import {
  changesOf,
  EmptyHero,
  Header,
  type QueuedMessage,
  type Resumed,
  StatusSlab,
} from "./chrome";
import { Composer, type Selection } from "./composer";
import { type EnterAction, type EscapeAction, enterAction, escapeAction, focusKind } from "./keys";
import { EntranceProvider, ParallaxProvider, useDepth, useParallaxSource } from "./primitives";
import { Sidebar } from "./sidebar";
import { type Halt, type PermissionInteraction, Transcript } from "./transcript";

/**
 * The far plane: the agent's light field, drifting against the pointer, and a wash of the agent's
 * light low behind the composer that rises while the agent works (amber while it waits on you), so
 * the state reads even over a bright, busy wallpaper.
 */
function Backplane({
  agent,
  intensity,
  activity,
}: {
  readonly agent: string;
  readonly intensity: AuraIntensity;
  readonly activity: OrbState;
}) {
  const { x, y } = useDepth(-14);
  return (
    <motion.div className="halo-backplane" style={{ x, y }} aria-hidden="true">
      <AuraField brand={agent} intensity={intensity} />
      <div className="halo-backplane-wash" data-activity={activity} />
      <div className="halo-backplane-sheen" />
    </motion.div>
  );
}

/** The status slab and the composer, floating together over the bottom of the transcript. */
function Dock({
  children,
  onHeight,
}: {
  readonly children: ReactNode;
  readonly onHeight: (height: number) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const { x, y } = useDepth(5);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    onHeight(element.offsetHeight);
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver(() => onHeight(element.offsetHeight));
    observer.observe(element);
    return () => observer.disconnect();
  }, [onHeight]);
  return (
    // Holds the parallax while the pointer is over it, so what you aim at stays put.
    <motion.div ref={ref} className="halo-dock-stack" data-parallax-hold="" style={{ x, y }}>
      {children}
    </motion.div>
  );
}

const WORKING: ReadonlySet<OrbState> = new Set(["thinking", "streaming", "tool", "waiting"]);

/** Permission answers given in the lab, and which answer was just pressed (for its pressed look). */
function usePermissionAnswers(): PermissionInteraction {
  const [answers, setAnswers] = useState<Readonly<Record<string, string>>>({});
  const [pressed, setPressed] = useState<string | null>(null);
  const answer = useCallback((requestId: string, optionId: string) => {
    setPressed(optionId);
    window.setTimeout(() => {
      setPressed(null);
      setAnswers((previous) => ({ ...previous, [requestId]: optionId }));
    }, 140);
  }, []);
  return useMemo(() => ({ answers, pressed, answer }), [answers, pressed, answer]);
}

/**
 * Screen reader announcements for what the agent does: polite for the turn starting and ending,
 * assertive for a permission request, the one moment that blocks the agent until you answer.
 * Opening a chat announces nothing.
 */
function Announcer({ model, halt }: { readonly model: TranscriptModel; readonly halt: Halt }) {
  const turn = model.turns[model.turns.length - 1] ?? null;
  const pending = model.pendingPermissions[0] ?? null;
  const [polite, setPolite] = useState("");
  const [assertive, setAssertive] = useState("");
  const seen = useRef<{
    readonly turn: string | null;
    readonly status: string | null;
    readonly permission: string | null;
  } | null>(null);

  useEffect(() => {
    const now = {
      turn: turn?.id ?? null,
      status: turn?.status ?? null,
      permission: pending?.requestId ?? null,
    };
    const before = seen.current;
    seen.current = now;
    if (before === null) return;
    const name = getBrand(turn?.agent ?? "generic").name;
    if (pending && now.permission !== before.permission) {
      setAssertive(`${name} needs approval: ${pending.title} Enter allows once, Escape rejects.`);
    }
    if (!turn || (now.turn === before.turn && now.status === before.status)) return;
    if (turn.status === "running" && (before.status !== "waiting" || now.turn !== before.turn)) {
      setPolite(`${name} is working`);
    } else if (turn.status === "completed" && turn.footer) {
      const files = turn.footer.files.length;
      setPolite(
        `${name} finished in ${formatDuration(turn.footer.durationMs)}${files > 0 ? `, ${plural(files, "file")} changed` : ""}`,
      );
    } else if (turn.status === "failed") {
      setPolite("Stopped with an error");
    } else if (turn.status === "cancelled") {
      setPolite("Stopped");
    }
  }, [turn, pending]);

  useEffect(() => {
    if (halt === "stopped") setPolite("Stopped");
  }, [halt]);

  return (
    <>
      <div className="halo-sr" role="status" aria-live="polite" aria-atomic="true">
        {polite}
      </div>
      <div className="halo-sr" aria-live="assertive" aria-atomic="true">
        {assertive}
      </div>
    </>
  );
}

/**
 * Halo: the window as a set of floating glass slabs at different depths. The sidebar and the turn
 * cards rest on the window's glass, the status slab and composer float nearer, menus nearest; the
 * agent's light field lies far behind, drifting against the pointer while near planes drift with
 * it (and hold still while you work in the dock).
 */
export function App({ state, model, workspace, player }: DirectionProps) {
  const reduced = usePrefersReducedMotion();
  const parallax = useParallaxSource(!reduced);
  const [selection, setSelection] = useState<Selection>(() => ({
    agent: workspace.composer.agent,
    model: workspace.composer.model,
    effort: workspace.composer.effort,
    autonomy: workspace.composer.autonomy,
  }));
  const [pickerOpen, setPickerOpen] = useState(workspace.composer.pickerOpen);
  const [menuOpen, setMenuOpen] = useState(false);
  const permissions = usePermissionAnswers();
  const mainRef = useRef<HTMLElement>(null);

  const chat = findChat(workspace, workspace.activeChatId);
  const project = findProject(workspace, workspace.activeProjectId);
  const brand = getBrand(selection.agent);
  const empty = model.turns.length === 0;
  const changes = changesOf(model);
  const lastTurn = model.turns[model.turns.length - 1] ?? null;
  const turnRunning =
    lastTurn !== null && (lastTurn.status === "running" || lastTurn.status === "waiting");

  /*
   * Stop, in the lab: the recording pauses where it is and the turn reads as stopped. Escape (or
   * Stop) asks the agent to stop, and it does a moment later; a second press forces it. Playing
   * the recording again takes the stop back.
   */
  const [stopped, setStopped] = useState<{
    readonly turnId: string;
    readonly phase: "stopping" | "stopped";
  } | null>(null);
  const halt: Halt =
    stopped !== null && turnRunning && lastTurn?.id === stopped.turnId ? stopped.phase : null;
  const stop = useCallback(
    (force: boolean) => {
      if (!lastTurn) return;
      player.pause();
      setStopped({ turnId: lastTurn.id, phase: force ? "stopped" : "stopping" });
    },
    [player, lastTurn],
  );
  useEffect(() => {
    if (stopped?.phase !== "stopping") return;
    const timer = window.setTimeout(
      () => setStopped((current) => (current ? { ...current, phase: "stopped" } : current)),
      1200,
    );
    return () => window.clearTimeout(timer);
  }, [stopped]);
  useEffect(() => {
    if (player.playing) setStopped(null);
  }, [player.playing]);

  const pending: PermissionModel | null =
    halt === null
      ? (model.pendingPermissions.find(
          (permission) => permissions.answers[permission.requestId] === undefined,
        ) ?? null)
      : null;
  // A request answered here, before the recording moves on: the agent reads as carrying on.
  const answered =
    halt === null && pending === null
      ? (model.pendingPermissions.find(
          (permission) => permissions.answers[permission.requestId] !== undefined,
        ) ?? null)
      : null;
  const resumed: Resumed | null = answered
    ? {
        permission: answered,
        allowed: (
          answered.options.find(
            (option) => option.option_id === permissions.answers[answered.requestId],
          )?.kind ?? ""
        ).startsWith("allow"),
      }
    : null;
  const working = WORKING.has(model.activity) && halt !== "stopped";
  const activity: OrbState =
    halt === "stopped"
      ? "idle"
      : resumed && model.activity === "waiting"
        ? resumed.allowed
          ? "tool"
          : "thinking"
        : model.activity;
  const aura: AuraIntensity =
    halt === "stopped" ? "calm" : resumed && model.aura === "alert" ? "active" : model.aura;
  const turnAgent = lastTurn?.agent ?? null;
  const handoff =
    turnAgent !== null && !working && selection.agent !== turnAgent
      ? { from: turnAgent, to: selection.agent }
      : null;

  // Messages typed while the agent works wait here; when the turn ends they go out (and clear).
  const [queued, setQueued] = useState<readonly QueuedMessage[]>([]);
  const queueCount = useRef(0);
  const [wasWorking, setWasWorking] = useState(working);
  if (wasWorking !== working) {
    setWasWorking(working);
    if (!working) setQueued([]);
  }
  const queue = useCallback((text: string) => {
    queueCount.current += 1;
    const id = `queued-${queueCount.current}`;
    setQueued((list) => [...list, { id, text }]);
  }, []);
  const unqueue = useCallback((id: string) => {
    setQueued((list) => list.filter((message) => message.id !== id));
  }, []);

  const answerPending = useCallback(
    (kinds: Parameters<typeof optionForShortcut>[1]) => {
      if (!pending) return;
      const optionId = optionForShortcut(pending, kinds);
      if (optionId) permissions.answer(pending.requestId, optionId);
    },
    [pending, permissions],
  );

  /*
   * The window's keys, in one place (see keys.ts): Escape closes the topmost layer first, and
   * Enter answers a pending permission only where Enter means nothing else. The listener reads
   * the latest state through a ref, so it is added once.
   */
  const keys = useRef<{
    readonly context: {
      readonly menuOpen: boolean;
      readonly pickerOpen: boolean;
      readonly permissionPending: boolean;
      readonly working: boolean;
      readonly stopping: boolean;
    };
    readonly escape: (action: EscapeAction) => void;
    readonly enter: (action: EnterAction) => void;
  } | null>(null);
  useLayoutEffect(() => {
    keys.current = {
      context: {
        menuOpen,
        pickerOpen,
        permissionPending: pending !== null,
        working,
        stopping: halt === "stopping",
      },
      escape: (action) => {
        switch (action) {
          case "close-menu":
            setMenuOpen(false);
            return;
          case "close-picker":
            setPickerOpen(false);
            return;
          case "reject":
            answerPending(["reject_once", "reject_always"]);
            return;
          case "stop":
            stop(false);
            return;
          case "force-stop":
            stop(true);
            return;
        }
      },
      enter: (action) =>
        answerPending(action === "allow-always" ? ["allow_always"] : ["allow_once"]),
    };
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent): void => {
      const current = keys.current;
      if (!current || event.defaultPrevented || event.isComposing) return;
      const context = { ...current.context, focus: focusKind(event.target) };
      if (event.key === "Escape") {
        const action = escapeAction(context);
        if (action === null) return;
        event.preventDefault();
        current.escape(action);
      } else if (event.key === "Enter") {
        const action = enterAction(context, {
          meta: event.metaKey || event.ctrlKey,
          onButton: event.target instanceof HTMLButtonElement,
        });
        if (action === null) return;
        event.preventDefault();
        current.enter(action);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onDockHeight = useCallback((height: number) => {
    mainRef.current?.style.setProperty("--halo-dock-h", `${Math.ceil(height)}px`);
  }, []);

  const composer = (
    <Composer
      workspace={workspace}
      selection={selection}
      onSelect={setSelection}
      activity={empty ? "idle" : activity}
      working={working}
      waitingFor={pending ? getBrand(turnAgent ?? "generic").name : null}
      pickerOpen={pickerOpen}
      onPickerOpen={setPickerOpen}
      menuOpen={menuOpen}
      onMenuOpen={setMenuOpen}
      hero={empty}
      onStop={() => stop(halt === "stopping")}
      onQueue={queue}
      onReplace={() => answerPending(["reject_once", "reject_always"])}
    />
  );

  return (
    <LucideProvider strokeWidth={1.75}>
      <ParallaxProvider value={parallax.values}>
        <EntranceProvider>
          <div
            className="halo"
            data-state={state}
            data-activity={activity}
            style={brandCssVariables(brand) as CSSProperties}
            onPointerMove={parallax.onPointerMove}
            onPointerLeave={parallax.onPointerLeave}
          >
            <div className="halo-guard" aria-hidden="true" />
            <Backplane
              agent={brand.id}
              intensity={empty ? "calm" : aura}
              activity={empty ? "idle" : activity}
            />
            <Sidebar workspace={workspace} newChat={empty} activeAgent={selection.agent} />
            <main ref={mainRef} className="halo-main" data-picker={pickerOpen ? "" : undefined}>
              <Header
                project={project}
                title={chat?.title ?? model.session.title ?? "New chat"}
                changes={changes}
              />
              {empty ? (
                <EmptyHero agent={brand.id} project={project} composer={composer} />
              ) : (
                <Transcript
                  model={model}
                  activity={activity}
                  workspace={workspace}
                  player={player}
                  permissions={permissions}
                  halt={halt}
                  footer={
                    <Dock onHeight={onDockHeight}>
                      <StatusSlab
                        model={model}
                        activity={activity}
                        resumed={resumed}
                        player={player}
                        halt={halt}
                        handoff={handoff}
                        queued={queued}
                        onUnqueue={unqueue}
                      />
                      {composer}
                    </Dock>
                  }
                />
              )}
              <div className="halo-scrim" aria-hidden="true" />
            </main>
            <Announcer model={model} halt={halt} />
          </div>
        </EntranceProvider>
      </ParallaxProvider>
    </LucideProvider>
  );
}
