import { emit, listen } from "@tauri-apps/api/event";
import { inTauri } from "./ipc";

/** The question Rust asks before quitting (active chats, unsaved messages, open terminals). */
export interface QuitQuestion {
  readonly token: string;
  readonly message: string;
  /** The explicit choice that quits, such as "Stop agents and quit". */
  readonly quit: string;
  /** The default, focused choice: "Keep Ghost open". */
  readonly keep: string;
}

/** The event names Rust uses (sessions.rs): the question, that it shows, and the choice. */
export const QUIT_QUESTION_EVENT = "auracode-quit-question";
export const QUIT_QUESTION_SHOWN_EVENT = "auracode-quit-question-shown";
export const QUIT_ANSWER_EVENT = "auracode-quit-answer";

export interface QuitChannel {
  /** Calls `ask` for every question Rust sends; returns how to stop listening. */
  readonly listen: (ask: (question: QuitQuestion) => void) => () => void;
  readonly shown: (token: string) => void;
  readonly answer: (token: string, quit: boolean) => void;
}

export const tauriQuitChannel: QuitChannel = {
  listen: (ask) => {
    let stop: (() => void) | null = null;
    let stopped = false;
    void listen<QuitQuestion>(QUIT_QUESTION_EVENT, (event) => ask(event.payload))
      .then((unlisten) => {
        if (stopped) unlisten();
        else stop = unlisten;
      })
      .catch(() => {
        /* Rust falls back to the system dialog when the question is never shown. */
      });
    return () => {
      stopped = true;
      stop?.();
    };
  },
  shown: (token) => void emit(QUIT_QUESTION_SHOWN_EVENT, { token }).catch(() => {}),
  answer: (token, quit) => void emit(QUIT_ANSWER_EVENT, { token, quit }).catch(() => {}),
};

/**
 * The open question, one at a time (Rust runs one quit flow at a time; a newer question replaces
 * an older one). The dialog reports that it shows the question once it is on screen, and every
 * way of closing it but the quit button keeps Ghost open.
 */
export class QuitQuestions {
  #current: QuitQuestion | null = null;
  readonly #listeners = new Set<() => void>();
  readonly #channel: QuitChannel;

  constructor(channel: QuitChannel) {
    this.#channel = channel;
  }

  start(): () => void {
    return this.#channel.listen((question) => {
      this.#current = question;
      this.#notify();
    });
  }

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  readonly getSnapshot = (): QuitQuestion | null => this.#current;

  /** The dialog is on screen: Rust waits for the choice instead of asking natively. */
  shown(token: string): void {
    if (this.#current?.token === token) this.#channel.shown(token);
  }

  answer(token: string, quit: boolean): void {
    if (this.#current?.token !== token) return;
    this.#current = null;
    this.#channel.answer(token, quit);
    this.#notify();
  }

  #notify(): void {
    for (const listener of this.#listeners) listener();
  }
}

/** Whether the native shell can ask: only inside Tauri. */
export function quitQuestionsAvailable(): boolean {
  return inTauri();
}
