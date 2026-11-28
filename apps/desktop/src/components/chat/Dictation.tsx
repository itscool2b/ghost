import { prefersReducedMotion } from "@ghost/ui";
import { Check, Mic, X } from "lucide-react";
import {
  type CSSProperties,
  type RefObject,
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { flushSync } from "react-dom";
import {
  type DictationApi,
  DictationController,
  type DictationSource,
  type DictationState,
  dictationBusy,
  dictationFrame,
  dictationSpacing,
  KEPT_MESSAGE,
  type LiveWord,
  liveTextOf,
  liveWords,
  offersRetry,
  type Placement,
  placeDictation,
  recordingTime,
  sameDictationView,
  WAVE_TICK_MS,
  type WaveSample,
} from "../../lib/dictation";
import "./dictation.css";

/**
 * What the composer draws over its message box while dictation runs. The draft keeps its own
 * text; the ghost repeats it with the words heard so far at the cursor, lighter while they can
 * still change. Finishing sets them in ink with one sweep; cancelling dissolves them.
 */
export type Ghost =
  | {
      readonly mode: "live" | "finishing" | "dissolve";
      readonly key: string;
      readonly before: string;
      readonly after: string;
      readonly words: readonly LiveWord[];
    }
  | {
      readonly mode: "set";
      readonly key: string;
      readonly before: string;
      readonly inserted: string;
      readonly after: string;
    };

type Exit =
  | { readonly kind: "set"; readonly serial: number; readonly placed: Placement }
  | { readonly kind: "dissolve"; readonly serial: number; readonly words: readonly LiveWord[] };

export interface DictationView {
  /** The recording's stage and words; the level and the time are read from `source`. */
  readonly state: DictationState;
  /** Every microphone report, for the waveform and the time. */
  readonly source: DictationSource;
  /** Starting, listening, finishing or stopping: the draft is read-only and Send waits. */
  readonly busy: boolean;
  /** The bar stands in for the composer's controls. */
  readonly open: boolean;
  readonly ghost: Ghost | null;
  /** Changes whenever the ghost's text or mode does, so the composer refits its height. */
  readonly layout: string;
  begin(): void;
  finish(): void;
  cancel(): void;
  dismiss(): void;
  /** Typing in the draft: ends a finishing animation or a failure's message at once. */
  interrupt(): void;
  /** The ghost's closing animation ended. */
  settled(): void;
}

const IDLE: DictationState = { state: "idle" };
const unsubscribed = () => () => {};

/** The longest wait for a closing animation that never reports its end. */
const EXIT_TIMEOUT = 1500;

export function useDictation({
  api,
  input,
  text,
  setText,
  disabled,
}: {
  readonly api: DictationApi | undefined;
  readonly input: RefObject<HTMLTextAreaElement | null>;
  readonly text: string;
  readonly setText: (value: (current: string) => string) => void;
  readonly disabled: boolean;
}): DictationView | null {
  // The draft and its selection when dictation began: the words go there.
  const anchor = useRef({ text, start: text.length, end: text.length });
  const latest = useRef({ text, setText, input });
  latest.current = { text, setText, input };
  const caret = useRef<number | null>(null);
  const serial = useRef(0);
  const [exit, setExit] = useState<Exit | null>(null);
  const [controller] = useState(() =>
    api
      ? new DictationController(api, (transcript) => {
          const { text: original, start, end } = anchor.current;
          const placed = placeDictation(latest.current.text, transcript, original, start, end);
          caret.current = placed.end;
          // The words and their sweep arrive in one frame, so the ghost never blinks out.
          flushSync(() => {
            latest.current.setText(
              (current) => placeDictation(current, transcript, original, start, end).text,
            );
            setExit(
              prefersReducedMotion() ? null : { kind: "set", serial: ++serial.current, placed },
            );
          });
        })
      : null,
  );
  // The composer renders again only when the message box would change (`sameDictationView`): the
  // level, the waveform and the time arrive 12 times a second and are drawn by the bar alone.
  const shown = useRef<DictationState>(IDLE);
  const view = useCallback(() => {
    const next = controller?.getSnapshot() ?? IDLE;
    if (!sameDictationView(shown.current, next)) shown.current = next;
    return shown.current;
  }, [controller]);
  const state = useSyncExternalStore(controller?.subscribe ?? unsubscribed, view, view);
  const busy = dictationBusy(state);
  const open = state.state !== "idle";
  const settled = useCallback(() => setExit(null), []);

  const cancel = useCallback(() => {
    if (!controller) return;
    const words = liveWords(liveTextOf(controller.getSnapshot()));
    if (words.length && !prefersReducedMotion())
      setExit({ kind: "dissolve", serial: ++serial.current, words });
    void controller.cancel();
  }, [controller]);

  useEffect(() => {
    if (!controller) return;
    controller.activate();
    return () => {
      controller.dispose();
    };
  }, [controller]);
  useEffect(() => {
    if (disabled && controller && dictationBusy(controller.getSnapshot())) cancel();
  }, [controller, disabled, cancel]);
  useEffect(() => {
    if (!controller) return;
    const hide = () => {
      if (document.hidden && dictationBusy(controller.getSnapshot())) cancel();
    };
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const current = controller.getSnapshot();
      if (current.state === "idle") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      if (current.state === "failed") controller.dismiss();
      else cancel();
    };
    document.addEventListener("visibilitychange", hide);
    window.addEventListener("keydown", onEscape, true);
    return () => {
      document.removeEventListener("visibilitychange", hide);
      window.removeEventListener("keydown", onEscape, true);
    };
  }, [controller, cancel]);

  // The bar closing would leave focus on nothing: the draft takes it back, the caret after the
  // words it just took, or where it was before a cancelled recording. Only on closing.
  const wasOpen = useRef(open);
  useEffect(() => {
    const closed = wasOpen.current && !open;
    wasOpen.current = open;
    if (!closed) return;
    const field = latest.current.input.current;
    const focused = document.activeElement;
    if (!field || (focused && focused !== document.body && focused !== field)) return;
    field.focus({ preventScroll: true });
    const at = caret.current;
    if (at === null && field.value === anchor.current.text)
      field.setSelectionRange(anchor.current.start, anchor.current.end);
    else if (at !== null) field.setSelectionRange(at, at);
    caret.current = null;
  }, [open]);

  if (!controller) return null;

  const frame = dictationFrame(text, anchor.current.text, anchor.current.start, anchor.current.end);
  let ghost: Ghost | null = null;
  if (state.state === "starting" || state.state === "listening" || state.state === "finishing")
    ghost = {
      mode: state.state === "finishing" ? "finishing" : "live",
      key: "live",
      ...frame,
      words: liveWords(liveTextOf(state)),
    };
  else if (exit?.kind === "dissolve")
    ghost = { mode: "dissolve", key: `exit-${exit.serial}`, ...frame, words: exit.words };
  else if (exit?.kind === "set" && exit.placed.text === text)
    ghost = {
      mode: "set",
      key: `exit-${exit.serial}`,
      before: text.slice(0, exit.placed.start),
      inserted: text.slice(exit.placed.start, exit.placed.end),
      after: text.slice(exit.placed.end),
    };
  const layout = ghost
    ? `${ghost.key}|${ghost.mode}|${
        ghost.mode === "set" ? ghost.inserted : ghost.words.map((word) => word.text).join(" ")
      }`
    : "";

  return {
    state,
    source: controller,
    busy,
    open,
    ghost,
    layout,
    begin: () => {
      const field = latest.current.input.current;
      const draft = latest.current.text;
      anchor.current = {
        text: draft,
        start: field?.selectionStart ?? draft.length,
        end: field?.selectionEnd ?? draft.length,
      };
      caret.current = null;
      setExit(null);
      void controller.open();
    },
    finish: () => void controller.stop(),
    cancel,
    dismiss: () => controller.dismiss(),
    interrupt: () => {
      setExit(null);
      controller.dismiss();
    },
    settled,
  };
}

/** The microphone in the composer's controls. */
export function DictationButton({
  dictation,
  disabled,
}: {
  readonly dictation: DictationView;
  readonly disabled: boolean;
}) {
  return (
    <button
      type="button"
      className="chat-composer-attach"
      aria-label="Dictate a message"
      title="Dictate a message. Speech stays on this device."
      disabled={disabled || dictation.open}
      onClick={dictation.begin}
    >
      <Mic size={16} />
    </button>
  );
}

/**
 * The dictation bar, in place of the composer's controls: cancel, the live waveform, the time and
 * Finish; a failure's message and its recovery take the waveform's place.
 */
export function DictationBar({ dictation }: { readonly dictation: DictationView }) {
  const { state } = dictation;
  const finish = useRef<HTMLButtonElement>(null);
  const close = useRef<HTMLButtonElement>(null);
  const recover = useRef<HTMLButtonElement>(null);
  const problem =
    state.state === "failed"
      ? "failed"
      : state.state === "cancelling" && state.message
        ? "stuck"
        : null;
  // Enter or Space finishes: Finish holds focus while the microphone is open. A problem hands
  // focus to its recovery, or to Close.
  useEffect(() => {
    if (problem) (recover.current ?? close.current)?.focus();
    else finish.current?.focus();
  }, [problem]);

  if (state.state === "failed" || (state.state === "cancelling" && state.message)) {
    const retry = state.state === "failed" ? offersRetry(state) : true;
    return (
      // biome-ignore lint/a11y/useSemanticElements: a fieldset's anonymous content box breaks the bar's single flex row.
      <div
        className="dictation-bar"
        role="group"
        aria-label="Dictation"
        data-testid="dictation-bar"
        data-state="problem"
      >
        <button
          ref={close}
          type="button"
          className="chat-composer-attach"
          aria-label="Close dictation"
          title="Close (Esc)"
          disabled={state.state === "cancelling"}
          onClick={dictation.dismiss}
        >
          <X size={16} />
        </button>
        <p role="alert" className="dictation-problem">
          {state.message}
          {state.state === "failed" && state.kept ? ` ${KEPT_MESSAGE}` : ""}
        </p>
        {retry ? (
          <button
            ref={recover}
            type="button"
            className="g-button g-button-28"
            onClick={state.state === "failed" ? dictation.begin : dictation.cancel}
          >
            {state.state === "failed" ? "Try again" : "Try stopping again"}
          </button>
        ) : null}
      </div>
    );
  }
  const waiting = state.state !== "listening";
  return (
    // biome-ignore lint/a11y/useSemanticElements: a fieldset's anonymous content box breaks the bar's single flex row.
    <div
      className="dictation-bar"
      role="group"
      aria-label="Dictation"
      data-testid="dictation-bar"
      data-state={state.state}
    >
      <span role="status" className="sr-only">
        {state.state === "starting"
          ? "Starting dictation"
          : state.state === "listening"
            ? "Listening"
            : state.state === "finishing"
              ? "Finishing"
              : "Cancelling dictation"}
      </span>
      <button
        type="button"
        className="chat-composer-attach"
        aria-label="Cancel dictation"
        title="Cancel and discard what was heard (Esc)"
        disabled={state.state === "cancelling"}
        onClick={dictation.cancel}
      >
        <X size={16} />
      </button>
      <Wave source={dictation.source} quiet={state.state !== "listening"} />
      <RecordingTime source={dictation.source} live={state.state === "listening"} />
      <button
        ref={finish}
        type="button"
        className="g-button g-button-primary g-button-icon chat-send"
        aria-label="Finish dictation"
        title="Finish and add the words to your draft (Enter)"
        aria-disabled={waiting || undefined}
        onClick={() => {
          if (!waiting) dictation.finish();
        }}
      >
        <Check size={16} strokeWidth={2.25} />
      </button>
    </div>
  );
}

const recordingOf = (value: DictationState) =>
  value.state === "listening" || value.state === "finishing" ? value : null;

/**
 * How long the recording has run, after a dot that breathes while the microphone is open. It
 * renders once a second, whatever the microphone reports in between.
 */
export function RecordingTime({
  source,
  live,
}: {
  readonly source: DictationSource;
  readonly live: boolean;
}) {
  const seconds = useSyncExternalStore(
    source.subscribe,
    () => recordingOf(source.getSnapshot())?.seconds ?? 0,
    () => 0,
  );
  return (
    <span className="dictation-time" data-live={live ? "" : undefined}>
      {recordingTime(seconds)}
    </span>
  );
}

/** The waveform's geometry, in CSS pixels: one bar per report, 2px wide, 2px apart. */
const BAR = 2;
const PITCH = 4;

/**
 * The microphone's level over the last seconds, newest at the right: a bar per report, centred on
 * the line, in ink where voice detection heard speech. It is drawn on a canvas straight from the
 * recording, never through React, and glides one bar to the left over each report's 80 ms; under
 * reduced motion it steps instead.
 */
export function Wave({
  source,
  quiet,
}: {
  readonly source: DictationSource;
  readonly quiet: boolean;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const meter = useRef<HTMLMeterElement>(null);
  useEffect(() => {
    const node = canvas.current;
    const context = node?.getContext("2d");
    if (!node || !context) return;
    const still = prefersReducedMotion();
    let samples: readonly WaveSample[] = [];
    let tick = -1;
    let arrived = performance.now();
    let width = 0;
    let height = 0;
    let ratio = 1;
    let quietInk = "";
    let voicedInk = "";
    // The last frame drawn at rest: nothing moves until the next report.
    let settled = false;
    const colours = () => {
      const style = getComputedStyle(node);
      quietInk = style.color;
      voicedInk = style.getPropertyValue("--dictation-voiced").trim() || quietInk;
    };
    const draw = (progress: number) => {
      if (!width || !height) return;
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, width, height);
      const shift = (1 - progress) * PITCH;
      const count = samples.length;
      for (let index = count - 1; index >= 0; index--) {
        const x = width - (count - index) * PITCH + shift + (PITCH - BAR);
        if (x + BAR < 0) break;
        const sample = samples[index];
        if (!sample) continue;
        const tall = Math.max(BAR, Math.round(sample.level * (height - 2)));
        context.fillStyle = sample.voiced ? voicedInk : quietInk;
        const y = (height - tall) / 2;
        if (context.roundRect) {
          context.beginPath();
          context.roundRect(x, y, BAR, tall, BAR / 2);
          context.fill();
        } else context.fillRect(x, y, BAR, tall);
      }
    };
    const read = () => {
      const recording = recordingOf(source.getSnapshot());
      if (!recording || recording.tick === tick) return;
      samples = recording.wave;
      tick = recording.tick;
      arrived = performance.now();
      settled = false;
      colours();
      if (meter.current) meter.current.value = Math.round((samples.at(-1)?.level ?? 0) * 100) / 100;
      if (still) draw(1);
    };
    const resize = () => {
      ratio = window.devicePixelRatio || 1;
      width = node.clientWidth;
      height = node.clientHeight;
      node.width = Math.round(width * ratio);
      node.height = Math.round(height * ratio);
      colours();
      draw(1);
    };
    const observer = new ResizeObserver(resize);
    observer.observe(node);
    const stop = source.subscribe(read);
    read();
    resize();
    let frame = 0;
    const loop = (now: number) => {
      if (!settled) {
        const progress = Math.min(1, (now - arrived) / WAVE_TICK_MS);
        draw(progress);
        settled = progress === 1;
      }
      frame = requestAnimationFrame(loop);
    };
    if (!still) frame = requestAnimationFrame(loop);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      stop();
    };
  }, [source]);
  return (
    <div className="dictation-wave" data-quiet={quiet ? "" : undefined}>
      <meter
        ref={meter}
        className="sr-only"
        aria-label="Microphone level"
        min={0}
        max={1}
      />
      {/* No fallback content: the meter above is what assistive technology reads. */}
      <canvas ref={canvas} className="dictation-wave-canvas" />
    </div>
  );
}

/**
 * The ghost over the message box. It sets the draft's own text with the same metrics, so the
 * words heard so far sit exactly where they will land.
 */
export function DictationGhost({
  ghost,
  node,
  onSettled,
}: {
  readonly ghost: Ghost;
  readonly node: RefObject<HTMLDivElement | null>;
  readonly onSettled: () => void;
}) {
  const closing = ghost.mode === "set" || ghost.mode === "dissolve";
  // Should a closing animation never report its end, the draft shows through anyway.
  useEffect(() => {
    if (!closing) return;
    const timer = window.setTimeout(onSettled, EXIT_TIMEOUT);
    return () => window.clearTimeout(timer);
  }, [closing, onSettled]);
  // A textarea shows a line after a final newline; a block does only with something on it.
  const tail = (ghost.after || (ghost.mode === "set" ? ghost.inserted : "")).endsWith("\n")
    ? "​"
    : "";
  if (ghost.mode === "set")
    return (
      <div
        ref={node}
        className="dictation-ghost"
        data-mode="set"
        aria-hidden="true"
        onAnimationEnd={(event) => {
          if (event.animationName === "dictation-ink") onSettled();
        }}
      >
        {ghost.before}
        <span className="dictation-set">{ghost.inserted}</span>
        {ghost.after}
        {tail}
      </div>
    );
  const { left, right } = dictationSpacing(ghost);
  const count = ghost.words.length;
  return (
    <div
      ref={node}
      className="dictation-ghost"
      data-mode={ghost.mode}
      aria-hidden="true"
      onAnimationEnd={(event) => {
        if (
          event.animationName === "dictation-dissolve" &&
          event.target instanceof HTMLElement &&
          event.target.dataset.final !== undefined
        )
          onSettled();
      }}
    >
      {ghost.before}
      {count ? left : null}
      <span className="dictation-words">
        {ghost.words.map((word, index) => (
          <span
            // biome-ignore lint/suspicious/noArrayIndexKey: a word is its place and its text; it keeps its node while both stay, so only new or changed words fade in.
            key={`${index}:${word.text}`}
            className="dictation-word"
            data-kind={word.kind}
            data-final={ghost.mode === "dissolve" && index === 0 ? "" : undefined}
            style={
              ghost.mode === "dissolve"
                ? ({ "--from-end": Math.min(16, count - 1 - index) } as CSSProperties)
                : undefined
            }
          >
            {index ? " " : ""}
            {word.text}
          </span>
        ))}
      </span>
      {ghost.mode === "live" ? <span className="dictation-caret" /> : null}
      {ghost.mode === "live" && !count && !ghost.before && !ghost.after ? (
        <span className="dictation-hint">Listening</span>
      ) : null}
      {count ? right : null}
      {ghost.after}
      {tail}
    </div>
  );
}

/**
 * Keeps the newest words (or, as they set, the inserted ones) in view, and the message box
 * scrolled with the ghost so the two agree when it leaves.
 */
export function revealDictation(node: HTMLElement, field: HTMLTextAreaElement) {
  const mark = node.querySelector<HTMLElement>(
    ".dictation-caret, .dictation-set, .dictation-words",
  );
  const rects = mark?.getClientRects();
  const last = rects?.[rects.length - 1];
  if (!last) return;
  const box = node.getBoundingClientRect();
  const style = getComputedStyle(node);
  const top = last.top - box.top + node.scrollTop - Number.parseFloat(style.paddingTop);
  const bottom = last.bottom - box.top + node.scrollTop + Number.parseFloat(style.paddingBottom);
  if (bottom > node.scrollTop + node.clientHeight) node.scrollTop = bottom - node.clientHeight;
  else if (top < node.scrollTop) node.scrollTop = Math.max(0, top);
  if (node.dataset.mode === "set") field.scrollTop = node.scrollTop;
}
