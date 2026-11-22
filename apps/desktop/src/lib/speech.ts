import { Channel } from "@tauri-apps/api/core";
import {
  commands,
  type SpeechEvent,
  type SpeechFailure,
  type SpeechStatus,
  type SpeechVoice,
} from "../bindings";
import { errorMessage } from "./errors";
import { settle } from "./ipc";

/**
 * Reading aloud (docs/read-aloud.md). The voice model ships in the app and runs on this device:
 * an answer's words go to the speech engine and out of the speakers, and are neither stored nor
 * sent anywhere. One thing is read at a time; asking for another stops the first.
 */
export interface SpeechApi {
  status(): Promise<SpeechStatus>;
  speak(
    id: string,
    text: string,
    voice: string | null,
    event: (event: SpeechEvent) => void,
  ): Promise<void>;
  stop(id: string): Promise<void>;
  /** Loads the model and readies the voice ahead of a request. Says and reports nothing. */
  prepare(voice: string | null): Promise<void>;
}

export const speechApi: SpeechApi = {
  status: () => commands.speechStatus(),
  speak: async (id, text, voice, onmessage) => {
    await settle(commands.speechSpeak(id, text, voice, new Channel(onmessage)));
  },
  stop: (id) => commands.speechStop(id),
  prepare: async (voice) => {
    await settle(commands.speechPrepare(voice));
  },
};

/** The most the core reads in one request (`speech::commands::MAX_TEXT_BYTES`). */
export const MAX_SPOKEN_BYTES = 24_000;

/** What a person hears as a sample of a voice. */
export const VOICE_SAMPLE = "I fixed the login flow, and all 42 tests pass. Nothing else changed.";

const ENCODER = new TextEncoder();

/** A sentence's end, so a list item or a heading is not run into what follows. */
function closed(line: string): string {
  return /[.!?:;,]$/.test(line) ? line : `${line}.`;
}

/**
 * What is said for an answer written in Markdown: its words, without the marks. Code blocks are
 * not read out (a listener cannot follow code by ear); each is named in its place. Links say
 * their words, bare addresses say "link", tables are read a row at a time, and headings and list
 * items end as sentences.
 */
export function speakable(markdown: string): string {
  const said: string[] = [];
  let fence: string | null = null;
  for (const raw of markdown.replace(/\r\n?/g, "\n").split("\n")) {
    const mark = /^\s{0,3}(`{3,}|~{3,})/.exec(raw)?.[1] ?? null;
    if (fence !== null) {
      if (mark !== null && mark[0] === fence) fence = null;
      continue;
    }
    if (mark !== null) {
      fence = mark[0] ?? null;
      said.push("Code block.");
      continue;
    }
    let line = raw.trim();
    if (line === "") continue;
    // A table's rule row, a thematic break, and a line of nothing but marks say nothing.
    if (/^[|\s:-]+$/.test(line) || /^([-*_]\s*){3,}$/.test(line)) continue;
    const heading = /^#{1,6}\s+(.*?)\s*#*$/.exec(line);
    const item = /^(?:[-*+]|\d+[.)])\s+(?:\[[ xX]\]\s+)?(.*)$/.exec(line);
    const structural = heading !== null || item !== null || line.startsWith("|");
    if (heading?.[1] !== undefined) line = heading[1];
    else if (item?.[1] !== undefined) line = item[1];
    line = line
      .replace(/^>+\s?/, "")
      // A table row: its cells, one after another.
      .replace(/^\||\|$/g, "")
      .replace(/\s*\|\s*/g, ", ")
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
      .replace(/<https?:\/\/[^>\s]+>/g, "link")
      .replace(/https?:\/\/[^\s)]+/g, "link")
      .replace(/<\/?[a-zA-Z][^>]*>/g, "")
      .replace(/`+([^`]*)`+/g, "$1")
      .replace(/(\*\*|__|~~)(.+?)\1/g, "$2")
      // Emphasis hugs its words: "2 * 3 * 4" keeps its stars.
      .replace(/(^|[\s(])([*_])([^\s*_](?:[^*_\n]*[^\s*_])?)\2(?=$|[\s).,;:!?])/g, "$1$3")
      .replace(/\\([\\`*_{}[\]()#+\-.!|>])/g, "$1")
      .replace(/\s+/g, " ")
      .trim();
    if (line === "") continue;
    said.push(structural ? closed(line) : line);
  }
  return said.join("\n");
}

/**
 * `text` cut to what one request may carry, at the end of a sentence where there is one. `clipped`
 * says some was left unread.
 */
export function clipSpoken(
  text: string,
  limit = MAX_SPOKEN_BYTES,
): { readonly text: string; readonly clipped: boolean } {
  if (ENCODER.encode(text).length <= limit) return { text, clipped: false };
  let cut = "";
  let bytes = 0;
  // Whole characters only: a cut inside a character would not be text.
  for (const character of text) {
    const size = ENCODER.encode(character).length;
    if (bytes + size > limit) break;
    cut += character;
    bytes += size;
  }
  const end = Math.max(cut.lastIndexOf(". "), cut.lastIndexOf(".\n"), cut.lastIndexOf("\n"));
  return { text: (end > cut.length / 2 ? cut.slice(0, end + 1) : cut).trim(), clipped: true };
}

/** What Rust reports for a processor without AVX2. */
export const UNSUPPORTED_MESSAGE =
  "Reading aloud needs a processor with AVX2 (Intel from 2013 or AMD from 2015 onward). The rest of o7 works as usual.";
export const MISSING_MESSAGE =
  "This build of o7 is missing its voice model. Reinstall o7 to restore it.";

/** What the Voice page says about reading aloud on this device. */
export function speechDetail(status: SpeechStatus | null | "failed"): string {
  if (status === null) return "Checking the built-in voice model.";
  if (status === "failed") return "o7 could not read the voice model's status.";
  if (!status.supported) return UNSUPPORTED_MESSAGE;
  if (!status.installed) return MISSING_MESSAGE;
  return `${status.name}, built in and ready: English, spoken on this device.`;
}

/** The voices in the groups the picker shows them in. */
export function voiceGroups(
  voices: readonly SpeechVoice[],
): readonly { readonly label: string; readonly voices: readonly SpeechVoice[] }[] {
  const groups = [
    { label: "American", voices: voices.filter((voice) => voice.locale === "en-us") },
    { label: "British", voices: voices.filter((voice) => voice.locale === "en-gb") },
    { label: "Characters", voices: voices.filter((voice) => voice.locale === "characters") },
  ];
  const grouped = new Set(groups.flatMap((group) => group.voices));
  const rest = voices.filter((voice) => !grouped.has(voice));
  return [...groups, { label: "Other", voices: rest }].filter((group) => group.voices.length > 0);
}

/** The voice a saved choice means here: itself when this build has it, else the model's own. */
export function chosenVoice(
  voices: readonly SpeechVoice[],
  saved: string | null | undefined,
): SpeechVoice | null {
  return voices.find((voice) => voice.id === saved) ?? voices[0] ?? null;
}

export type SpeechPhase = "idle" | "preparing" | "speaking";

export interface SpeechSnapshot {
  /** What is being read: the key its button gave. */
  readonly key: string | null;
  readonly phase: SpeechPhase;
  /** Why the last request of `failedKey` failed, until something else is read. */
  readonly error: string | null;
  readonly failedKey: string | null;
  readonly failure: SpeechFailure | null;
  /** Whether reading aloud can run here; `null` until the core has said. */
  readonly available: boolean | null;
}

const IDLE: SpeechSnapshot = {
  key: null,
  phase: "idle",
  error: null,
  failedKey: null,
  failure: null,
  available: null,
};

/**
 * The one voice of the window: which button is being read, and whether it is getting ready or
 * speaking. Every read-aloud button and the Voice page's sample share it, so starting one stops
 * another.
 */
export class SpeechPlayer {
  #api: SpeechApi;
  #state: SpeechSnapshot = IDLE;
  #listeners = new Set<() => void>();
  #request: string | null = null;
  #voice: string | null = null;
  #asked = false;
  #prepared: string | null | undefined = undefined;

  constructor(api: SpeechApi) {
    this.#api = api;
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    this.#ask();
    return () => {
      this.#listeners.delete(listener);
    };
  };

  readonly getSnapshot = (): SpeechSnapshot => this.#state;

  /** Another engine, for a preview or a test: what was known of the last one is forgotten. */
  use(api: SpeechApi): void {
    this.#api = api;
    this.#request = null;
    this.#asked = false;
    this.#prepared = undefined;
    this.#set(IDLE);
    if (this.#listeners.size > 0) this.#ask();
  }

  /** The saved voice, from the profile's preferences; `null` is the model's own. */
  setVoice(voice: string | null): void {
    this.#voice = voice;
  }

  #set(next: SpeechSnapshot): void {
    this.#state = next;
    for (const listener of this.#listeners) listener();
  }

  /** Asks once whether reading aloud can run here, so the buttons know whether to show. */
  #ask(): void {
    if (this.#asked) return;
    this.#asked = true;
    this.#api.status().then(
      (status) => this.#set({ ...this.#state, available: status.installed && status.supported }),
      () => this.#set({ ...this.#state, available: false }),
    );
  }

  /** Readies the saved voice, when the pointer reaches a button: the click then speaks at once. */
  prepare(): void {
    if (this.#state.available !== true || this.#prepared === this.#voice) return;
    this.#prepared = this.#voice;
    void this.#api.prepare(this.#voice).catch(() => undefined);
  }

  /** Reads `markdown` for `key`, or stops when `key` is what is being read. */
  toggle(key: string, markdown: string, voice: string | null = this.#voice): void {
    if (this.#state.key === key && this.#state.phase !== "idle") {
      this.stop();
      return;
    }
    void this.speak(key, speakable(markdown), voice);
  }

  /** Reads `text` as it is, in `voice`. */
  async speak(key: string, text: string, voice: string | null = this.#voice): Promise<void> {
    const previous = this.#request;
    const id = crypto.randomUUID();
    this.#request = id;
    this.#set({
      ...this.#state,
      key,
      phase: "preparing",
      error: null,
      failedKey: null,
      failure: null,
    });
    // The core stops the earlier request by itself; saying so first frees its button at once.
    if (previous !== null) void this.#api.stop(previous).catch(() => undefined);
    const fail = (message: string, failure: SpeechFailure | null) => {
      if (this.#request !== id) return;
      this.#request = null;
      this.#set({
        ...this.#state,
        key: null,
        phase: "idle",
        error: message,
        failedKey: key,
        failure,
      });
    };
    const spoken = clipSpoken(text).text;
    if (spoken.trim() === "") {
      fail("There is nothing here to read aloud.", null);
      return;
    }
    try {
      await this.#api.speak(id, spoken, voice, (event) => {
        if (this.#request !== id) return;
        if (event.state === "preparing" || event.state === "speaking") {
          this.#set({ ...this.#state, key, phase: event.state });
        } else if (event.state === "failed") {
          fail(event.message, event.reason);
        } else {
          this.#request = null;
          this.#set({ ...this.#state, key: null, phase: "idle" });
        }
      });
    } catch (reason) {
      fail(errorMessage(reason), null);
    }
  }

  stop(): void {
    const id = this.#request;
    if (id === null) return;
    this.#request = null;
    this.#set({ ...this.#state, key: null, phase: "idle" });
    void this.#api.stop(id).catch(() => undefined);
  }
}

/** The window's voice. */
export const speechPlayer = new SpeechPlayer(speechApi);
