import "./game.css";
// The note beside what was pointed at is the browser's own, and wears its styles.
import "../browser/browser.css";
import { AgentMark } from "@ghost/ui/graphite";
import {
  AppWindow,
  Camera,
  Crosshair,
  Gamepad2,
  History,
  Joystick,
  Pause,
  Play,
  RotateCw,
  SlidersHorizontal,
  Square,
  StepForward,
  TriangleAlert,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import {
  type CSSProperties,
  type RefObject,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type {
  ChatGame,
  GameExport,
  GameInput,
  GameLog,
  GamePicked,
  GameRecent,
  GameTunables,
  GodotStatus,
  Starter,
} from "../../bindings";
import { FileLinkContext } from "../../lib/chat/fileLinks";
import { errorMessage } from "../../lib/errors";
import {
  engineName,
  type GameApi,
  type GameStore,
  KeyboardSender,
  megabytes,
  mouseButton,
  PAD_REST,
  type PadState,
  padEvents,
  padRelease,
  SPEEDS,
  STARTERS,
} from "../../lib/game";
import {
  areaOf,
  keepText,
  num,
  pickedLabel,
  pointText,
  recentText,
  type turned,
} from "../../lib/gameTell";
import { ScreenPlayer, type ScreenState, videoPoint } from "../../lib/screen";
import { AnnotationComposer } from "../browser/AnnotationComposer";
import type { AnnotationTarget } from "../browser/BrowserPanel";
import { commandKey } from "../chat/Keys";
import { AssetsView } from "./AssetsView";
import { connected, EngineConnect } from "./EngineConnect";
import { ExportSheet } from "./ExportSheet";
import { markedPicture, pngFrom, TuneSheet, themeColor } from "./GameTell";

/** A box in the stage's own pixels: where a note is put beside. */
interface Anchor {
  readonly x: number;
  readonly y: number;
  readonly width: number;
  readonly height: number;
}

/** An element's size as it is laid out, kept up as it changes. */
function useSize(element: RefObject<HTMLElement | null>): { width: number; height: number } {
  const [size, setSize] = useState({ width: 0, height: 0 });
  useLayoutEffect(() => {
    const node = element.current;
    if (!node) return;
    const measure = () => {
      const box = node.getBoundingClientRect();
      setSize((now) =>
        now.width === box.width && now.height === box.height
          ? now
          : { width: box.width, height: box.height },
      );
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const watching = new ResizeObserver(measure);
    watching.observe(node);
    return () => watching.disconnect();
  }, [element]);
  return size;
}

/** How many controllers the window sees. One shows up at its first press. */
function useControllers(): number {
  const [count, setCount] = useState(0);
  useEffect(() => {
    const look = () => {
      const pads = typeof navigator.getGamepads === "function" ? navigator.getGamepads() : [];
      setCount([...pads].filter((pad) => pad?.connected).length);
    };
    look();
    window.addEventListener("gamepadconnected", look);
    window.addEventListener("gamepaddisconnected", look);
    return () => {
      window.removeEventListener("gamepadconnected", look);
      window.removeEventListener("gamepaddisconnected", look);
    };
  }, []);
  return count;
}

/** How long what the agent did stays in the header after its call, so a run of calls reads as one. */
export const AGENT_LINGER_MS = 1200;

/** What the agent is doing, held for `ms` after it stops, and the last thing it did. */
function useLingering(value: string | null, ms: number): { shown: boolean; last: string | null } {
  const [held, setHeld] = useState(value !== null);
  const last = useRef<string | null>(value);
  if (value !== null) last.current = value;
  useEffect(() => {
    if (value !== null) {
      setHeld(true);
      return;
    }
    const timer = window.setTimeout(() => setHeld(false), ms);
    return () => window.clearTimeout(timer);
  }, [value, ms]);
  return { shown: value !== null || held, last: last.current };
}

/**
 * The game beside the chat (docs/game.md): the chat's own game, run by Ghost out of sight and
 * shown here as it plays. The person plays it with the keyboard and the mouse; the chat's agent
 * plays the same game through its tools, in frozen time, and the header says what it is doing.
 * Getting Godot, turning Game on for a chat, a window of the game's own and Godot's editor are
 * the person's, here and nowhere else.
 */
export function GamePanel({
  chatId,
  store,
  narrow,
  onClose,
  agentId,
  agentName,
  toolsPossible,
  onOpenBrowser,
  annotation,
}: {
  readonly chatId: string;
  readonly store: GameStore;
  /** How the pane hands a note to the chat: something pointed at, the last seconds of play. */
  readonly annotation?: AnnotationTarget;
  /** The window is too narrow for two panes: the pane covers the chat's work area. */
  readonly narrow: boolean;
  readonly onClose: () => void;
  readonly agentId: string;
  readonly agentName: string;
  /** This chat's agent can be given Ghost's tools at all (a Chat session on this computer). */
  readonly toolsPossible: boolean;
  /** Opens the browser beside the chat, where a web game runs. */
  readonly onOpenBrowser?: () => void;
}) {
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  const status = store.status();
  const game = store.of(chatId);
  const api = store.api;
  const [problem, setProblem] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [logOpen, setLogOpen] = useState(false);
  /** What the pane shows: the game, or the folder's pictures, sounds and models. */
  const [view, setView] = useState<"game" | "assets">("game");
  const wantedAsset = store.wantedAsset(chatId);
  // The chat asked for an asset: the pane turns to them.
  useEffect(() => {
    if (wantedAsset !== null) setView("assets");
  }, [wantedAsset]);
  const assetShown = useCallback(() => store.shownAsset(chatId), [store, chatId]);
  /** An export under way, and what the last one made. */
  const [exporting, setExporting] = useState(false);
  const [exported, setExported] = useState<GameExport | null>(null);
  const exportWeb = () => {
    setProblem(null);
    setExported(null);
    setExporting(true);
    api
      .exportWeb(chatId)
      .then(setExported, fail)
      .finally(() => setExporting(false));
  };
  /** What the pointer is for when it is not the game's: choosing something in the picture. */
  const [pointing, setPointing] = useState<"tell" | "tune" | null>(null);
  const [picked, setPicked] = useState<{ node: GamePicked; anchor: Anchor } | null>(null);
  const [recent, setRecent] = useState<GameRecent | null>(null);
  /** The node whose values the Tune sheet shows. */
  const [tuned, setTuned] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [noteError, setNoteError] = useState<string | null>(null);
  /** The game ran when pointing froze it, so it runs again when pointing ends. */
  const thaw = useRef(false);
  /** The picture as the pane shows it, for the image a note carries. */
  const shown = useRef<HTMLCanvasElement | null>(null);
  const stage = useRef<HTMLDivElement>(null);
  const stageSize = useSize(stage);
  const fail = useCallback((error: unknown) => setProblem(errorMessage(error)), []);
  const doing = useLingering(game?.agent ?? null, AGENT_LINGER_MS);
  // Opening the pane looks for Godot again, and at what the chat's folder holds now.
  useEffect(() => store.refresh(chatId), [store, chatId]);
  // A notice leaves by itself.
  useEffect(() => {
    if (notice === null) return;
    const timer = window.setTimeout(() => setNotice(null), 6000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const running = game?.state === "running";
  const errors = game?.errors ?? 0;
  // The log closes with the run it described.
  useEffect(() => {
    if (!running && errors === 0) setLogOpen(false);
  }, [running, errors]);

  const play = (scene: string | null = null) => {
    setProblem(null);
    api.run(chatId, scene).catch(fail);
  };
  const giveTools = () => {
    setProblem(null);
    api.turnOn(chatId, true).then((change) => {
      setNotice(
        change === "restarting"
          ? `${agentName} is starting again with the game tools.`
          : `${agentName} gets the game tools when its session next starts.`,
      );
      store.look(chatId);
    }, fail);
  };
  const startProject = (starter: Starter) => {
    setProblem(null);
    api.startProject(chatId, starter).then((change) => {
      if (toolsPossible) {
        setNotice(
          change === "restarting"
            ? `${agentName} is starting again with the game tools.`
            : `${agentName} gets the game tools when its session next starts.`,
        );
      }
      store.look(chatId);
    }, fail);
  };
  const name = game?.project?.name ?? null;
  const title = name ?? "Game";

  const stopPointing = useCallback(() => {
    setPointing(null);
    setPicked(null);
    setNoteError(null);
    if (thaw.current) {
      thaw.current = false;
      api.time(chatId, false, null).catch(fail);
    }
  }, [api, chatId, fail]);
  const startPointing = (purpose: "tell" | "tune") => {
    setProblem(null);
    setRecent(null);
    setLogOpen(false);
    if (purpose === "tell") setTuned(null);
    // What moves cannot be pointed at: the game stands still while the person chooses.
    if (pointing === null && game && !game.frozen) {
      thaw.current = true;
      api.time(chatId, true, null).catch(fail);
    }
    setPicked(null);
    setNoteError(null);
    setPointing(purpose);
  };
  const onPicked = (node: GamePicked | null, anchor: Anchor) => {
    if (!node) {
      setNotice("Nothing is drawn there. Click something in the picture.");
      return;
    }
    if (pointing === "tune") {
      setTuned(node.script_on || node.path);
      stopPointing();
      return;
    }
    setNoteError(null);
    setNotice(null);
    setPicked({ node, anchor });
  };
  const keepRecent = () => {
    setProblem(null);
    if (pointing) stopPointing();
    setLogOpen(false);
    setTuned(null);
    api.recent(chatId, 15).then((kept) => {
      setNoteError(null);
      setRecent(kept);
    }, fail);
  };
  const deliver = async (text: string, how: "send" | "add") => {
    if (!annotation || !game) return;
    setSending(true);
    setNoteError(null);
    try {
      const pictured = annotation.images !== false;
      let message: string;
      let image: Uint8Array | null = null;
      if (picked) {
        if (pictured && shown.current)
          image = await markedPicture(
            shown.current,
            areaOf(picked.node),
            game.width,
            themeColor("--g-volt", "rgb(59, 130, 246)"),
          );
        message = pointText(
          text,
          picked.node,
          { name, width: game.width, height: game.height },
          image !== null,
        );
      } else if (recent) {
        if (pictured) image = await pngFrom(recent.picture);
        message = recentText(text, recent, name, image !== null);
      } else return;
      const why = await (how === "send" ? annotation.send : annotation.add)(message, image);
      if (why) {
        setNoteError(why);
        return;
      }
      setRecent(null);
      stopPointing();
      setNotice(how === "send" ? `Sent to ${agentName}.` : "Added to the message box.");
    } catch (error) {
      setNoteError(errorMessage(error));
    } finally {
      setSending(false);
    }
  };
  const keepTuned = async (tunables: GameTunables, changes: ReturnType<typeof turned>) => {
    if (!annotation) return "This chat cannot take a message now.";
    const why = await annotation.send(keepText(tunables, changes, name), null);
    if (!why) setNotice(`Sent to ${agentName}.`);
    return why;
  };
  // What was being pointed at or tuned belongs to the run that showed it.
  useEffect(() => {
    if (running) return;
    thaw.current = false;
    setPointing(null);
    setPicked(null);
    setRecent(null);
    setTuned(null);
  }, [running]);
  const note = picked
    ? { subject: pickedLabel(picked.node), anchor: picked.anchor }
    : recent
      ? {
          subject: `The last ${Math.max(1, Math.round(num(recent.seconds)))} seconds of play`,
          // Over the foot of the picture, in the middle.
          anchor: {
            x: Math.max(8, stageSize.width / 2 - 190),
            y: Math.max(0, stageSize.height - 12),
            width: 0,
            height: 0,
          },
        }
      : null;

  return (
    <aside
      className="game-panel"
      aria-label="Game"
      data-testid="game-panel"
      data-full={narrow ? "" : undefined}
      data-state={game?.state ?? "idle"}
      data-pointing={pointing ?? undefined}
      onKeyDown={(event) => {
        // Esc gives the pointer back to the game. A note that is open takes its own Esc first.
        if (event.key === "Escape" && pointing && !event.defaultPrevented) {
          event.preventDefault();
          stopPointing();
        }
      }}
    >
      <div className="game-head">
        {/* The game's name, and in its place while the agent works, what it is doing: the
            header keeps its height, so the picture under it never moves. */}
        <span
          className="game-title"
          data-testid="game-title"
          data-agent={doing.shown ? "" : undefined}
        >
          <span className="game-title-name" aria-hidden={doing.shown ? true : undefined}>
            <Gamepad2 size={15} aria-hidden="true" />
            <span className="game-title-text">{title}</span>
          </span>
          <output
            className="game-title-agent"
            data-testid={doing.shown ? "game-agent" : undefined}
            aria-hidden={doing.shown ? undefined : true}
          >
            <span className="game-mark" aria-hidden="true">
              <AgentMark agent={agentId} state="tool" size={14} />
            </span>
            <span className="game-title-text">
              {doing.last ? `${agentName} · ${doing.last}` : ""}
            </span>
          </output>
        </span>
        <span className="game-head-end">
          {game?.project ? (
            <fieldset className="game-views">
              <legend className="sr-only">What the pane shows</legend>
              <button
                type="button"
                className="g-button g-button-ghost g-button-28"
                aria-pressed={view === "game"}
                data-testid="game-view-game"
                onClick={() => setView("game")}
              >
                Game
              </button>
              <button
                type="button"
                className="g-button g-button-ghost g-button-28"
                aria-pressed={view === "assets"}
                data-testid="game-view-assets"
                onClick={() => setView("assets")}
              >
                Assets
              </button>
            </fieldset>
          ) : null}
          {view === "assets" ? null : game && errors > 0 ? (
            <button
              type="button"
              className="g-button g-button-ghost g-button-28 game-errors"
              data-testid="game-errors"
              aria-expanded={logOpen}
              title={logOpen ? "Hide the game's log" : "Show the game's errors and what it printed"}
              onClick={() => setLogOpen((open) => !open)}
            >
              <TriangleAlert size={14} aria-hidden="true" />
              {errors === 1 ? "1 error" : `${errors} errors`}
            </button>
          ) : running ? (
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              data-testid="game-log-toggle"
              aria-expanded={logOpen}
              onClick={() => setLogOpen((open) => !open)}
            >
              Log
            </button>
          ) : null}
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Close the game"
            title={`Close the game (${commandKey()} Shift G)`}
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </span>
      </div>
      {game?.project && !game.tools && toolsPossible && game.project.engine === "godot" ? (
        <p className="game-bar" data-testid="game-tools-offer">
          <span className="game-bar-text">
            {agentName} started before this was a game, so it does not have the game tools.
          </span>
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            data-testid="game-give-tools"
            onClick={giveTools}
          >
            Give it the tools
          </button>
        </p>
      ) : null}
      {view === "assets" && game?.project ? (
        <AssetsView
          chatId={chatId}
          api={api}
          agentName={agentName}
          wanted={wantedAsset}
          onWantedShown={assetShown}
          onAdd={annotation ? (text) => annotation.add(text, null) : undefined}
        />
      ) : null}
      <div className="game-stage" ref={stage} hidden={view === "assets" && Boolean(game?.project)}>
        {status === null || game === null ? null : (
          <Stage
            chatId={chatId}
            api={api}
            game={game}
            pointing={pointing !== null && picked === null}
            marked={picked?.node ?? null}
            shown={shown}
            stage={stage}
            onPicked={onPicked}
            godot={status.godot}
            agentName={agentName}
            toolsPossible={toolsPossible}
            onPlay={play}
            onGiveTools={giveTools}
            onStartProject={startProject}
            onOpenBrowser={onOpenBrowser}
            onError={fail}
            exporting={exporting}
            onExport={exportWeb}
          />
        )}
        {exporting ? (
          <output className="game-bar game-exporting" data-testid="game-exporting">
            <span className="game-progress" aria-hidden="true" />
            <span className="game-bar-text">
              Packing the game for the web. A project's first export takes longer: Godot's web part
              is fetched once (10 MB) and the project is imported.
            </span>
          </output>
        ) : null}
        {exported && view === "game" ? (
          <ExportSheet
            exported={exported}
            onShow={() => api.exportShow(chatId).catch(fail)}
            onClose={() => setExported(null)}
          />
        ) : null}
        {logOpen && game ? (
          <LogSheet chatId={chatId} api={api} game={game} onClose={() => setLogOpen(false)} />
        ) : null}
        {tuned !== null && running && pointing === null && !logOpen ? (
          <TuneSheet
            chatId={chatId}
            api={api}
            node={tuned}
            agentName={agentName}
            canKeep={annotation !== undefined}
            onKeep={keepTuned}
            onPoint={() => startPointing("tune")}
            onClose={() => setTuned(null)}
          />
        ) : null}
        {note && annotation && running ? (
          <AnnotationComposer
            subject={note.subject}
            placeholder={picked ? "What is wrong with this?" : "What went wrong?"}
            noteOptional={picked === null}
            testId="game-note"
            anchor={note.anchor}
            page={stageSize}
            controls={annotation.controls}
            agentName={agentName}
            images={annotation.images}
            sending={sending}
            error={noteError}
            onSend={(text) => void deliver(text, "send")}
            onAdd={(text) => void deliver(text, "add")}
            onCancel={() => {
              setNoteError(null);
              // Back to choosing: the pointer still picks until Esc or the button.
              if (picked) setPicked(null);
              else setRecent(null);
            }}
          />
        ) : null}
        {/* Notices float over the foot of the stage, so the picture does not move for them. */}
        {problem || notice ? (
          <div className="game-notices">
            {problem ? (
              <p className="game-bar ui-error" role="alert" data-testid="game-problem">
                <span className="game-bar-text">{problem}</span>
                <button
                  type="button"
                  className="g-button g-button-ghost g-button-icon g-button-28"
                  aria-label="Dismiss"
                  onClick={() => setProblem(null)}
                >
                  <X size={14} />
                </button>
              </p>
            ) : null}
            {notice ? (
              <output className="game-bar" data-testid="game-notice">
                <span className="game-bar-text">{notice}</span>
              </output>
            ) : null}
          </div>
        ) : null}
      </div>
      {running && game && view === "game" ? (
        <Controls
          chatId={chatId}
          api={api}
          game={game}
          onRestart={() => play(game.scene === "" ? null : game.scene)}
          onNotice={setNotice}
          onError={fail}
          agentName={agentName}
          canTell={annotation !== undefined}
          pointing={pointing === "tell"}
          tuning={tuned !== null}
          onPoint={() => (pointing === "tell" ? stopPointing() : startPointing("tell"))}
          onRecent={keepRecent}
          onTune={() => {
            if (pointing) stopPointing();
            setLogOpen(false);
            setRecent(null);
            setTuned((now) => (now === null ? "." : null));
          }}
        />
      ) : null}
    </aside>
  );
}

/** What fills the pane: the game's picture, or the words for where things stand. */
function Stage({
  chatId,
  api,
  game,
  godot,
  agentName,
  toolsPossible,
  onPlay,
  onGiveTools,
  onStartProject,
  onOpenBrowser,
  onError,
  pointing,
  marked,
  shown,
  stage,
  onPicked,
  exporting,
  onExport,
}: {
  /** An export is under way. */
  readonly exporting: boolean;
  /** Packs the game for the web. */
  readonly onExport: () => void;
  readonly chatId: string;
  readonly api: GameApi;
  readonly game: ChatGame;
  readonly pointing: boolean;
  readonly marked: GamePicked | null;
  readonly shown: RefObject<HTMLCanvasElement | null>;
  readonly stage: RefObject<HTMLDivElement | null>;
  readonly onPicked: (node: GamePicked | null, anchor: Anchor) => void;
  readonly godot: GodotStatus;
  readonly agentName: string;
  readonly toolsPossible: boolean;
  readonly onPlay: (scene?: string | null) => void;
  readonly onGiveTools: () => void;
  readonly onStartProject: (starter: Starter) => void;
  readonly onOpenBrowser?: () => void;
  readonly onError: (error: unknown) => void;
}) {
  const project = game.project;
  if (game.state === "running")
    return (
      <Screen
        chatId={chatId}
        api={api}
        game={game}
        pointing={pointing}
        marked={marked}
        shown={shown}
        stage={stage}
        onPicked={onPicked}
      />
    );
  if (game.state === "starting")
    return (
      <div className="game-message" role="status" data-testid="game-starting">
        <span className="game-progress" aria-hidden="true" />
        <h2>Starting {project?.name ?? "the game"}</h2>
        <p>A project's first start takes longer: Godot imports its pictures and sounds once.</p>
      </div>
    );
  if (project === null)
    return (
      <div className="game-message" data-testid="game-none">
        <h2>No game here yet</h2>
        <p>
          {game.tools
            ? `Game is on for this chat. Ask ${agentName} to make a game, or start from one of these, and it shows up here to play.`
            : "This chat's folder holds no game Ghost knows. Start one here, and tell the agent what it should become."}
        </p>
        <ul className="game-list" aria-label="Start a game">
          {STARTERS.map((starter) => (
            <li key={starter.id} className="game-row" data-testid="game-starter">
              <span className="game-row-text">
                <span className="game-row-title">{starter.name}</span>
                <span className="game-row-what">{starter.what}</span>
              </span>
              <button
                type="button"
                className="g-button g-button-ghost g-button-28"
                aria-label={`Start a ${starter.name} game`}
                data-testid={`game-start-${starter.id}`}
                onClick={() => onStartProject(starter.id)}
              >
                Start
              </button>
            </li>
          ))}
        </ul>
        <p className="game-note">
          Ghost writes a few small files into this chat's folder and writes over nothing.
          {toolsPossible
            ? ` ${agentName} gets the tools to run, see and play the game; its session starts again to take them.`
            : ""}
        </p>
        {!game.tools && toolsPossible ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            data-testid="game-turn-on"
            title={`For a game ${agentName} makes from nothing: it gets the game tools, and its session starts again to take them`}
            onClick={onGiveTools}
          >
            Only turn on Game for this chat
          </button>
        ) : null}
      </div>
    );
  if (connected(project.engine))
    return <EngineConnect chatId={chatId} api={api} project={project} agentName={agentName} />;
  if (project.engine !== "godot")
    return (
      <div className="game-message" data-testid="game-other-engine">
        <h2>
          {project.name ?? "This project"} is made with {engineName(project.engine)}
          {project.framework ? ` (${project.framework})` : ""}
        </h2>
        {project.engine === "web" ? (
          <>
            <p>
              A web game is a page, so it runs in Ghost's browser beside the chat. {agentName}{" "}
              starts its dev server and opens it there, plays it with time frozen while it thinks,
              and you play it in the same place.
            </p>
            <div className="game-actions">
              {onOpenBrowser ? (
                <button
                  type="button"
                  className="g-button g-button-primary g-button-28"
                  data-testid="game-open-browser"
                  onClick={onOpenBrowser}
                >
                  Open the browser
                </button>
              ) : null}
              <button
                type="button"
                className="g-button g-button-ghost g-button-28"
                data-testid="game-export"
                title="Pack the built game into a zip that itch.io takes as it is"
                disabled={exporting}
                onClick={onExport}
              >
                {exporting ? "Packing" : "Pack for the web"}
              </button>
            </div>
          </>
        ) : (
          <p>
            Ghost has no pane for {engineName(project.engine)} games. {agentName} builds and runs it
            from its shell, and the game opens in a window of its own.
          </p>
        )}
      </div>
    );
  if (project.unsupported)
    return (
      <div className="game-message" data-testid="game-unsupported">
        <h2>Ghost cannot run {project.name ?? "this project"} yet</h2>
        <p>{project.unsupported}</p>
      </div>
    );
  if (!godot.looked)
    return (
      <div className="game-message" role="status" data-testid="game-looking">
        <span className="game-progress" aria-hidden="true" />
        <p>Looking for Godot.</p>
      </div>
    );
  if (game.godot_missing !== null || godot.fetch.state !== "idle")
    return <GetGodot api={api} godot={godot} why={game.godot_missing} onError={onError} />;
  return (
    <div
      className="game-message"
      data-testid={game.state === "ended" ? "game-ended" : "game-ready"}
    >
      <h2>{project.name ?? "Your game"}</h2>
      {game.state === "ended" && game.ended ? (
        <p className={game.errors > 0 ? "ui-error" : undefined} data-testid="game-ended-why">
          {game.ended}
        </p>
      ) : (
        <p>
          A Godot{project.version ? ` ${project.version}` : ""} project. It plays here, beside the
          chat: click the picture and use your keyboard and mouse.
        </p>
      )}
      {game.last_error && game.state === "ended" ? (
        <p className="game-note" data-testid="game-last-error">
          {game.last_error.text}
          {game.last_error.file ? ` (${game.last_error.file}:${game.last_error.line})` : ""}
        </p>
      ) : null}
      <div className="game-actions">
        <button
          type="button"
          className="g-button g-button-primary g-button-28"
          data-testid="game-play"
          onClick={() => onPlay(null)}
        >
          <Play size={14} aria-hidden="true" />
          {game.state === "ended" ? "Play again" : "Play"}
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          data-testid="game-open-editor"
          title="Open this project in Godot's own editor"
          onClick={() => api.openEditor(chatId).catch(onError)}
        >
          Open in Godot
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          data-testid="game-export"
          title="Export the game as a web page, in a zip that itch.io takes as it is"
          disabled={exporting}
          onClick={onExport}
        >
          {exporting ? "Exporting" : "Export for the web"}
        </button>
      </div>
    </div>
  );
}

/** Godot is not here, or not one that runs this project: what getting it means, and its progress. */
function GetGodot({
  api,
  godot,
  why,
  onError,
}: {
  readonly api: GameApi;
  readonly godot: GodotStatus;
  readonly why: string | null;
  readonly onError: (error: unknown) => void;
}) {
  const fetch = godot.fetch;
  if (fetch.state === "running" || fetch.state === "unpacking") {
    const part =
      fetch.state === "running" && fetch.total > 0 ? Math.min(1, fetch.received / fetch.total) : 0;
    return (
      <div className="game-message" role="status" data-testid="game-fetching">
        <h2>Getting Godot {godot.pinned}</h2>
        <progress
          className="game-meter"
          value={fetch.state === "unpacking" ? 1 : part}
          max={1}
          aria-label="Godot download progress"
        />
        <p data-testid="game-fetch-progress">
          {fetch.state === "unpacking"
            ? "Checking it against the hash Godot publishes, and unpacking."
            : `${megabytes(fetch.received)} of ${megabytes(fetch.total)}`}
        </p>
        {fetch.state === "running" ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            onClick={() => api.fetchCancel().catch(onError)}
          >
            Cancel
          </button>
        ) : null}
      </div>
    );
  }
  // A Godot is here but too old for this project: getting Ghost's pinned one may not help either,
  // so the reason is said as it is.
  const none = !godot.fetched && godot.found.length === 0;
  return (
    <div className="game-message" data-testid="game-get-godot">
      <h2>{none ? "This game needs Godot" : "This game needs another Godot"}</h2>
      {none ? (
        <p>
          Godot is the free, open engine this project is made with. Ghost can get its own copy:
          Godot {godot.pinned}
          {godot.size > 0 ? `, ${megabytes(godot.size)}` : ""}, from Godot's releases on GitHub,
          checked against the hash Godot publishes. It goes in Ghost's own folder and changes
          nothing else on this computer.
        </p>
      ) : (
        <p>{why}</p>
      )}
      {fetch.state === "failed" ? (
        <p className="ui-error" role="alert" data-testid="game-fetch-failed">
          {fetch.message}
        </p>
      ) : null}
      {godot.size > 0 && !godot.fetched ? (
        <button
          type="button"
          className="g-button g-button-primary g-button-28"
          data-testid="game-get"
          onClick={() => api.fetchStart().catch(onError)}
        >
          {fetch.state === "failed" ? "Try again" : `Get Godot ${godot.pinned}`}
        </button>
      ) : null}
      {godot.size === 0 ? (
        <p className="game-note">
          Godot publishes no build Ghost can fetch for this computer. Install Godot yourself and
          Ghost uses it.
        </p>
      ) : null}
    </div>
  );
}

/** The live game: its frames on a canvas, taking the pointer and the keyboard as the game would. */
function Screen({
  chatId,
  api,
  game,
  pointing,
  marked,
  shown,
  stage,
  onPicked,
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly game: ChatGame;
  /** The pointer chooses something in the picture instead of playing. */
  readonly pointing: boolean;
  /** What a note is being written about: it stays outlined. */
  readonly marked: GamePicked | null;
  /** Given the picture's canvas, for the image a note carries. */
  readonly shown: RefObject<HTMLCanvasElement | null>;
  readonly stage: RefObject<HTMLDivElement | null>;
  readonly onPicked: (node: GamePicked | null, anchor: Anchor) => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  /** What is under the pointer while it chooses. */
  const [hover, setHover] = useState<GamePicked | null>(null);
  const asking = useRef({
    seq: 0,
    at: 0,
    busy: false,
    next: null as { x: number; y: number } | null,
  });
  const pad = useRef<PadState>(PAD_REST);
  const player = useRef<ScreenPlayer | null>(null);
  const [state, setState] = useState<ScreenState>({ kind: "waiting" });
  const [focused, setFocused] = useState(false);
  const [keys] = useState(
    () => new KeyboardSender((events) => void api.input(chatId, events).catch(() => {})),
  );
  /** The latest pointer move, sent once a frame however fast the pointer moves. */
  const move = useRef<{ x: number; y: number } | null>(null);
  const moving = useRef(0);
  const pressed = useRef(new Set<string>());

  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const screen = new ScreenPlayer(element, setState);
    player.current = screen;
    let stop: (() => void) | null = null;
    let cancelled = false;
    api
      .watch(chatId, (packet) => screen.push(packet))
      .then(
        (stopWatching) => {
          if (cancelled) stopWatching();
          else stop = stopWatching;
        },
        () => {},
      );
    return () => {
      cancelled = true;
      stop?.();
      screen.close();
      player.current = null;
      // The pane is going: nothing stays held down in the game.
      keys.releaseAll();
      if (moving.current) cancelAnimationFrame(moving.current);
    };
  }, [chatId, api, keys]);

  const point = (event: { clientX: number; clientY: number }) => {
    const element = canvas.current;
    const picture = player.current?.size();
    if (!element || !picture) return null;
    const box = element.getBoundingClientRect();
    const x = Math.min(box.right - 1, Math.max(box.left, event.clientX));
    const y = Math.min(box.bottom - 1, Math.max(box.top, event.clientY));
    const at = videoPoint(x, y, box, picture);
    if (!at) return null;
    // The picture may be smaller than the game: the game hears its own pixels.
    const scale = game.width > 0 ? game.width / picture.width : 1;
    return { x: at.x * scale, y: at.y * scale };
  };
  const send = (events: GameInput[]) => void api.input(chatId, events).catch(() => {});
  const releaseButtons = () => {
    for (const button of pressed.current) send([{ t: "mouse", x: 0, y: 0, button, down: false }]);
    pressed.current.clear();
  };

  useEffect(() => {
    shown.current = canvas.current;
    return () => {
      shown.current = null;
    };
  }, [shown]);

  // While the pointer chooses, nothing of the person's stays held in the game.
  useEffect(() => {
    if (!pointing) {
      setHover(null);
      asking.current.seq += 1;
      asking.current.next = null;
      return;
    }
    keys.releaseAll();
    for (const button of pressed.current)
      void api.input(chatId, [{ t: "mouse", x: 0, y: 0, button, down: false }]).catch(() => {});
    pressed.current.clear();
  }, [pointing, keys, api, chatId]);

  /** Asks what is under a point, one question at a time, the newest place winning. */
  const ask = (at: { x: number; y: number }) => {
    const state = asking.current;
    state.next = at;
    if (state.busy) return;
    const run = () => {
      const place = state.next;
      if (!place) return;
      state.next = null;
      state.busy = true;
      const seq = state.seq;
      api
        .pick(chatId, place.x, place.y)
        .then(
          (found) => {
            if (seq === state.seq) setHover(found);
          },
          () => {},
        )
        .finally(() => {
          state.busy = false;
          run();
        });
    };
    run();
  };

  /** A box of the game's own pixels as the stage's, for the note beside it. */
  const anchorOf = (area: readonly number[]): Anchor => {
    const element = canvas.current;
    const around = stage.current;
    if (!element || !around || game.width <= 0) return { x: 8, y: 8, width: 0, height: 0 };
    const box = element.getBoundingClientRect();
    const outer = around.getBoundingClientRect();
    const scale = box.width / game.width;
    const [x = 0, y = 0, width = 0, height = 0] = area;
    return {
      x: box.left - outer.left + x * scale,
      y: box.top - outer.top + y * scale,
      width: width * scale,
      height: height * scale,
    };
  };

  // A controller plays the game while the picture has the keyboard. A game that reads the
  // computer's controllers itself is sent none: it would hear every press twice.
  const [focusedNow, setFocusedNow] = useState(false);
  const padOn = focusedNow && !pointing && game.joypads === 0;
  useEffect(() => {
    if (!padOn || typeof navigator.getGamepads !== "function") return;
    let frame = 0;
    const read = () => {
      const found = [...navigator.getGamepads()].find((entry) => entry?.connected);
      if (found) {
        const { events, state } = padEvents(pad.current, found);
        pad.current = state;
        if (events.length > 0) void api.input(chatId, events).catch(() => {});
      }
      frame = requestAnimationFrame(read);
    };
    frame = requestAnimationFrame(read);
    return () => {
      cancelAnimationFrame(frame);
      const release = padRelease(pad.current);
      pad.current = PAD_REST;
      if (release.length > 0) void api.input(chatId, release).catch(() => {});
    };
  }, [padOn, api, chatId]);

  /** A box of the game's pixels as a share of the picture, for the outline over it. */
  const outline = (node: GamePicked, kind: "hover" | "marked") => {
    if (game.width <= 0 || game.height <= 0) return null;
    const [x, y, width, height] = areaOf(node);
    return (
      <span
        className="game-pick"
        data-kind={kind}
        data-testid={kind === "hover" ? "game-pick-hover" : "game-pick-marked"}
        style={
          {
            left: `${(x / game.width) * 100}%`,
            top: `${(y / game.height) * 100}%`,
            width: `${(width / game.width) * 100}%`,
            height: `${(height / game.height) * 100}%`,
          } as CSSProperties
        }
      >
        <span className="game-pick-label">{pickedLabel(node)}</span>
      </span>
    );
  };

  // The frame has the picture's own shape (the game's, until the first frame says), as large as
  // the stage allows: what is said over the picture's foot is then on the picture.
  const shape =
    state.kind === "live"
      ? { width: state.width, height: state.height }
      : game.width > 0 && game.height > 0
        ? { width: game.width, height: game.height }
        : { width: 16, height: 9 };
  return (
    <div
      className="game-screen"
      data-state={state.kind}
      data-frozen={game.frozen ? "" : undefined}
      data-pointing={pointing ? "" : undefined}
    >
      <div
        className="game-frame"
        style={{ "--game-w": shape.width, "--game-h": shape.height } as CSSProperties}
      >
        {state.kind === "waiting" ? (
          <span className="game-progress game-progress-over" aria-hidden="true" />
        ) : null}
        {/* biome-ignore lint/a11y/noInteractiveElementToNoninteractiveRole: the picture is a game that takes the pointer and the keyboard as its own window would (role application) */}
        <canvas
          ref={canvas}
          className="game-canvas"
          role="application"
          aria-label={`The game. Click it and your keys and mouse go to the game. ${commandKey()} Shift G closes the pane and gives the keyboard back.`}
          aria-roledescription="game"
          tabIndex={0}
          data-testid="game-canvas"
          onContextMenu={(event) => event.preventDefault()}
          onFocus={() => {
            setFocused(true);
            setFocusedNow(true);
          }}
          onBlur={() => {
            setFocused(false);
            setFocusedNow(false);
            keys.releaseAll();
            releaseButtons();
          }}
          onPointerLeave={() => {
            if (!pointing) return;
            asking.current.seq += 1;
            asking.current.next = null;
            setHover(null);
          }}
          onPointerDown={(event) => {
            const button = mouseButton(event.button);
            const at = point(event);
            if (!button || !at) return;
            event.currentTarget.focus();
            if (pointing) {
              if (button !== "left") return;
              // The click asks for itself: what is outlined may be a moment old.
              asking.current.seq += 1;
              api.pick(chatId, at.x, at.y).then(
                (found) => {
                  setHover(null);
                  onPicked(found, anchorOf(found ? areaOf(found) : [at.x, at.y, 0, 0]));
                },
                () => onPicked(null, anchorOf([at.x, at.y, 0, 0])),
              );
              return;
            }
            event.currentTarget.setPointerCapture(event.pointerId);
            pressed.current.add(button);
            send([
              { t: "move", x: at.x, y: at.y },
              { t: "mouse", x: at.x, y: at.y, button, down: true },
            ]);
          }}
          onPointerMove={(event) => {
            const at = point(event);
            if (!at) return;
            if (pointing) {
              ask(at);
              return;
            }
            move.current = at;
            if (moving.current) return;
            moving.current = requestAnimationFrame(() => {
              moving.current = 0;
              const latest = move.current;
              move.current = null;
              if (latest) send([{ t: "move", x: latest.x, y: latest.y }]);
            });
          }}
          onPointerUp={(event) => {
            const button = mouseButton(event.button);
            const at = point(event);
            if (!button || !pressed.current.delete(button)) return;
            send([{ t: "mouse", x: at?.x ?? 0, y: at?.y ?? 0, button, down: false }]);
          }}
          onPointerCancel={releaseButtons}
          onWheel={(event) => {
            const at = point(event);
            if (!at || pointing) return;
            // One notch of a wheel is about 100 pixels.
            const notches = Math.max(-8, Math.min(8, event.deltaY / 100));
            send([{ t: "wheel", x: at.x, y: at.y, dy: notches }]);
          }}
          onKeyDown={(event) => {
            // While the pointer chooses, keys are the pane's (Esc ends it), not the game's.
            if (pointing) return;
            if (keys.down(event.nativeEvent)) event.preventDefault();
          }}
          onKeyUp={(event) => {
            if (keys.up(event.nativeEvent)) event.preventDefault();
          }}
        />
        {pointing && hover ? outline(hover, "hover") : null}
        {marked ? outline(marked, "marked") : null}
        {state.kind === "live" && pointing ? (
          <span className="game-hint" data-testid="game-pointing">
            Click what you mean. Esc to go back.
          </span>
        ) : null}
        {state.kind === "live" && !pointing && !marked && !focused && !game.frozen ? (
          <span className="game-hint" data-testid="game-hint">
            Click to play
          </span>
        ) : null}
        {state.kind === "live" && !pointing && !marked && game.frozen ? (
          <span className="game-hint" data-testid="game-frozen">
            {game.agent ? "Frozen while the agent plays" : "Paused"}
          </span>
        ) : null}
      </div>
    </div>
  );
}

/** Under a running game: time, sound, a picture, a window of its own, and stop. */
function Controls({
  chatId,
  api,
  game,
  onRestart,
  onNotice,
  onError,
  agentName,
  canTell,
  pointing,
  tuning,
  onPoint,
  onRecent,
  onTune,
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly game: ChatGame;
  readonly onRestart: () => void;
  readonly onNotice: (text: string) => void;
  readonly onError: (error: unknown) => void;
  readonly agentName: string;
  /** The chat can be handed a note. */
  readonly canTell: boolean;
  readonly pointing: boolean;
  readonly tuning: boolean;
  readonly onPoint: () => void;
  readonly onRecent: () => void;
  readonly onTune: () => void;
}) {
  const controllers = useControllers();
  const speed = game.speed ?? 1;
  const fps = Math.round(game.fps ?? 0);
  // The next speed up, and round to the slowest after the fastest.
  const nextSpeed = SPEEDS.find((option) => option > speed) ?? SPEEDS[0] ?? 1;
  return (
    <div className="game-controls" role="toolbar" aria-label="Game controls">
      <span className="game-controls-group">
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label={game.frozen ? "Resume" : "Pause"}
          title={game.frozen ? "Resume" : "Pause"}
          data-testid="game-pause"
          onClick={() => api.time(chatId, !game.frozen, null).catch(onError)}
        >
          {game.frozen ? <Play size={15} /> : <Pause size={15} />}
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Step one frame"
          title="Step one frame (while paused)"
          data-testid="game-step"
          disabled={!game.frozen}
          onClick={() => api.step(chatId).catch(onError)}
        >
          <StepForward size={15} />
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-28 game-speed"
          aria-label={`Speed ${speed}x. Press for ${nextSpeed}x.`}
          title={`The game runs at ${speed}x speed. Press for ${nextSpeed}x.`}
          data-testid="game-speed"
          onClick={() => api.time(chatId, null, nextSpeed).catch(onError)}
        >
          {speed}x
        </button>
      </span>
      <span className="game-controls-group">
        {canTell ? (
          <>
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28"
              aria-label={`Point at something for ${agentName}`}
              aria-pressed={pointing}
              title={
                pointing
                  ? "Stop pointing (Esc)"
                  : `Point at what is wrong: the game freezes, you click it, and ${agentName} gets the node, its scene and its script`
              }
              data-testid="game-point"
              onClick={onPoint}
            >
              <Crosshair size={15} />
            </button>
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28"
              aria-label={`Send the last 15 seconds to ${agentName}`}
              title={`Send the last 15 seconds: ${agentName} gets pictures of what you just played and can play the same run again`}
              data-testid="game-recent"
              onClick={onRecent}
            >
              <History size={15} />
            </button>
          </>
        ) : null}
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Tune values"
          aria-pressed={tuning}
          title="Tune values: turn the numbers a script exports while the game runs"
          data-testid="game-tune"
          onClick={onTune}
        >
          <SlidersHorizontal size={15} />
        </button>
      </span>
      <output className="game-meter-text" data-testid="game-fps" aria-label="Frames a second">
        {game.frozen ? "Frozen" : `${fps} fps`}
        {game.width > 0 ? ` · ${game.width}×${game.height}` : ""}
      </output>
      {controllers > 0 || game.joypads > 0 ? (
        <span
          className="game-pad"
          role="img"
          aria-label="A controller is connected"
          title={
            game.joypads > 0
              ? "The game reads your controller itself."
              : "Your controller plays the game while the picture has the keyboard: click the game first."
          }
          data-testid="game-pad"
        >
          <Joystick size={14} />
        </span>
      ) : null}
      <span className="game-controls-group">
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label={game.sound ? "Mute the game" : "Hear the game"}
          aria-pressed={game.sound}
          title={game.sound ? "Mute the game" : "Hear the game"}
          data-testid="game-sound"
          onClick={() => api.sound(chatId, !game.sound).catch(onError)}
        >
          {game.sound ? <Volume2 size={15} /> : <VolumeX size={15} />}
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Save a picture of the game"
          title="Save a picture of the game"
          data-testid="game-picture"
          onClick={() =>
            api.picture(chatId).then((where) => onNotice(`Saved to ${where}`), onError)
          }
        >
          <Camera size={15} />
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label={
            game.windowed ? "Put the game's window away" : "Give the game a window of its own"
          }
          aria-pressed={game.windowed}
          title={
            game.windowed
              ? "Put the game's window away. The game starts again."
              : "Give the game a window of its own, at full speed. The game starts again."
          }
          data-testid="game-window"
          onClick={() => api.window(chatId, !game.windowed).catch(onError)}
        >
          <AppWindow size={15} />
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Start the game again"
          title="Start the game again, with the files as they are now"
          data-testid="game-restart"
          onClick={onRestart}
        >
          <RotateCw size={15} />
        </button>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Stop the game"
          title="Stop the game"
          data-testid="game-stop"
          onClick={() => api.stop(chatId).catch(onError)}
        >
          <Square size={13} />
        </button>
      </span>
    </div>
  );
}

/** The run's errors, each at its file and line, and the last lines the game printed. */
function LogSheet({
  chatId,
  api,
  game,
  onClose,
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly game: ChatGame;
  readonly onClose: () => void;
}) {
  const links = useContext(FileLinkContext);
  const [log, setLog] = useState<GameLog | null>(null);
  // Read again whenever the run's error count or state changes: the status carries no lines.
  const stamp = `${game.state}:${game.errors}:${game.last_error?.count ?? 0}`;
  useEffect(() => {
    let current = true;
    void stamp;
    api.log(chatId).then(
      (next) => {
        if (current) setLog(next);
      },
      () => {},
    );
    return () => {
      current = false;
    };
  }, [api, chatId, stamp]);
  return (
    <section className="game-log" aria-label="The game's log" data-testid="game-log">
      <div className="game-log-head">
        <h2>Log</h2>
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Close the log"
          onClick={onClose}
        >
          <X size={14} />
        </button>
      </div>
      {log === null ? null : (
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a long log scrolls, and a keyboard reaches it to scroll
        <div className="game-log-body" tabIndex={0}>
          {log.errors.length === 0 ? (
            <p className="game-note">No errors or warnings in this run.</p>
          ) : (
            <ul className="game-log-errors">
              {log.errors.map((error) => {
                const file = error.file.startsWith("res://") ? error.file.slice(6) : "";
                const where = error.file ? `${error.file}:${error.line}` : "";
                return (
                  <li
                    key={`${error.kind}:${error.file}:${error.line}:${error.text}`}
                    data-kind={error.kind}
                  >
                    <span className="game-log-text">
                      {error.text}
                      {error.count > 1 ? ` (×${error.count})` : ""}
                    </span>
                    {where === "" ? null : links && file !== "" ? (
                      <button
                        type="button"
                        className="game-log-where"
                        title={`Open in ${links.editorName}`}
                        onClick={() => links.open(file, error.line > 0 ? error.line : null, null)}
                      >
                        {where}
                      </button>
                    ) : (
                      <span className="game-log-where">{where}</span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {log.lines.length > 0 ? (
            <pre className="game-log-lines" data-testid="game-log-lines">
              {log.dropped > 0 ? `… ${log.dropped} earlier lines are gone\n` : ""}
              {log.lines.join("\n")}
            </pre>
          ) : (
            <p className="game-note">The game has printed nothing.</p>
          )}
        </div>
      )}
    </section>
  );
}
