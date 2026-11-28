import "./game.css";
import { AgentMark } from "@ghost/ui/graphite";
import {
  AppWindow,
  Camera,
  Gamepad2,
  Pause,
  Play,
  RotateCw,
  Square,
  StepForward,
  TriangleAlert,
  Volume2,
  VolumeX,
  X,
} from "lucide-react";
import { useCallback, useContext, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { ChatGame, GameInput, GameLog, GodotStatus } from "../../bindings";
import { FileLinkContext } from "../../lib/chat/fileLinks";
import { errorMessage } from "../../lib/errors";
import {
  engineName,
  type GameApi,
  type GameStore,
  KeyboardSender,
  megabytes,
  mouseButton,
  SPEEDS,
} from "../../lib/game";
import { ScreenPlayer, type ScreenState, videoPoint } from "../../lib/screen";
import { commandKey } from "../chat/Keys";

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
}: {
  readonly chatId: string;
  readonly store: GameStore;
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
  const name = game?.project?.name ?? null;
  const title = name ?? "Game";

  return (
    <aside
      className="game-panel"
      aria-label="Game"
      data-testid="game-panel"
      data-full={narrow ? "" : undefined}
      data-state={game?.state ?? "idle"}
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
          {game && errors > 0 ? (
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
      <div className="game-stage">
        {status === null || game === null ? null : (
          <Stage
            chatId={chatId}
            api={api}
            game={game}
            godot={status.godot}
            agentName={agentName}
            toolsPossible={toolsPossible}
            onPlay={play}
            onGiveTools={giveTools}
            onOpenBrowser={onOpenBrowser}
            onError={fail}
          />
        )}
        {logOpen && game ? (
          <LogSheet chatId={chatId} api={api} game={game} onClose={() => setLogOpen(false)} />
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
      {running && game ? (
        <Controls
          chatId={chatId}
          api={api}
          game={game}
          onRestart={() => play(game.scene === "" ? null : game.scene)}
          onNotice={setNotice}
          onError={fail}
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
  onOpenBrowser,
  onError,
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly game: ChatGame;
  readonly godot: GodotStatus;
  readonly agentName: string;
  readonly toolsPossible: boolean;
  readonly onPlay: (scene?: string | null) => void;
  readonly onGiveTools: () => void;
  readonly onOpenBrowser?: () => void;
  readonly onError: (error: unknown) => void;
}) {
  const project = game.project;
  if (game.state === "running") return <Screen chatId={chatId} api={api} game={game} />;
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
        {game.tools ? (
          <p>
            Game is on for this chat. Ask {agentName} to start a game, a Godot project for example,
            and it shows up here to play.
          </p>
        ) : (
          <>
            <p>
              This chat's folder holds no game Ghost knows. Ghost runs Godot projects in this pane,
              and recognises web games, Unity, Unreal and Roblox projects.
            </p>
            {toolsPossible ? (
              <>
                <p className="game-note">
                  Starting one from nothing? Turn Game on here and {agentName} gets the tools to
                  run, see and play what it builds. Its session starts again to take them.
                </p>
                <button
                  type="button"
                  className="g-button g-button-primary g-button-28"
                  data-testid="game-turn-on"
                  onClick={onGiveTools}
                >
                  Turn on Game for this chat
                </button>
              </>
            ) : null}
          </>
        )}
      </div>
    );
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
              A web game is a page, so it runs in Ghost's browser beside the chat: {agentName}{" "}
              starts its dev server and opens it there, and you play it in the same place.
            </p>
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
          </>
        ) : project.engine === "unity" ||
          project.engine === "unreal" ||
          project.engine === "roblox" ? (
          <p>
            Ghost does not run {engineName(project.engine)} itself. Its editor runs the game, and
            its maker's own agent tools let {agentName} work in it. The game shows in the editor's
            window, not in this pane.
          </p>
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
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly game: ChatGame;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
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

  return (
    <div className="game-screen" data-state={state.kind} data-frozen={game.frozen ? "" : undefined}>
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
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false);
          keys.releaseAll();
          releaseButtons();
        }}
        onPointerDown={(event) => {
          const button = mouseButton(event.button);
          const at = point(event);
          if (!button || !at) return;
          event.currentTarget.focus();
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
          if (!at) return;
          // One notch of a wheel is about 100 pixels.
          const notches = Math.max(-8, Math.min(8, event.deltaY / 100));
          send([{ t: "wheel", x: at.x, y: at.y, dy: notches }]);
        }}
        onKeyDown={(event) => {
          if (keys.down(event.nativeEvent)) event.preventDefault();
        }}
        onKeyUp={(event) => {
          if (keys.up(event.nativeEvent)) event.preventDefault();
        }}
      />
      {state.kind === "live" && !focused && !game.frozen ? (
        <span className="game-hint" data-testid="game-hint">
          Click to play
        </span>
      ) : null}
      {state.kind === "live" && game.frozen ? (
        <span className="game-hint" data-testid="game-frozen">
          {game.agent ? "Frozen while the agent plays" : "Paused"}
        </span>
      ) : null}
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
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly game: ChatGame;
  readonly onRestart: () => void;
  readonly onNotice: (text: string) => void;
  readonly onError: (error: unknown) => void;
}) {
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
      <output className="game-meter-text" data-testid="game-fps" aria-label="Frames a second">
        {game.frozen ? "Frozen" : `${fps} fps`}
        {game.width > 0 ? ` · ${game.width}×${game.height}` : ""}
      </output>
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
