import { Channel } from "@tauri-apps/api/core";
import {
  commands,
  type DictationEvent,
  type DictationFailure,
  type DictationModel,
  type LargerEvent,
  type LiveText,
} from "../bindings";
import { errorMessage } from "./errors";
import { settle } from "./ipc";

/**
 * Dictation in the composer (docs/local-dictation.md). The bundled models run on this device: the
 * microphone streams live text until the person finishes or cancels, with no time limit, and only
 * the finished text joins the draft. Nothing here can send a message.
 */
export interface DictationApi {
  model(): Promise<DictationModel>;
  start(id: string, event: (event: DictationEvent) => void): Promise<void>;
  stop(id: string): Promise<void>;
  cancel(id: string): Promise<void>;
  /** The larger speech model's download; a view without it offers the built-in model alone. */
  readonly larger?: LargerApi;
}

/**
 * The larger speech model (docs/local-dictation.md, "The larger model"): fetched once from its
 * pinned address, by Ghost itself shortly after it starts or when the person asks. The page supplies
 * nothing but the wish.
 */
export interface LargerApi {
  /**
   * Starts the download; `event` hears it arrive and how it ended. It also takes back an earlier
   * Cancel or Remove, so Ghost finishes the download by itself should this attempt not.
   */
  download(event: (event: LargerEvent) => void): Promise<void>;
  /**
   * Stops it, or says no before Ghost started it. What arrived is kept, and the next download
   * continues from it; Ghost does not start one by itself again.
   */
  cancel(): Promise<void>;
  /** Deletes it. Ghost does not fetch it by itself again. */
  remove(): Promise<void>;
}

export const dictationApi: DictationApi = {
  model: () => commands.dictationModel(),
  start: async (id, onmessage) => {
    await settle(commands.dictationStart(id, new Channel(onmessage)));
  },
  stop: (id) => commands.dictationStop(id),
  cancel: (id) => commands.dictationCancel(id),
  larger: {
    download: async (onmessage) => {
      await settle(commands.dictationLargerDownload(new Channel(onmessage)));
    },
    cancel: async () => {
      await settle(commands.dictationLargerCancel());
    },
    remove: async () => {
      await settle(commands.dictationLargerRemove());
    },
  },
};

/** Where the larger model's row is, between what the core reports. */
export type LargerPhase =
  /** `kept`: a download was stopped and what arrived waits for the next one. */
  | { readonly state: "idle"; readonly kept?: boolean }
  | { readonly state: "downloading"; readonly received: number; readonly total: number }
  | { readonly state: "verifying" }
  | { readonly state: "removing" }
  | { readonly state: "failed"; readonly message: string };

const megabytes = (bytes: number): number => Math.round(bytes / 1_000_000);

/** What the larger model's row says: what it is and costs, how far it has come, or that it is in use. */
export function largerDetail(model: DictationModel, phase: LargerPhase): string {
  const { name, bytes, state } = model.larger;
  const size = `${megabytes(bytes)} MB`;
  if (phase.state === "downloading")
    return `Downloading ${name}: ${megabytes(phase.received)} of ${megabytes(phase.total)} MB.`;
  if (phase.state === "verifying") return "Checking the download.";
  if (phase.state === "removing") return `Removing ${name}.`;
  if (state === "ready")
    return `${name} is downloaded and in use. Removing it returns dictation to the built-in model and frees ${size}.`;
  // A download this page did not start, Ghost's own included: the core says how far it has come.
  if (state === "downloading")
    return `Downloading ${name}: ${megabytes(model.larger.received)} of ${megabytes(bytes)} MB.`;
  const what = `${name} makes about 40% fewer mistakes, gets programming words right and hears 24 more European languages.`;
  // Ghost fetches it by itself unless the person said no: the row says so before it starts.
  if (model.larger.automatic && phase.state === "idle" && !phase.kept)
    return `${what} Ghost downloads it by itself, once: ${size} from Hugging Face. It runs on this device.`;
  const offer = `${what} A one-time ${size} download from Hugging Face; it runs on this device.`;
  return phase.state === "idle" && phase.kept ? `${offer} What arrived so far is kept.` : offer;
}

/** What Rust reports for a processor without AVX2; the bar says it before any command runs. */
export const UNSUPPORTED_MESSAGE =
  "Dictation needs a processor with AVX2 (Intel from 2013 or AMD from 2015 onward). The rest of Ghost works as usual.";
export const MISSING_MESSAGE =
  "This build of Ghost is missing its dictation models. Reinstall Ghost to restore them.";
/** Said after a failure when the words heard before it were kept in the draft. */
export const KEPT_MESSAGE = "What it heard so far is in your draft.";

export const NO_TEXT: LiveText = { settled: "", stable: "", tentative: "" };

/** Samples of microphone level the waveform keeps: 160 ticks of 80 ms, about 13 seconds. */
export const WAVE_SAMPLES = 160;

export interface WaveSample {
  /** Shown height, 0 to 1. */
  readonly level: number;
  /** Voice detection heard speech at this tick. */
  readonly voiced: boolean;
}

export type DictationState =
  | { readonly state: "idle" }
  /** Checking the models, loading them the first time, and opening the microphone. */
  | { readonly state: "starting" }
  | {
      readonly state: "listening";
      readonly seconds: number;
      readonly speaking: boolean;
      readonly text: LiveText;
      readonly wave: readonly WaveSample[];
      /** Counts the microphone's reports, so each waveform sample keeps its identity. */
      readonly tick: number;
    }
  | {
      readonly state: "finishing";
      readonly seconds: number;
      readonly text: LiveText;
      readonly wave: readonly WaveSample[];
      readonly tick: number;
    }
  | { readonly state: "cancelling"; readonly text: LiveText; readonly message?: string }
  | {
      readonly state: "failed";
      readonly message: string;
      readonly reason: DictationFailure;
      /** The words heard before the failure went into the draft. */
      readonly kept: boolean;
    };

/** Retrying cannot help a processor that lacks the engine's instructions. */
export function offersRetry(value: DictationState): boolean {
  return value.state === "failed" && value.reason !== "unsupported";
}

/** A recording is starting, running, finishing or stopping: the draft is the dictation's. */
export function dictationBusy(value: DictationState): boolean {
  return ["starting", "listening", "finishing", "cancelling"].includes(value.state);
}

/** The live text the state carries, if any. */
export function liveTextOf(value: DictationState): LiveText {
  return value.state === "listening" || value.state === "finishing" || value.state === "cancelling"
    ? value.text
    : NO_TEXT;
}

export type WordKind = "settled" | "stable" | "tentative";
export interface LiveWord {
  readonly text: string;
  readonly kind: WordKind;
}

/** The live text as words in reading order, each marked with how final it is. */
export function liveWords(text: LiveText): LiveWord[] {
  const words: LiveWord[] = [];
  for (const kind of ["settled", "stable", "tentative"] as const) {
    for (const word of text[kind].split(/\s+/)) if (word) words.push({ text: word, kind });
  }
  return words;
}

/** The live text as one string, as the draft would take it. */
export function joinLive(text: LiveText): string {
  return liveWords(text)
    .map((word) => word.text)
    .join(" ");
}

/**
 * A microphone level (Rust reports 0 to 1, about eight times the RMS) as a bar height: room noise
 * stays flat and speech fills most of the bar.
 */
export function waveLevel(level: number | null): number {
  if (level === null || !Number.isFinite(level)) return 0;
  return Math.min(1, Math.sqrt(Math.max(0, level - 0.03) / 0.97));
}

/** The time a recording has run, as `m:ss`. */
export function recordingTime(seconds: number): string {
  const whole = Math.max(0, Math.floor(seconds));
  return `${Math.floor(whole / 60)}:${String(whole % 60).padStart(2, "0")}`;
}

/** Where dictation goes in the draft: the text around the captured selection. */
export interface DictationFrame {
  readonly before: string;
  readonly after: string;
}

/**
 * The draft around the selection captured when dictation began. If the draft changed since, the
 * words go at its end, so nothing typed or restored in the meantime is overwritten.
 */
export function dictationFrame(
  current: string,
  original: string,
  start: number,
  end: number,
): DictationFrame {
  return current === original
    ? { before: current.slice(0, start), after: current.slice(end) }
    : { before: current, after: "" };
}

/** The spaces that keep dictated words apart from the draft around them. */
export function dictationSpacing(frame: DictationFrame): { left: string; right: string } {
  return {
    left: frame.before && !/\s$/.test(frame.before) ? " " : "",
    right: frame.after && !/^\s/.test(frame.after) ? " " : "",
  };
}

export interface Placement {
  readonly text: string;
  /** The dictated words' range in `text`, without the spaces around them. */
  readonly start: number;
  readonly end: number;
}

/** The draft with `transcript` in place, and where it went. An empty transcript changes nothing. */
export function placeDictation(
  current: string,
  transcript: string,
  original: string,
  start: number,
  end: number,
): Placement {
  const words = transcript.trim();
  if (!words) return { text: current, start: current.length, end: current.length };
  const frame = dictationFrame(current, original, start, end);
  const { left, right } = dictationSpacing(frame);
  const from = frame.before.length + left.length;
  return {
    text: `${frame.before}${left}${words}${right}${frame.after}`,
    start: from,
    end: from + words.length,
  };
}

/** Owns one recording. Late events and invoke completions cannot edit another chat. */
export class DictationController {
  #state: DictationState = { state: "idle" };
  #listeners = new Set<() => void>();
  #active: string | null = null;
  #cancelled: string | null = null;
  #stopping: string | null = null;
  #disposed = false;
  #generation = 0;
  constructor(
    readonly api: DictationApi,
    /** Puts finished words into the draft. */
    readonly insert: (text: string) => void,
  ) {}
  readonly subscribe = (listener: () => void) => {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  };
  readonly getSnapshot = () => this.#state;
  #set(value: DictationState) {
    if (this.#disposed) return;
    this.#state = value;
    for (const listener of this.#listeners) listener();
  }
  /** Checks the bundled models and starts listening. */
  async open() {
    if (this.#disposed || this.#active || dictationBusy(this.#state)) return;
    const generation = ++this.#generation;
    this.#set({ state: "starting" });
    try {
      const model = await this.api.model();
      if (this.#disposed || generation !== this.#generation) return;
      if (!model.supported)
        this.#set({
          state: "failed",
          message: UNSUPPORTED_MESSAGE,
          reason: "unsupported",
          kept: false,
        });
      else if (!model.installed)
        this.#set({ state: "failed", message: MISSING_MESSAGE, reason: "model", kept: false });
      else await this.#start();
    } catch (error) {
      if (generation === this.#generation)
        this.#set({ state: "failed", message: errorMessage(error), reason: "other", kept: false });
    }
  }
  async #start() {
    if (this.#active || this.#disposed) return;
    const id = crypto.randomUUID();
    this.#active = id;
    this.#cancelled = null;
    this.#stopping = null;
    this.#set({ state: "starting" });
    try {
      await this.api.start(id, (event) => this.#receive(id, event));
      // The person can cancel, or the chat unmount, while the command is still being admitted.
      if (this.#disposed || this.#active !== id) await this.api.cancel(id);
      else if (this.#cancelled === id) await this.cancel();
    } catch (error) {
      if (this.#active !== id) return;
      this.#active = null;
      this.#set(
        this.#cancelled === id
          ? { state: "idle" }
          : { state: "failed", message: errorMessage(error), reason: "other", kept: false },
      );
    }
  }
  #receive(id: string, event: DictationEvent) {
    if (this.#disposed || this.#active !== id) return;
    const cancelled = this.#cancelled === id;
    const previous = this.#state;
    const recorded =
      previous.state === "listening" || previous.state === "finishing" ? previous : null;
    switch (event.state) {
      case "preparing":
        if (!cancelled) this.#set({ state: "starting" });
        return;
      case "listening": {
        if (cancelled) return;
        const sample = { level: waveLevel(event.level), voiced: event.speaking };
        const wave = [...(recorded?.wave ?? []).slice(1 - WAVE_SAMPLES), sample];
        const tick = (recorded?.tick ?? -1) + 1;
        // Finishing was asked for: the microphone's last report only updates the words.
        if (this.#stopping === id)
          this.#set({ state: "finishing", seconds: event.seconds, text: event.text, wave, tick });
        else
          this.#set({
            state: "listening",
            seconds: event.seconds,
            speaking: event.speaking,
            text: event.text,
            wave,
            tick,
          });
        return;
      }
      case "finishing":
        if (cancelled) return;
        this.#set({
          state: "finishing",
          seconds: recorded?.seconds ?? 0,
          text: event.text,
          wave: recorded?.wave ?? [],
          tick: recorded?.tick ?? 0,
        });
        return;
      case "complete":
        this.#active = null;
        if (!cancelled && event.text.trim()) this.insert(event.text.trim());
        this.#set({ state: "idle" });
        return;
      case "cancelled":
        this.#active = null;
        this.#set({ state: "idle" });
        return;
      case "failed": {
        this.#active = null;
        if (cancelled) {
          this.#set({ state: "idle" });
          return;
        }
        // A microphone that stops mid-sentence keeps what was already heard.
        const heard = joinLive(liveTextOf(previous));
        if (heard) this.insert(heard);
        this.#set({
          state: "failed",
          message: event.message,
          reason: event.reason,
          kept: heard !== "",
        });
      }
    }
  }
  /** Closes the microphone; the last words settle and join the draft. */
  async stop() {
    const id = this.#active;
    const current = this.#state;
    if (!id || current.state !== "listening") return;
    this.#stopping = id;
    this.#set({
      state: "finishing",
      seconds: current.seconds,
      text: current.text,
      wave: current.wave,
      tick: current.tick,
    });
    try {
      await this.api.stop(id);
    } catch (error) {
      if (this.#active !== id) return;
      this.#set({ state: "failed", message: errorMessage(error), reason: "other", kept: false });
      await this.cancel();
    }
  }
  /** Discards the recording and its words. */
  async cancel() {
    ++this.#generation;
    const id = this.#active;
    if (!id) {
      this.#set({ state: "idle" });
      return;
    }
    this.#cancelled = id;
    this.#set({ state: "cancelling", text: liveTextOf(this.#state) });
    try {
      await this.api.cancel(id);
    } catch (error) {
      if (this.#active !== id) return;
      this.#set({
        state: "cancelling",
        text: NO_TEXT,
        message: `Could not stop dictation. ${errorMessage(error)}`,
      });
    }
  }
  /** Closes a failure's message. */
  dismiss() {
    if (this.#state.state === "failed") this.#set({ state: "idle" });
  }
  dispose() {
    void this.cancel();
    this.#disposed = true;
    this.#active = null;
    this.#listeners.clear();
  }
  activate() {
    if (this.#disposed) {
      this.#disposed = false;
      this.#set({ state: "idle" });
    }
  }
}
