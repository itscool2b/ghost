import { Check, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { DictationModel } from "../../bindings";
import {
  type DictationApi,
  DictationController,
  dictationBusy,
  liveTextOf,
  liveWords,
  MISSING_MESSAGE,
  recordingTime,
  UNSUPPORTED_MESSAGE,
} from "../../lib/dictation";
import type { SpeechApi } from "../../lib/speech";
import { Wave } from "../chat/Dictation";
import "../chat/dictation.css";
import { LargerModel } from "./LargerModel";
import type { Preferences } from "./preferences";
import { ReadAloudSettings } from "./ReadAloudSettings";
import { Row } from "./SettingsRow";

/** What dictation's bundled models say about this device. */
function modelDetail(model: DictationModel | null | "failed"): string {
  if (model === null) return "Checking the built-in speech model.";
  if (model === "failed") return "o7 could not read the speech model's status.";
  if (!model.supported) return UNSUPPORTED_MESSAGE;
  if (!model.installed) return MISSING_MESSAGE;
  if (model.larger.state === "ready")
    return `${model.name}, downloaded and ready: English and 24 other European languages, with punctuation and capitals.`;
  return `${model.name}, built in and ready: English, with punctuation and capitals.`;
}

/**
 * Settings' Voice page: dictation's built-in model and the larger one it can fetch, the microphone
 * it hears, and a test that shows
 * the level and the words without a chat (docs/local-dictation.md); then reading aloud, with the
 * voice answers are read in (docs/read-aloud.md).
 */
export function VoicePage({
  api,
  speech,
  prefs,
}: {
  readonly api: DictationApi;
  /** Reading aloud, with the saved preferences its voice is one of. A view without them shows dictation alone. */
  readonly speech?: SpeechApi;
  readonly prefs?: Preferences;
}) {
  const [model, setModel] = useState<DictationModel | null | "failed">(null);
  const shown = useRef(true);
  // Read again whenever the larger model arrives or goes: the model in use changes with it.
  const read = useCallback(() => {
    api.model().then(
      (value) => {
        if (shown.current) setModel(value);
      },
      () => {
        if (shown.current) setModel("failed");
      },
    );
  }, [api]);
  useEffect(() => {
    shown.current = true;
    read();
    return () => {
      shown.current = false;
    };
  }, [read]);
  const larger =
    api.larger && model !== null && model !== "failed" && model.installed && model.supported
      ? model.larger.available
      : false;
  return (
    <>
      <div className="settings-group">
        <Row title="Dictation" detail={modelDetail(model)} />
        {larger && api.larger && model !== null && model !== "failed" ? (
          <LargerModel api={api.larger} model={model} onChanged={read} />
        ) : null}
        <Row
          title="Microphone"
          detail="Dictation hears your system's default input. Choose another in your system's sound settings."
        />
        <Row
          title="Test your microphone"
          detail="Speak a sentence to see the level and the words, without a chat."
        >
          <MicrophoneTest api={api} />
        </Row>
      </div>
      <p className="settings-footnote">
        In a chat, click the microphone in the message box. The words appear as you speak; Enter
        finishes and adds them to your draft, Esc cancels. Speech stays on this device: audio is
        never saved or uploaded, and nothing is sent until you send the message.
      </p>
      {speech && prefs ? (
        <>
          <ReadAloudSettings api={speech} prefs={prefs} />
          <p className="settings-footnote">
            Under an answer, the speaker reads it aloud and the same button stops it. Code is
            skipped, since code is hard to follow by ear. The voice runs on this device: what is
            read is not saved or uploaded.
          </p>
        </>
      ) : null}
    </>
  );
}

/** A recording that goes nowhere: the bar, the live words, then what was heard. */
export function MicrophoneTest({ api }: { readonly api: DictationApi }) {
  const [heard, setHeard] = useState<string | null>(null);
  const [controller] = useState(() => new DictationController(api, (text) => setHeard(text)));
  const state = useSyncExternalStore(
    controller.subscribe,
    controller.getSnapshot,
    controller.getSnapshot,
  );
  const finish = useRef<HTMLButtonElement>(null);
  const start = useRef<HTMLButtonElement>(null);
  const busy = dictationBusy(state);
  useEffect(() => {
    controller.activate();
    return () => controller.dispose();
  }, [controller]);
  // Esc cancels the test rather than leaving Settings; Enter finishes from the focused button.
  useEffect(() => {
    if (!busy) return;
    finish.current?.focus();
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      void controller.cancel();
    };
    window.addEventListener("keydown", onEscape, true);
    return () => window.removeEventListener("keydown", onEscape, true);
  }, [busy, controller]);
  const wasBusy = useRef(busy);
  useEffect(() => {
    if (wasBusy.current && !busy) {
      const focused = document.activeElement;
      if (!focused || focused === document.body) start.current?.focus();
    }
    wasBusy.current = busy;
  }, [busy]);
  const recording = state.state === "listening" || state.state === "finishing" ? state : null;
  const words = liveWords(liveTextOf(state));
  return (
    <div
      className="settings-mic-test"
      data-open={busy || heard !== null || state.state === "failed" ? "" : undefined}
    >
      {busy ? (
        <div className="dictation-bar settings-mic-bar" data-state={state.state}>
          <span role="status" className="sr-only">
            {state.state === "listening"
              ? "Listening"
              : state.state === "finishing"
                ? "Finishing"
                : "Starting the test"}
          </span>
          <button
            type="button"
            className="chat-composer-attach"
            aria-label="Cancel the test"
            title="Cancel (Esc)"
            disabled={state.state === "cancelling"}
            onClick={() => void controller.cancel()}
          >
            <X size={16} />
          </button>
          <Wave
            wave={recording?.wave ?? []}
            tick={recording?.tick ?? -1}
            quiet={state.state !== "listening"}
          />
          <span className="dictation-time">{recordingTime(recording?.seconds ?? 0)}</span>
          <button
            ref={finish}
            type="button"
            className="g-button g-button-primary g-button-icon chat-send"
            aria-label="Finish the test"
            title="Finish (Enter)"
            aria-disabled={state.state !== "listening" || undefined}
            onClick={() => {
              if (state.state === "listening") void controller.stop();
            }}
          >
            <Check size={16} strokeWidth={2.25} />
          </button>
        </div>
      ) : (
        <button
          ref={start}
          type="button"
          className="g-button g-button-outline g-button-28"
          onClick={() => {
            setHeard(null);
            void controller.open();
          }}
        >
          {heard !== null || state.state === "failed" ? "Test again" : "Test"}
        </button>
      )}
      {busy && words.length ? (
        <p className="settings-mic-words" aria-hidden="true">
          {words.map((word, index) => (
            <span
              // biome-ignore lint/suspicious/noArrayIndexKey: a word is its place and its text; it keeps its node while both stay.
              key={`${index}:${word.text}`}
              className="dictation-word"
              data-kind={word.kind}
            >
              {index ? " " : ""}
              {word.text}
            </span>
          ))}
        </p>
      ) : null}
      {!busy && heard !== null ? (
        <p className="settings-mic-words" role="status">
          Heard: {heard}
        </p>
      ) : null}
      {state.state === "failed" ? (
        <p className="settings-mic-problem" role="alert">
          {state.message}
        </p>
      ) : null}
    </div>
  );
}
