/**
 * The Game pane's Assets view (docs/game.md, "Assets"): the pictures, sounds and 3D models of the
 * chat's folder, the newest first. A picture opens large, and a sprite sheet plays its frames;
 * a sound shows its shape and plays; a model turns under the pointer. Nothing here makes art:
 * it shows what the person and their agent put in the project.
 */
import {
  AudioLines,
  Box,
  Copy,
  ImageOff,
  MessageSquarePlus,
  Pause,
  Play,
  RefreshCw,
  X,
} from "lucide-react";
import {
  type CSSProperties,
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { AssetKind, GameAsset, GameAssets } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import type { GameApi } from "../../lib/game";
import {
  AssetBytes,
  assetFolder,
  assetName,
  counts,
  frameCount,
  frameRect,
  guessFrames,
  KINDS,
  MAKERS,
  matching,
  peaks,
  sizeText,
  soundLength,
} from "../../lib/gameAssets";

const ModelViewer = lazy(() => import("./ModelViewer"));

/** Makers whose own tools an agent can use for art and sound, each by its own guide. */
function Makers({ agentName }: { readonly agentName: string }) {
  return (
    <details className="game-makers" data-testid="game-makers">
      <summary>Where art and sound can come from</summary>
      <p className="game-note">
        Ghost shows what is in the folder; it makes none of it. These makers publish tools{" "}
        {agentName} can use. Each needs an account of your own with them, and their guide says how
        to connect it.
      </p>
      <ul className="game-makers-list">
        {MAKERS.map((maker) => (
          <li key={maker.name} className="game-row">
            <span className="game-row-text">
              <span className="game-row-title">{maker.name}</span>
              <span className="game-row-what">{maker.makes}</span>
            </span>
            <a
              className="g-button g-button-ghost g-button-28"
              href={maker.guide}
              target="_blank"
              rel="noreferrer noopener"
              aria-label={`${maker.name}'s guide`}
            >
              Its guide
            </a>
          </li>
        ))}
      </ul>
    </details>
  );
}

/** How many cards are drawn before the person asks for more. */
const PAGE = 240;

type Loaded = { readonly url: string; readonly bytes: Uint8Array; readonly mime: string };

/** An asset's bytes while something shows it: nothing until they arrive, a reason when they do not. */
function useAssetBytes(
  bytes: AssetBytes,
  asset: GameAsset | null,
  wanted = true,
): { readonly loaded: Loaded | null; readonly problem: string | null } {
  const [state, setState] = useState<{ loaded: Loaded | null; problem: string | null }>({
    loaded: null,
    problem: null,
  });
  const key = asset ? `${asset.modified}:${asset.path}` : "";
  // biome-ignore lint/correctness/useExhaustiveDependencies: the key is the asset's identity; a new list hands the same asset as a new object.
  useEffect(() => {
    setState({ loaded: null, problem: null });
    if (!asset || !wanted || asset.too_large) return;
    let current = true;
    const held = bytes.acquire(asset);
    held.promise.then(
      (loaded) => {
        if (current) setState({ loaded, problem: null });
      },
      (error) => {
        if (current) setState({ loaded: null, problem: errorMessage(error) });
      },
    );
    return () => {
      current = false;
      held.release();
    };
  }, [bytes, key, wanted]);
  return state;
}

/** A card's picture, read only once the card is in sight. */
function Thumb({ asset, bytes }: { readonly asset: GameAsset; readonly bytes: AssetBytes }) {
  const box = useRef<HTMLSpanElement>(null);
  const [seen, setSeen] = useState(false);
  const [small, setSmall] = useState(false);
  useEffect(() => {
    const node = box.current;
    if (!node || seen) return;
    if (typeof IntersectionObserver === "undefined") {
      setSeen(true);
      return;
    }
    const watching = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) setSeen(true);
      },
      { rootMargin: "200px" },
    );
    watching.observe(node);
    return () => watching.disconnect();
  }, [seen]);
  const { loaded, problem } = useAssetBytes(bytes, asset, seen && asset.kind === "picture");
  return (
    <span className="game-asset-thumb" data-kind={asset.kind} ref={box} aria-hidden="true">
      {asset.kind === "sound" ? (
        <AudioLines size={22} />
      ) : asset.kind === "model" ? (
        <Box size={22} />
      ) : loaded ? (
        <img
          src={loaded.url}
          alt=""
          draggable={false}
          data-small={small ? "" : undefined}
          // Pixel art stays sharp: a small picture is shown in whole pixels.
          onLoad={(event) =>
            setSmall(
              Math.max(event.currentTarget.naturalWidth, event.currentTarget.naturalHeight) <= 128,
            )
          }
        />
      ) : asset.too_large || problem ? (
        <ImageOff size={20} />
      ) : null}
    </span>
  );
}

/** Whether the person asked the computer for less motion. */
function calm(): boolean {
  if (typeof document !== "undefined") {
    const motion = document.documentElement.dataset.motion;
    if (motion === "reduced" || motion === "off") return true;
  }
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function PictureDetail({ asset, loaded }: { readonly asset: GameAsset; readonly loaded: Loaded }) {
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  const [frames, setFrames] = useState({ across: 1, down: 1 });
  const [rate, setRate] = useState(8);
  const [playing, setPlaying] = useState(() => !calm());
  const image = useRef<HTMLImageElement | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const count = frames.across * frames.down;
  // The picture's own size, and a first guess at how it is cut.
  useEffect(() => {
    setSize(null);
    const picture = new Image();
    picture.onload = () => {
      image.current = picture;
      setSize({ width: picture.naturalWidth, height: picture.naturalHeight });
      setFrames(guessFrames(picture.naturalWidth, picture.naturalHeight));
    };
    picture.src = loaded.url;
    return () => {
      picture.onload = null;
    };
  }, [loaded.url]);
  // A sheet's frames, one after another.
  useEffect(() => {
    const [element, picture] = [canvas.current, image.current];
    if (!element || !picture || !size || count <= 1) return;
    const cell = frameRect(0, frames.across, frames.down, size.width, size.height);
    element.width = Math.max(1, Math.round(cell.width));
    element.height = Math.max(1, Math.round(cell.height));
    const context = element.getContext("2d");
    if (!context) return;
    context.imageSmoothingEnabled = false;
    let index = 0;
    const draw = () => {
      const at = frameRect(index, frames.across, frames.down, size.width, size.height);
      context.clearRect(0, 0, element.width, element.height);
      context.drawImage(
        picture,
        at.x,
        at.y,
        at.width,
        at.height,
        0,
        0,
        element.width,
        element.height,
      );
      element.dataset.frame = String(index);
    };
    draw();
    if (!playing) return;
    const timer = window.setInterval(() => {
      index = (index + 1) % count;
      draw();
    }, 1000 / rate);
    return () => window.clearInterval(timer);
  }, [size, frames, count, rate, playing]);
  const small = size !== null && Math.max(size.width, size.height) <= 256;
  const cell = size ? frameRect(0, frames.across, frames.down, size.width, size.height) : null;
  return (
    <>
      <div className="game-asset-stage" data-small={small ? "" : undefined}>
        {count > 1 && size ? (
          <canvas
            ref={canvas}
            className="game-asset-frames"
            role="img"
            aria-label={`${assetName(asset.path)}, its ${count} frames in turn`}
            data-testid="game-asset-frames"
          />
        ) : (
          <img
            className="game-asset-picture"
            src={loaded.url}
            alt={assetName(asset.path)}
            draggable={false}
          />
        )}
      </div>
      <div className="game-asset-controls">
        <span className="game-note" data-testid="game-asset-size">
          {size ? `${size.width}×${size.height}` : "Reading it"}
          {size && cell && count > 1
            ? ` · ${count} frames of ${Math.round(cell.width)}×${Math.round(cell.height)}`
            : ""}
        </span>
        <label className="game-asset-field">
          Frames across
          <input
            type="number"
            min={1}
            max={64}
            value={frames.across}
            onChange={(event) =>
              setFrames({ ...frames, across: frameCount(event.currentTarget.value) })
            }
          />
        </label>
        <label className="game-asset-field">
          down
          <input
            type="number"
            min={1}
            max={64}
            value={frames.down}
            onChange={(event) =>
              setFrames({ ...frames, down: frameCount(event.currentTarget.value) })
            }
          />
        </label>
        {count > 1 ? (
          <>
            <label className="game-asset-field">
              a second
              <input
                type="number"
                min={1}
                max={30}
                value={rate}
                onChange={(event) =>
                  setRate(
                    Math.min(30, Math.max(1, Math.round(Number(event.currentTarget.value)) || 1)),
                  )
                }
              />
            </label>
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28"
              aria-label={playing ? "Hold the frames still" : "Play the frames"}
              aria-pressed={playing}
              data-testid="game-asset-play-frames"
              onClick={() => setPlaying(!playing)}
            >
              {playing ? <Pause size={14} /> : <Play size={14} />}
            </button>
          </>
        ) : null}
      </div>
    </>
  );
}

function SoundDetail({ asset, loaded }: { readonly asset: GameAsset; readonly loaded: Loaded }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const audio = useRef<AudioContext | null>(null);
  const source = useRef<AudioBufferSourceNode | null>(null);
  const [sound, setSound] = useState<AudioBuffer | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  // The sound is read into the window's own audio, which also plays it: no player is pointed
  // at a file.
  useEffect(() => {
    setSound(null);
    setProblem(null);
    let current = true;
    const Audio =
      window.AudioContext ??
      (window as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Audio) {
      setProblem("This window cannot play sounds.");
      return;
    }
    const context = new Audio();
    audio.current = context;
    const copy = loaded.bytes.buffer.slice(
      loaded.bytes.byteOffset,
      loaded.bytes.byteOffset + loaded.bytes.byteLength,
    ) as ArrayBuffer;
    context.decodeAudioData(copy).then(
      (buffer) => {
        if (current) setSound(buffer);
      },
      () => {
        if (current) setProblem("This sound cannot be read here. The game may still play it.");
      },
    );
    return () => {
      current = false;
      source.current?.stop();
      source.current = null;
      void context.close().catch(() => {});
      audio.current = null;
    };
  }, [loaded.bytes]);
  // Its shape: the quietest and the loudest of each slice, as bars.
  useEffect(() => {
    const element = canvas.current;
    if (!element || !sound) return;
    const box = element.getBoundingClientRect();
    const ratio = Math.min(2, window.devicePixelRatio || 1);
    element.width = Math.max(1, Math.round(box.width * ratio));
    element.height = Math.max(1, Math.round(box.height * ratio));
    const context = element.getContext("2d");
    if (!context) return;
    const bars = Math.max(24, Math.floor(box.width / 3));
    const shape = peaks(sound.getChannelData(0), bars);
    context.clearRect(0, 0, element.width, element.height);
    context.fillStyle = getComputedStyle(element).color;
    const [step, middle] = [element.width / bars, element.height / 2];
    shape.forEach(([low, high], index) => {
      const top = middle - high * middle;
      const height = Math.max(ratio, (high - low) * middle);
      context.fillRect(index * step, top, Math.max(1, step - ratio), height);
    });
    element.dataset.drawn = String(bars);
  }, [sound]);
  const toggle = () => {
    const context = audio.current;
    if (!context || !sound) return;
    if (source.current) {
      source.current.stop();
      return;
    }
    const node = context.createBufferSource();
    node.buffer = sound;
    node.connect(context.destination);
    node.onended = () => {
      if (source.current === node) source.current = null;
      setPlaying(false);
    };
    source.current = node;
    void context.resume().catch(() => {});
    node.start();
    setPlaying(true);
  };
  return (
    <>
      <div className="game-asset-stage" data-kind="sound">
        <canvas
          ref={canvas}
          className="game-asset-wave"
          role="img"
          aria-label={`The shape of ${assetName(asset.path)}`}
          data-testid="game-asset-wave"
        />
      </div>
      <div className="game-asset-controls">
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          disabled={!sound}
          aria-pressed={playing}
          data-testid="game-asset-play"
          onClick={toggle}
        >
          {playing ? <Pause size={14} /> : <Play size={14} />}
          {playing ? "Stop" : "Play"}
        </button>
        <span className="game-note" data-testid="game-asset-length">
          {problem ??
            (sound
              ? `${soundLength(sound.duration)} · ${sound.numberOfChannels === 1 ? "mono" : "stereo"}`
              : "Reading it")}
        </span>
      </div>
    </>
  );
}

function Detail({
  asset,
  bytes,
  agentName,
  onAdd,
  onClose,
}: {
  readonly asset: GameAsset;
  readonly bytes: AssetBytes;
  readonly agentName: string;
  readonly onAdd?: (text: string) => Promise<string | null>;
  readonly onClose: () => void;
}) {
  const { loaded, problem } = useAssetBytes(bytes, asset);
  const [said, setSaid] = useState<string | null>(null);
  const close = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    close.current?.focus();
  }, []);
  // Esc goes back to the list wherever the keyboard is: a click on a button does not move it
  // there in every browser, so the sheet cannot count on hearing its own keys.
  useEffect(() => {
    const heard = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", heard);
    return () => window.removeEventListener("keydown", heard);
  }, [onClose]);
  const folder = assetFolder(asset.path);
  const glb = asset.path.toLowerCase().endsWith(".glb");
  return (
    <section
      className="game-asset-detail"
      aria-label={assetName(asset.path)}
      data-testid="game-asset-detail"
    >
      <div className="game-log-head">
        <h2 title={asset.path}>
          {assetName(asset.path)}
          <span className="game-tune-script">
            {folder ? `${folder} · ` : ""}
            {sizeText(asset.bytes)}
          </span>
        </h2>
        <button
          ref={close}
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Back to the assets"
          onClick={onClose}
        >
          <X size={14} />
        </button>
      </div>
      {asset.too_large ? (
        <p className="game-note game-asset-said" data-testid="game-asset-too-large">
          This file is {sizeText(asset.bytes)}, more than Ghost reads to show it. The game uses it
          as it is.
        </p>
      ) : problem ? (
        <p className="ui-error game-asset-said" role="alert">
          {problem}
        </p>
      ) : !loaded ? (
        <p className="game-note game-asset-said" role="status">
          Reading it
        </p>
      ) : asset.kind === "picture" ? (
        <PictureDetail asset={asset} loaded={loaded} />
      ) : asset.kind === "sound" ? (
        <SoundDetail asset={asset} loaded={loaded} />
      ) : glb ? (
        <Suspense
          fallback={
            <p className="game-note game-asset-said" role="status">
              Opening the viewer
            </p>
          }
        >
          <ModelViewer bytes={loaded.bytes} name={assetName(asset.path)} />
        </Suspense>
      ) : (
        <p className="game-note game-asset-said" data-testid="game-asset-gltf">
          A .gltf keeps its shapes and pictures in files beside it, which this viewer does not
          gather. A .glb holds them in one file and opens here.
        </p>
      )}
      <div className="game-asset-foot">
        <span className="game-note" role="status">
          {said ?? ""}
        </span>
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          data-testid="game-asset-copy"
          onClick={() => {
            navigator.clipboard?.writeText(asset.path).then(
              () => setSaid("Path copied."),
              () => setSaid("The path could not be copied."),
            );
          }}
        >
          <Copy size={13} />
          Copy path
        </button>
        {onAdd ? (
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            title={`Put this file's path in your message to ${agentName}`}
            data-testid="game-asset-add"
            onClick={() => {
              onAdd(`\`${asset.path}\``).then(
                (why) => setSaid(why ?? "Added to the message box."),
                (error) => setSaid(errorMessage(error)),
              );
            }}
          >
            <MessageSquarePlus size={14} />
            Add to chat
          </button>
        ) : null}
      </div>
    </section>
  );
}

export function AssetsView({
  chatId,
  api,
  agentName,
  wanted,
  onWantedShown,
  onAdd,
}: {
  readonly chatId: string;
  readonly api: GameApi;
  readonly agentName: string;
  /** An asset to open at once: the chat showed it as new and the person chose it. */
  readonly wanted: string | null;
  readonly onWantedShown: () => void;
  /** Puts words in the chat's message box; answers why it could not, or null. */
  readonly onAdd?: (text: string) => Promise<string | null>;
}) {
  const [list, setList] = useState<GameAssets | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [kind, setKind] = useState<AssetKind | null>(null);
  const [typed, setTyped] = useState("");
  const [open, setOpen] = useState<GameAsset | null>(null);
  const [shown, setShown] = useState(PAGE);
  const [reading, setReading] = useState(0);
  const bytes = useMemo(() => new AssetBytes((path) => api.asset(chatId, path)), [api, chatId]);
  useEffect(() => () => bytes.clear(), [bytes]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `reading` asks for the list again.
  useEffect(() => {
    let current = true;
    api.assets(chatId).then(
      (next) => {
        if (!current) return;
        setProblem(null);
        setList(next);
      },
      (error) => {
        if (current) setProblem(errorMessage(error));
      },
    );
    return () => {
      current = false;
    };
  }, [api, chatId, reading]);

  // The asset the chat asked for opens once the list that holds it is here.
  useEffect(() => {
    if (wanted === null || list === null) return;
    const found = list.assets.find((asset) => asset.path === wanted);
    if (found) setOpen(found);
    onWantedShown();
  }, [wanted, list, onWantedShown]);

  const all = list?.assets ?? [];
  const found = useMemo(() => matching(all, kind, typed), [all, kind, typed]);
  const each = useMemo(() => counts(all), [all]);
  const closeDetail = useCallback(() => setOpen(null), []);

  return (
    <div className="game-assets" data-testid="game-assets">
      <div className="game-assets-bar">
        <fieldset className="game-assets-kinds">
          <legend className="sr-only">Which assets to show</legend>
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            aria-pressed={kind === null}
            data-testid="game-assets-all"
            onClick={() => setKind(null)}
          >
            All <span className="game-assets-count">{all.length}</span>
          </button>
          {KINDS.filter((entry) => each[entry.kind] > 0).map((entry) => (
            <button
              key={entry.kind}
              type="button"
              className="g-button g-button-ghost g-button-28"
              aria-pressed={kind === entry.kind}
              data-testid={`game-assets-${entry.kind}`}
              onClick={() => setKind(kind === entry.kind ? null : entry.kind)}
            >
              {entry.many} <span className="game-assets-count">{each[entry.kind]}</span>
            </button>
          ))}
        </fieldset>
        <input
          className="game-assets-find"
          type="search"
          aria-label="Find an asset by its name or folder"
          placeholder="Find"
          value={typed}
          onChange={(event) => {
            setTyped(event.currentTarget.value);
            setShown(PAGE);
          }}
        />
        <button
          type="button"
          className="g-button g-button-ghost g-button-icon g-button-28"
          aria-label="Look at the folder again"
          title="Look at the folder again"
          data-testid="game-assets-again"
          onClick={() => setReading((count) => count + 1)}
        >
          <RefreshCw size={14} />
        </button>
      </div>
      {problem ? (
        <p className="ui-error game-asset-said" role="alert">
          {problem}
        </p>
      ) : list === null ? (
        <p className="game-note game-asset-said" role="status">
          Looking through the folder
        </p>
      ) : all.length === 0 ? (
        <div className="game-message" data-testid="game-assets-none">
          <h2>No art or sound here yet</h2>
          <p>
            Pictures, sounds and 3D models in this chat's folder show here, the newest first. Ask{" "}
            {agentName} to make some, or put your own in the folder.
          </p>
          <Makers agentName={agentName} />
        </div>
      ) : found.length === 0 ? (
        <p className="game-note game-asset-said" data-testid="game-assets-no-match">
          Nothing here has that in its name.
        </p>
      ) : (
        <div className="game-assets-scroll">
          <ul className="game-assets-grid">
            {found.slice(0, shown).map((asset) => (
              <li key={asset.path}>
                <button
                  type="button"
                  className="game-asset"
                  title={asset.path}
                  data-testid="game-asset"
                  data-path={asset.path}
                  onClick={() => setOpen(asset)}
                >
                  <Thumb asset={asset} bytes={bytes} />
                  <span className="game-asset-name">{assetName(asset.path)}</span>
                  <span className="game-asset-meta">
                    {assetFolder(asset.path) || sizeText(asset.bytes)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          {found.length > shown ? (
            <button
              type="button"
              className="g-button g-button-ghost g-button-28 game-assets-more"
              data-testid="game-assets-more"
              onClick={() => setShown(shown + PAGE)}
            >
              Show {Math.min(PAGE, found.length - shown)} more
            </button>
          ) : null}
          {list.cut ? (
            <p className="game-note game-asset-said">
              This folder holds more than Ghost lists. These are its newest.
            </p>
          ) : null}
          <Makers agentName={agentName} />
        </div>
      )}
      {open ? (
        <Detail
          key={`${open.modified}:${open.path}`}
          asset={open}
          bytes={bytes}
          agentName={agentName}
          onAdd={onAdd}
          onClose={closeDetail}
        />
      ) : null}
    </div>
  );
}

/**
 * What a turn left in the folder, above the message box: the pictures, sounds and models that
 * are new or changed since the turn began. A click opens one in the Game pane.
 */
export function NewAssets({
  assets,
  onOpen,
  onDismiss,
}: {
  readonly assets: readonly GameAsset[];
  readonly onOpen: (path: string) => void;
  readonly onDismiss: () => void;
}) {
  const few = assets.slice(0, 5);
  return (
    <section className="game-new-assets" aria-label="New assets" data-testid="game-new-assets">
      <span className="game-new-assets-text">
        {assets.length === 1 ? "1 new asset" : `${assets.length} new assets`}
      </span>
      <ul className="game-new-assets-list">
        {few.map((asset) => (
          <li key={asset.path} className="game-new-assets-item">
            <button
              type="button"
              className="g-button g-button-ghost g-button-28 game-new-asset"
              title={`Open ${asset.path} in Assets`}
              data-testid="game-new-asset"
              style={
                { "--game-new-chars": Math.min(22, assetName(asset.path).length) } as CSSProperties
              }
              onClick={() => onOpen(asset.path)}
            >
              {asset.kind === "sound" ? (
                <AudioLines size={13} aria-hidden="true" />
              ) : asset.kind === "model" ? (
                <Box size={13} aria-hidden="true" />
              ) : null}
              <span className="game-new-asset-name">{assetName(asset.path)}</span>
            </button>
          </li>
        ))}
      </ul>
      {assets.length > few.length ? (
        <button
          type="button"
          className="g-button g-button-ghost g-button-28"
          data-testid="game-new-assets-all"
          onClick={() => onOpen(few[0]?.path ?? "")}
        >
          and {assets.length - few.length} more
        </button>
      ) : null}
      <button
        type="button"
        className="g-button g-button-ghost g-button-icon g-button-24 game-new-assets-close"
        aria-label="Dismiss the new assets"
        onClick={onDismiss}
      >
        <X size={13} />
      </button>
    </section>
  );
}
