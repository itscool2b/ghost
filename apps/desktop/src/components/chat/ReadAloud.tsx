import { Square, Volume2 } from "lucide-react";
import { useSyncExternalStore } from "react";
import { type SpeechPlayer, speechPlayer } from "../../lib/speech";

/**
 * Read aloud, in the quiet row under an answer (owner, 22 November 2026): the answer's words
 * spoken in the voice chosen in Settings, by a model that runs on this device. The same button
 * stops it. It is not shown where reading aloud cannot run (a build without the voice model, a
 * processor the engine cannot use), and a failure says why beside it.
 *
 * `id` names what is read, so only this answer's button shows as speaking; `text` is the answer
 * as Markdown.
 */
export function ReadAloud({
  id,
  text,
  player = speechPlayer,
}: {
  readonly id: string;
  readonly text: string;
  readonly player?: SpeechPlayer;
}) {
  const snapshot = useSyncExternalStore(player.subscribe, player.getSnapshot, player.getSnapshot);
  if (snapshot.available !== true) return null;
  const phase = snapshot.key === id ? snapshot.phase : "idle";
  const label =
    phase === "idle"
      ? "Read aloud"
      : phase === "preparing"
        ? "Getting the voice ready"
        : "Stop reading";
  return (
    <>
      <button
        type="button"
        className="chat-action chat-read-aloud"
        aria-label={phase === "idle" ? "Read aloud" : "Stop reading"}
        aria-pressed={phase !== "idle"}
        title={label}
        data-state={phase}
        data-testid="read-aloud"
        data-tour="read-aloud"
        onPointerEnter={() => player.prepare()}
        onFocus={() => player.prepare()}
        onClick={() => player.toggle(id, text)}
      >
        {phase === "idle" ? <Volume2 size={16} /> : <Square size={14} />}
        <span className="sr-only" aria-live="polite">
          {phase === "speaking" ? "Reading aloud" : ""}
        </span>
      </button>
      {snapshot.failedKey === id && snapshot.error ? (
        <span role="alert" className="chat-action-error" data-testid="read-aloud-error">
          {snapshot.error}
        </span>
      ) : null}
    </>
  );
}
