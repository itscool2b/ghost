import "./mobile.css";
import { AgentMark } from "@ghost/ui/graphite";
import {
  Camera,
  ChevronLeft,
  Circle,
  RotateCw,
  Smartphone,
  Square,
  TriangleAlert,
  X,
} from "lucide-react";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type {
  ChatDevice,
  DeviceInfo,
  DeviceKey,
  Fetch,
  MobileStatus,
  ToolFetch,
} from "../../bindings";
import { errorMessage } from "../../lib/errors";
import {
  deviceKey,
  type MobileApi,
  type MobileStore,
  megabytes,
  ScreenPlayer,
  type ScreenState,
  videoPoint,
} from "../../lib/mobile";

/** How long what the agent did stays in the header after its call, so a run of calls reads as one. */
export const AGENT_LINGER_MS = 1200;

/**
 * What the agent is doing, held for `ms` after it stops, and the last thing it did (shown while
 * the line fades). The pane changes no size for it (owner, 24 November 2026: the screen "jitters
 * up and down" when a bar came and went over it).
 */
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
 * The device beside the chat (docs/superpowers/plans/2026-11-21-mobile.md): an Android emulator of
 * the chat's own, an iPhone simulator of its own on a Mac, or a phone the person chose, with its
 * live screen. The person uses it by clicking and typing; the chat's agent uses the same device
 * through its tools, and the header says what it is doing. Setting Android up, accepting Google's
 * licence and handing a phone to the agent happen here and nowhere else.
 */
export function DevicePanel({
  chatId,
  store,
  narrow,
  onClose,
  agentId,
  agentName,
  summoned = false,
}: {
  readonly chatId: string;
  readonly store: MobileStore;
  /** The window is too narrow for two panes: the pane covers the chat's work area. */
  readonly narrow: boolean;
  readonly onClose: () => void;
  readonly agentId: string;
  readonly agentName: string;
  /** The agent opened the pane: with nothing set up, Google's offer shows at once. */
  readonly summoned?: boolean;
}) {
  useSyncExternalStore(store.subscribe, store.getVersion, store.getVersion);
  const status = store.status();
  const held = store.held(chatId);
  const [problem, setProblem] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const fail = useCallback((error: unknown) => setProblem(errorMessage(error)), []);
  const doing = useLingering(held?.agent ?? null, AGENT_LINGER_MS);
  // Opening the pane looks again at what is installed and starts watching for phones.
  useEffect(() => store.refresh(), [store]);
  // The agent asked for a phone and there is none to give: the person sees what setting Android
  // up downloads, with Google's licence, and accepting it stays theirs. On a Mac the choice of
  // an Android phone or an iPhone shows instead.
  const offered = useRef(false);
  const offerNow =
    summoned &&
    status !== null &&
    held === null &&
    status.unavailable === null &&
    !status.setup.emulator &&
    !status.setup.ios.possible &&
    status.setup.fetch.state === "idle";
  useEffect(() => {
    if (!offerNow || offered.current) return;
    offered.current = true;
    store.api.fetchOffer().catch(fail);
  }, [offerNow, store, fail]);
  // A notice about a saved picture leaves by itself.
  useEffect(() => {
    if (saved === null) return;
    const timer = window.setTimeout(() => setSaved(null), 6000);
    return () => window.clearTimeout(timer);
  }, [saved]);

  const api = store.api;
  const attach = (deviceId: string | null) => {
    setProblem(null);
    api.attach(chatId, deviceId).catch(fail);
  };
  const live = held?.live === true;
  const iphone = held?.kind === "simulator";
  const press = (key: DeviceKey) => api.key(chatId, key).catch(fail);

  return (
    <aside
      className="device-panel"
      aria-label="Device"
      data-testid="device-panel"
      data-full={narrow ? "" : undefined}
    >
      <div className="device-head">
        {/* The device's name, and in its place while the agent works, what it is doing: the
            header keeps its height, so the screen under it never moves. */}
        <span
          className="device-title"
          data-testid="device-title"
          data-agent={doing.shown ? "" : undefined}
        >
          <span className="device-title-name" aria-hidden={doing.shown ? true : undefined}>
            <Smartphone size={15} aria-hidden="true" />
            <span className="device-title-text">{held ? held.title : "Device"}</span>
          </span>
          <output
            className="device-title-agent"
            data-testid={doing.shown ? "device-agent" : undefined}
            aria-hidden={doing.shown ? undefined : true}
          >
            <span className="device-bar-mark" aria-hidden="true">
              <AgentMark agent={agentId} state="tool" size={14} />
            </span>
            <span className="device-title-text">
              {doing.last ? `${agentName} · ${doing.last}` : ""}
            </span>
          </output>
        </span>
        <span className="device-head-end">
          {live ? (
            <>
              {iphone ? null : (
                <button
                  type="button"
                  className="g-button g-button-ghost g-button-icon g-button-28"
                  aria-label="Turn the device"
                  title="Turn the device"
                  onClick={() => api.rotate(chatId).catch(fail)}
                >
                  <RotateCw size={15} />
                </button>
              )}
              <button
                type="button"
                className="g-button g-button-ghost g-button-icon g-button-28"
                aria-label="Save a picture of the screen"
                title="Save a picture of the screen"
                data-testid="device-screenshot"
                onClick={() => api.screenshot(chatId).then(setSaved, fail)}
              >
                <Camera size={15} />
              </button>
            </>
          ) : null}
          {held ? (
            <button
              type="button"
              className="g-button g-button-ghost g-button-28"
              data-testid="device-change"
              title={
                held.kind === "phone"
                  ? "Choose another device"
                  : "Choose another device. This one keeps running for a while."
              }
              onClick={() => {
                setProblem(null);
                api.detach(chatId).catch(fail);
              }}
            >
              Change
            </button>
          ) : null}
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Close the device"
            title="Close the device (Ctrl+Shift+D)"
            onClick={onClose}
          >
            <X size={16} />
          </button>
        </span>
      </div>
      {held?.kind === "phone" && live ? (
        <label className="device-bar device-consent" data-testid="device-consent">
          <span className="device-bar-text">
            Let the agent use this phone. It can then see and use everything on it.
          </span>
          <input
            type="checkbox"
            role="switch"
            className="g-switch"
            aria-checked={held.agent_allowed}
            checked={held.agent_allowed}
            onChange={(event) => api.allowAgent(chatId, event.currentTarget.checked).catch(fail)}
          />
        </label>
      ) : null}
      <div className="device-stage">
        {status === null ? null : held ? (
          <HeldDevice
            chatId={chatId}
            api={api}
            held={held}
            tool={status.setup.ios.tool}
            onError={fail}
            onRetry={() => attach(held.device_id)}
          />
        ) : (
          <Chooser status={status} api={api} onAttach={attach} onError={fail} />
        )}
        {/* Notices float over the foot of the stage, so the screen does not move for them. */}
        {problem || saved ? (
          <div className="device-notices">
            {problem ? (
              <p className="device-bar ui-error" role="alert" data-testid="device-problem">
                <span className="device-bar-text">{problem}</span>
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
            {saved ? (
              <output className="device-bar" data-testid="device-saved">
                <span className="device-bar-text">Saved to {saved}</span>
              </output>
            ) : null}
          </div>
        ) : null}
      </div>
      {live ? (
        <div className="device-keys" role="toolbar" aria-label="Device buttons">
          {/* An iPhone has no Back button: its apps go back with a swipe from the left edge. */}
          {iphone ? null : (
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28"
              aria-label="Back"
              title="Back"
              onClick={() => press("back")}
            >
              <ChevronLeft size={17} />
            </button>
          )}
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Home"
            title="Home"
            onClick={() => press("home")}
          >
            <Circle size={14} />
          </button>
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Recent apps"
            title="Recent apps"
            onClick={() => press("recents")}
          >
            <Square size={13} />
          </button>
        </div>
      ) : null}
    </aside>
  );
}

/** The chat's device: starting, live, or stopped with its reason. */
function HeldDevice({
  chatId,
  api,
  held,
  tool,
  onError,
  onRetry,
}: {
  readonly chatId: string;
  readonly api: MobileApi;
  readonly held: ChatDevice;
  /** AXe, which an iPhone's first start downloads. */
  readonly tool: ToolFetch;
  readonly onError: (error: unknown) => void;
  readonly onRetry: () => void;
}) {
  if (held.live) return <Screen chatId={chatId} api={api} onError={onError} />;
  if (held.starting)
    return (
      <div className="device-message" role="status" data-testid="device-starting">
        <span className="device-progress" aria-hidden="true" />
        <h2>Starting {held.title}</h2>
        <p>
          {held.kind === "emulator"
            ? "A first start takes about a minute. After that it starts from where it left off."
            : held.kind === "simulator"
              ? "A first start takes about a minute. After that it starts quicker."
              : "Connecting to the phone."}
        </p>
        {held.kind === "simulator" && tool.state === "downloading" ? (
          <>
            <progress
              className="device-meter"
              value={
                (tool.total_mb ?? 0) > 0
                  ? Math.min(1, (tool.received_mb ?? 0) / (tool.total_mb ?? 1))
                  : 0
              }
              max={1}
              aria-label="AXe download progress"
            />
            <p data-testid="device-tool">
              Downloading AXe, which Ghost drives the simulator with: {megabytes(tool.received_mb)}{" "}
              of {megabytes(tool.total_mb)}.
            </p>
          </>
        ) : null}
      </div>
    );
  return (
    <div className="device-message" data-testid="device-stopped">
      <h2>{held.title} is not running</h2>
      {held.error ? <p className="ui-error">{held.error}</p> : null}
      <button type="button" className="g-button g-button-primary g-button-28" onClick={onRetry}>
        Start it again
      </button>
    </div>
  );
}

/** Sends touches in the order they happened, and only the latest of a run of moves. */
class TouchSender {
  readonly #send: (phase: "down" | "move" | "up", x: number, y: number) => Promise<void>;
  #queue: Promise<void> = Promise.resolve();
  #move: { x: number; y: number } | null = null;
  #moving = false;

  constructor(send: (phase: "down" | "move" | "up", x: number, y: number) => Promise<void>) {
    this.#send = send;
  }

  edge(phase: "down" | "up", x: number, y: number): void {
    // A move still waiting happened before this edge: it goes first.
    const move = this.#move;
    this.#move = null;
    this.#queue = this.#queue
      .then(() => (move ? this.#send("move", move.x, move.y) : undefined))
      .then(() => this.#send(phase, x, y))
      .catch(() => {});
  }

  move(x: number, y: number): void {
    this.#move = { x, y };
    if (this.#moving) return;
    this.#moving = true;
    this.#queue = this.#queue
      .then(async () => {
        while (this.#move) {
          const next = this.#move;
          this.#move = null;
          await this.#send("move", next.x, next.y);
        }
      })
      .catch(() => {})
      .finally(() => {
        this.#moving = false;
      });
  }
}

/** The live screen: the device's video on a canvas, taking the pointer and the keyboard. */
function Screen({
  chatId,
  api,
  onError,
}: {
  readonly chatId: string;
  readonly api: MobileApi;
  readonly onError: (error: unknown) => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const player = useRef<ScreenPlayer | null>(null);
  const pressed = useRef(false);
  const [state, setState] = useState<ScreenState>({ kind: "waiting" });
  const [sender] = useState(() => new TouchSender((phase, x, y) => api.touch(chatId, phase, x, y)));

  useEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const screen = new ScreenPlayer(element, setState);
    player.current = screen;
    let stop: (() => void) | null = null;
    let cancelled = false;
    api
      .watch(chatId, (packet) => screen.push(packet))
      .then((stopWatching) => {
        if (cancelled) stopWatching();
        else stop = stopWatching;
      }, onError);
    return () => {
      cancelled = true;
      stop?.();
      screen.close();
      player.current = null;
    };
  }, [chatId, api, onError]);

  /** Where the pointer is on the device, kept inside the picture once a press has begun. */
  const point = (event: { clientX: number; clientY: number }, inside: boolean) => {
    const element = canvas.current;
    const video = player.current?.size();
    if (!element || !video) return null;
    const box = element.getBoundingClientRect();
    if (inside) return videoPoint(event.clientX, event.clientY, box, video);
    const x = Math.min(box.right - 1, Math.max(box.left, event.clientX));
    const y = Math.min(box.bottom - 1, Math.max(box.top, event.clientY));
    return videoPoint(x, y, box, video);
  };

  return (
    <div className="device-screen" data-state={state.kind}>
      {state.kind === "unsupported" ? (
        <div className="device-message" role="status">
          <h2>The screen cannot be shown here</h2>
          <p>
            This system's web view cannot play the device's video. The device still runs, and your
            agent can still use it.
          </p>
        </div>
      ) : null}
      {state.kind === "waiting" ? (
        <span className="device-progress device-progress-over" aria-hidden="true" />
      ) : null}
      {/* biome-ignore lint/a11y/noInteractiveElementToNoninteractiveRole: the screen is video that takes the pointer and the keyboard as the device would (role application) */}
      <canvas
        ref={canvas}
        className="device-canvas"
        role="application"
        aria-label="The device's screen. Click to tap, drag to swipe, type to enter text."
        aria-roledescription="device screen"
        tabIndex={0}
        hidden={state.kind === "unsupported"}
        data-testid="device-canvas"
        onContextMenu={(event) => event.preventDefault()}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          const at = point(event, true);
          if (!at) return;
          event.currentTarget.focus();
          event.currentTarget.setPointerCapture(event.pointerId);
          pressed.current = true;
          sender.edge("down", at.x, at.y);
        }}
        onPointerMove={(event) => {
          if (!pressed.current) return;
          const at = point(event, false);
          if (at) sender.move(at.x, at.y);
        }}
        onPointerUp={(event) => {
          if (!pressed.current) return;
          pressed.current = false;
          const at = point(event, false);
          if (at) sender.edge("up", at.x, at.y);
        }}
        onPointerCancel={(event) => {
          if (!pressed.current) return;
          pressed.current = false;
          const at = point(event, false);
          if (at) sender.edge("up", at.x, at.y);
        }}
        onWheel={(event) => {
          const at = point(event, true);
          if (!at) return;
          // One notch of a wheel is about 100 pixels; the device scrolls the other way round.
          const notch = (delta: number) => Math.max(-16, Math.min(16, -delta / 100));
          api.scroll(chatId, at.x, at.y, notch(event.deltaX), notch(event.deltaY)).catch(() => {});
        }}
        onKeyDown={(event) => {
          if (event.ctrlKey || event.metaKey || event.altKey) return;
          const key = deviceKey(event.key);
          if (key) {
            event.preventDefault();
            api.key(chatId, key).catch(onError);
          } else if (event.key.length === 1 || [...event.key].length === 1) {
            event.preventDefault();
            api.text(chatId, event.key).catch(onError);
          }
        }}
        onPaste={(event) => {
          const text = event.clipboardData.getData("text/plain");
          if (text) {
            event.preventDefault();
            api.text(chatId, text).catch(onError);
          }
        }}
      />
    </div>
  );
}

/** No device yet: set Android up, or choose one. */
function Chooser({
  status,
  api,
  onAttach,
  onError,
}: {
  readonly status: MobileStatus;
  readonly api: MobileApi;
  readonly onAttach: (deviceId: string | null) => void;
  readonly onError: (error: unknown) => void;
}) {
  const { setup, devices } = status;
  const phones = devices.filter((device) => device.kind === "phone");
  const emulators = devices.filter((device) => device.kind === "emulator");
  const simulators = devices.filter((device) => device.kind === "simulator");
  const slow = setup.acceleration !== null && !setup.acceleration.usable;
  const busy = setup.fetch.state !== "idle";

  if (busy) return <FetchView fetch={setup.fetch} api={api} onError={onError} />;

  const listed = [...emulators, ...simulators, ...phones];
  const blocked = (device: DeviceInfo) =>
    device.kind === "emulator"
      ? slow || !setup.emulator
      : device.kind === "simulator"
        ? !setup.ios.ready
        : false;
  return (
    <div className="device-message device-chooser" data-testid="device-chooser">
      {setup.ios.possible ? (
        <MacChoices
          setup={setup}
          slow={slow}
          onAttach={onAttach}
          onSetUp={() => api.fetchOffer().catch(onError)}
        />
      ) : setup.emulator ? (
        <>
          <h2>A phone beside the chat</h2>
          <p>
            Start an Android phone that you and your agent both use. Each chat gets its own, so two
            chats never install over each other.
          </p>
          {slow ? (
            <p className="device-warning" role="alert">
              <TriangleAlert size={15} aria-hidden="true" />
              <span>
                This computer cannot run the emulator fast yet.{" "}
                {setup.acceleration?.fix ?? setup.acceleration?.detail}
              </span>
            </p>
          ) : (
            <button
              type="button"
              className="g-button g-button-primary g-button-28"
              data-testid="device-start"
              onClick={() => onAttach(null)}
            >
              Start a phone
            </button>
          )}
        </>
      ) : (
        <>
          <h2>Set up Android</h2>
          <p>
            The Device pane shows a real Android phone beside the chat, for you and your agent.
            Ghost downloads Android's emulator from Google into its own folder. It needs about 8 GB
            of disk space.
          </p>
          <button
            type="button"
            className="g-button g-button-primary g-button-28"
            data-testid="device-setup"
            onClick={() => api.fetchOffer().catch(onError)}
          >
            See what it downloads
          </button>
        </>
      )}
      {listed.length > 0 ? (
        <ul className="device-list" aria-label="Devices">
          {listed.map((device) => (
            <DeviceRow
              key={device.id}
              device={device}
              blocked={blocked(device)}
              onUse={() => onAttach(device.id)}
              onStop={() => api.stopDevice(device.id).catch(onError)}
            />
          ))}
        </ul>
      ) : null}
      {setup.adb && phones.length === 0 ? (
        <p className="device-note">
          To use your own Android phone, turn on USB debugging in its developer options and plug it
          in.
        </p>
      ) : null}
    </div>
  );
}

/**
 * On a Mac: an Android phone and an iPhone, one row each, started or set up from the row. An
 * iPhone needs Xcode, which the person installs from Apple; the row says what is missing.
 */
function MacChoices({
  setup,
  slow,
  onAttach,
  onSetUp,
}: {
  readonly setup: MobileStatus["setup"];
  readonly slow: boolean;
  readonly onAttach: (deviceId: string | null) => void;
  readonly onSetUp: () => void;
}) {
  const ios = setup.ios;
  const android = !setup.emulator
    ? "Downloads Android's emulator from Google, about 8 GB"
    : slow
      ? (setup.acceleration?.fix ?? setup.acceleration?.detail ?? "It cannot run fast here yet.")
      : "Android's emulator";
  return (
    <>
      <h2>A phone beside the chat</h2>
      <p>
        Start an Android phone or an iPhone that you and your agent both use. Each chat gets its
        own, so two chats never install over each other.
      </p>
      <ul className="device-list" aria-label="Phones to start" data-testid="device-kinds">
        <li className="device-row">
          <span className="device-row-text">
            <span className="device-row-title">Android phone</span>
            <span className="device-row-state">{android}</span>
          </span>
          {setup.emulator ? (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              data-testid="device-start"
              disabled={slow}
              onClick={() => onAttach("android")}
            >
              Start
            </button>
          ) : (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              data-testid="device-setup"
              onClick={onSetUp}
            >
              Set up
            </button>
          )}
        </li>
        <li className="device-row">
          <span className="device-row-text">
            <span className="device-row-title">iPhone</span>
            <span className="device-row-state" data-testid="device-iphone-state">
              {ios.ready ? ios.detail : (ios.missing ?? "Looking for Xcode")}
            </span>
          </span>
          {ios.ready ? (
            <button
              type="button"
              className="g-button g-button-outline g-button-28"
              data-testid="device-start-iphone"
              onClick={() => onAttach("ios")}
            >
              Start
            </button>
          ) : null}
        </li>
      </ul>
    </>
  );
}

function stateWords(device: DeviceInfo): string {
  if (device.chat_id) return "In use by another chat";
  switch (device.state) {
    case "ready":
      return device.kind === "phone" ? "Connected" : "Running";
    case "starting":
      return "Starting";
    case "stopping":
      return "Stopping";
    case "unauthorized":
      return 'Unlock the phone and accept "Allow USB debugging"';
    case "offline":
      return "Not reachable";
    default:
      return "Stopped";
  }
}

function DeviceRow({
  device,
  blocked,
  onUse,
  onStop,
}: {
  readonly device: DeviceInfo;
  readonly blocked: boolean;
  readonly onUse: () => void;
  readonly onStop: () => void;
}) {
  const own = device.kind !== "phone";
  const usable =
    !blocked &&
    device.chat_id === null &&
    (own ? device.state === "stopped" || device.state === "ready" : device.state === "ready");
  return (
    <li className="device-row" data-testid="device-row">
      <span className="device-row-text">
        <span className="device-row-title">{device.title}</span>
        <span className="device-row-state">{device.error ?? stateWords(device)}</span>
      </span>
      {own && device.state === "ready" && device.chat_id === null ? (
        <button type="button" className="g-button g-button-ghost g-button-28" onClick={onStop}>
          Stop
        </button>
      ) : null}
      <button
        type="button"
        className="g-button g-button-outline g-button-28"
        disabled={!usable}
        onClick={onUse}
      >
        Use
      </button>
    </li>
  );
}

const PHASES = { downloading: "Downloading", checking: "Checking", unpacking: "Unpacking" };

/** Fetching Android's packages: what is offered with its licence, the progress, or why it failed. */
export function FetchView({
  fetch,
  api,
  onError,
}: {
  readonly fetch: Fetch;
  readonly api: MobileApi;
  readonly onError: (error: unknown) => void;
}) {
  if (fetch.state === "planning")
    return (
      <div className="device-message" role="status">
        <span className="device-progress" aria-hidden="true" />
        <h2>Reading Google's package list</h2>
      </div>
    );
  if (fetch.state === "offer")
    return (
      <div className="device-message device-offer" data-testid="device-offer">
        <h2>Download from Google</h2>
        <ul className="device-list" aria-label="What is downloaded">
          {fetch.items.map((item) => (
            <li className="device-row" key={item.name}>
              <span className="device-row-text">
                <span className="device-row-title">{item.name}</span>
              </span>
              <span className="device-row-state">{megabytes(item.size_mb)}</span>
            </li>
          ))}
        </ul>
        <p>
          {megabytes(fetch.total_mb)} in all, from dl.google.com, each file checked against Google's
          own checksum. They are under Google's Android SDK licence:
        </p>
        <section
          className="device-license"
          // biome-ignore lint/a11y/noNoninteractiveTabindex: a long text that scrolls, so the keyboard must reach it
          tabIndex={0}
          aria-label="Android SDK licence"
          data-testid="device-license"
        >
          {fetch.license}
        </section>
        <div className="device-actions">
          <button
            type="button"
            className="g-button g-button-primary g-button-28"
            data-testid="device-accept"
            onClick={() => api.fetchStart().catch(onError)}
          >
            Accept and download
          </button>
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            onClick={() => api.fetchCancel().catch(onError)}
          >
            Not now
          </button>
        </div>
      </div>
    );
  if (fetch.state === "running") {
    const total = fetch.total_mb ?? 0;
    const share = total > 0 ? Math.min(1, (fetch.received_mb ?? 0) / total) : 0;
    return (
      <div className="device-message" data-testid="device-fetching">
        <h2>Downloading Android</h2>
        <progress className="device-meter" value={share} max={1} aria-label="Download progress" />
        <p role="status">
          {PHASES[fetch.phase]} {fetch.name}. {megabytes(fetch.received_mb)} of{" "}
          {megabytes(fetch.total_mb)}.
        </p>
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          onClick={() => api.fetchCancel().catch(onError)}
        >
          Stop
        </button>
      </div>
    );
  }
  if (fetch.state === "failed")
    return (
      <div className="device-message" data-testid="device-fetch-failed">
        <h2>Android was not downloaded</h2>
        <p className="ui-error" role="alert">
          {fetch.message}
        </p>
        <div className="device-actions">
          <button
            type="button"
            className="g-button g-button-primary g-button-28"
            onClick={() => api.fetchOffer().catch(onError)}
          >
            Try again
          </button>
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            onClick={() => api.fetchCancel().catch(onError)}
          >
            Close
          </button>
        </div>
      </div>
    );
  return null;
}
