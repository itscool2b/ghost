/**
 * The parts of the Game pane that hand something to the agent (docs/game.md, "Telling the
 * agent"): the picture that marks what was pointed at, and the sheet of values a person turns
 * while the game runs.
 */
import { Crosshair, RotateCcw, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import type { GameTunable, GameTunables } from "../../bindings";
import { errorMessage } from "../../lib/errors";
import type { GameApi } from "../../lib/game";
import { num, shownValue, tunableRange, turned } from "../../lib/gameTell";

/** A colour from the theme as `rgb()`, for a mark drawn into a picture. */
export function themeColor(token: string, fallback: string): string {
  if (typeof document === "undefined") return fallback;
  const probe = document.createElement("span");
  probe.style.color = `var(${token})`;
  probe.style.display = "none";
  document.body.append(probe);
  const value = getComputedStyle(probe).color;
  probe.remove();
  return /^rgba?\(/.test(value) ? value : fallback;
}

async function pngOf(canvas: HTMLCanvasElement): Promise<Uint8Array | null> {
  const blob = await new Promise<Blob | null>((done) => canvas.toBlob(done, "image/png"));
  return blob ? new Uint8Array(await blob.arrayBuffer()) : null;
}

/**
 * The game's picture as the pane shows it now, with what was pointed at outlined: a PNG.
 * `area` is in the game's own pixels, which the picture may be a smaller copy of.
 */
export async function markedPicture(
  shown: HTMLCanvasElement,
  area: readonly number[],
  gameWidth: number,
  color: string,
): Promise<Uint8Array | null> {
  if (shown.width === 0 || shown.height === 0) return null;
  const canvas = document.createElement("canvas");
  canvas.width = shown.width;
  canvas.height = shown.height;
  const context = canvas.getContext("2d");
  if (!context) return null;
  context.drawImage(shown, 0, 0);
  const scale = gameWidth > 0 ? shown.width / gameWidth : 1;
  const [x = 0, y = 0, width = 0, height = 0] = area.map((value) => value * scale);
  const line = Math.max(2, Math.round(shown.width / 320));
  // A dark line under the coloured one: the mark reads on a bright game and on a dark one.
  context.lineJoin = "round";
  context.strokeStyle = "rgba(0, 0, 0, 0.7)";
  context.lineWidth = line + 2;
  context.strokeRect(x - line, y - line, width + 2 * line, height + 2 * line);
  context.strokeStyle = color;
  context.lineWidth = line;
  context.strokeRect(x - line, y - line, width + 2 * line, height + 2 * line);
  return pngOf(canvas);
}

/** A picture the game sent (JPEG) as the PNG the message box takes. */
export async function pngFrom(jpeg: readonly number[] | Uint8Array): Promise<Uint8Array | null> {
  try {
    const bitmap = await createImageBitmap(
      new Blob([new Uint8Array(jpeg)], { type: "image/jpeg" }),
    );
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    canvas.getContext("2d")?.drawImage(bitmap, 0, 0);
    bitmap.close();
    return await pngOf(canvas);
  } catch {
    return null;
  }
}

/**
 * The values a node's script exports, as sliders and switches that change the running game as
 * they move. They change this run only: Keep hands what was turned to the agent, which writes
 * it into the project.
 */
export function TuneSheet({
  chatId,
  api,
  node,
  agentName,
  canKeep,
  onKeep,
  onPoint,
  onClose,
}: {
  readonly chatId: string;
  readonly api: GameApi;
  /** The node whose values are shown: a path in the running game, or "." for its scene. */
  readonly node: string;
  readonly agentName: string;
  /** The chat can take a message now. */
  readonly canKeep: boolean;
  /** Hands the turned values to the agent; answers why it could not, or null. */
  readonly onKeep: (
    tunables: GameTunables,
    changes: ReturnType<typeof turned>,
  ) => Promise<string | null>;
  /** Lets the person choose another node by pointing at it. */
  readonly onPoint: () => void;
  readonly onClose: () => void;
}) {
  const [tunables, setTunables] = useState<GameTunables | null>(null);
  /** The values as they were when the sheet opened on this node: what Reset goes back to. */
  const [first, setFirst] = useState<readonly GameTunable[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const [keeping, setKeeping] = useState(false);
  /** The newest value of each property that has not been sent yet: one call at a time each. */
  const waiting = useRef(new Map<string, { kind: string; value: number }>());
  const busy = useRef(new Set<string>());

  useEffect(() => {
    let current = true;
    setTunables(null);
    setProblem(null);
    api.tunables(chatId, node).then(
      (next) => {
        if (!current) return;
        setTunables(next);
        setFirst(next.values);
      },
      (error) => {
        if (current) setProblem(errorMessage(error));
      },
    );
    return () => {
      current = false;
    };
  }, [api, chatId, node]);

  const path = tunables?.path ?? node;
  const push = useCallback(
    (name: string) => {
      if (busy.current.has(name)) return;
      const next = waiting.current.get(name);
      if (!next) return;
      waiting.current.delete(name);
      busy.current.add(name);
      api
        .setValue(chatId, path, name, next.kind, next.value)
        .catch((error) => setProblem(errorMessage(error)))
        .finally(() => {
          busy.current.delete(name);
          push(name);
        });
    },
    [api, chatId, path],
  );
  const set = (tunable: GameTunable, value: number) => {
    setTunables((current) =>
      current
        ? {
            ...current,
            values: current.values.map((entry) =>
              entry.name === tunable.name ? { ...entry, value } : entry,
            ),
          }
        : current,
    );
    waiting.current.set(tunable.name, { kind: tunable.kind, value });
    push(tunable.name);
  };
  const changes = tunables ? turned(tunables.values, first) : [];
  const reset = () => {
    for (const change of changes) {
      const tunable = tunables?.values.find((entry) => entry.name === change.name);
      if (tunable) set(tunable, change.was);
    }
  };
  const keep = () => {
    if (!tunables || changes.length === 0) return;
    setKeeping(true);
    setProblem(null);
    onKeep(tunables, changes).then(
      (why) => {
        setKeeping(false);
        if (why) setProblem(why);
        // What was kept is what Reset goes back to from now on.
        else setFirst(tunables.values);
      },
      (error) => {
        setKeeping(false);
        setProblem(errorMessage(error));
      },
    );
  };

  return (
    <section className="game-log game-tune" aria-label="Tune values" data-testid="game-tune-sheet">
      <div className="game-log-head">
        <h2 title={tunables?.path}>
          {tunables ? `Tune ${tunables.name}` : "Tune"}
          {tunables?.script ? <span className="game-tune-script">{tunables.script}</span> : null}
        </h2>
        <span className="game-tune-head-end">
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Tune something else: point at it in the game"
            title="Tune something else: point at it in the game"
            data-testid="game-tune-point"
            onClick={onPoint}
          >
            <Crosshair size={14} />
          </button>
          <button
            type="button"
            className="g-button g-button-ghost g-button-icon g-button-28"
            aria-label="Close the values"
            onClick={onClose}
          >
            <X size={14} />
          </button>
        </span>
      </div>
      <div className="game-log-body game-tune-body">
        {problem ? (
          <p className="ui-error" role="alert" data-testid="game-tune-problem">
            {problem}
          </p>
        ) : null}
        {tunables === null && !problem ? (
          <p className="game-note" role="status">
            Reading its values
          </p>
        ) : null}
        {tunables && tunables.values.length === 0 ? (
          <p className="game-note" data-testid="game-tune-none">
            {tunables.name} has no values to turn. A script offers one with @export, as a number or
            a switch. Point at something else, or ask {agentName} to export what you want to tune.
          </p>
        ) : null}
        {tunables && tunables.values.length > 0 ? (
          <ul className="game-tune-list">
            {tunables.values.map((tunable) => {
              const id = `game-tune-${tunable.name}`;
              const range = tunableRange(tunable);
              const value = num(tunable.value);
              const was = first.find((entry) => entry.name === tunable.name)?.value;
              const moved = was !== undefined && num(was) !== value;
              return (
                <li
                  key={tunable.name}
                  className="game-tune-row"
                  data-moved={moved ? "" : undefined}
                  data-testid="game-tunable"
                >
                  <label className="game-tune-name" htmlFor={id} title={tunable.name}>
                    {tunable.name}
                  </label>
                  {tunable.kind === "bool" ? (
                    <input
                      id={id}
                      className="g-switch"
                      type="checkbox"
                      role="switch"
                      aria-checked={value !== 0}
                      checked={value !== 0}
                      onChange={(event) => set(tunable, event.currentTarget.checked ? 1 : 0)}
                    />
                  ) : (
                    <input
                      id={id}
                      className="game-tune-slider"
                      type="range"
                      min={range.min}
                      max={range.max}
                      step={range.step}
                      value={Math.min(range.max, Math.max(range.min, value))}
                      aria-valuetext={shownValue(tunable.kind, value)}
                      onChange={(event) => set(tunable, Number(event.currentTarget.value))}
                    />
                  )}
                  <output className="game-tune-value" htmlFor={id}>
                    {shownValue(tunable.kind, value)}
                  </output>
                </li>
              );
            })}
          </ul>
        ) : null}
      </div>
      {tunables && tunables.values.length > 0 ? (
        <div className="game-tune-foot">
          <span className="game-note">
            {changes.length === 0
              ? "Turn a value and the game changes as it runs."
              : "Changed in this run only."}
          </span>
          <button
            type="button"
            className="g-button g-button-ghost g-button-28"
            disabled={changes.length === 0 || keeping}
            data-testid="game-tune-reset"
            onClick={reset}
          >
            <RotateCcw size={13} />
            Reset
          </button>
          <button
            type="button"
            className="g-button g-button-primary g-button-28"
            disabled={changes.length === 0 || keeping || !canKeep}
            title={
              canKeep
                ? `Ask ${agentName} to write these values into the project`
                : "This chat cannot take a message now"
            }
            data-testid="game-tune-keep"
            onClick={keep}
          >
            Keep
          </button>
        </div>
      ) : null}
    </section>
  );
}
