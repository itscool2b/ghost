/**
 * Development-only stand-in for reading aloud: a voice model that is "built in", three voices, and
 * a request that gets ready, speaks, and ends when told to. No audio is made. `window.speechFixture`
 * reports what was asked for, for the browser checks.
 */
import type { SpeechEvent, SpeechStatus } from "../bindings";
import type { SpeechApi } from "../lib/speech";

export interface SpeechFixture {
  readonly api: SpeechApi;
  /** Every request, in order: what was read and in which voice. */
  readonly said: { readonly id: string; readonly text: string; readonly voice: string | null }[];
  readonly stopped: string[];
  readonly prepared: (string | null)[];
  /** Ends what is being read, as reaching the end of the text does. */
  finish(): void;
  /** Fails what is being read, as a lost output device does. */
  fail(message: string): void;
}

declare global {
  interface Window {
    speechFixture?: SpeechFixture;
  }
}

const STATUS: SpeechStatus = {
  installed: true,
  supported: true,
  name: "Phonon 31M",
  voices: [
    {
      id: "default",
      name: "Reuben",
      locale: "en-us",
      gender: "male",
      description: "The voice model's own: steady and plain.",
    },
    {
      id: "NbpkqMVS3CJeq2j8",
      name: "Zoey",
      locale: "en-us",
      gender: "female",
      description: "Playful and upbeat, with a standard American accent.",
    },
    {
      id: "Qm4wT-u1_cXb7RzA",
      name: "Alistair",
      locale: "en-gb",
      gender: "male",
      description: "Measured and warm, with a British accent.",
    },
    {
      id: "c8HnPq2LkV5sYd0E",
      name: "The Captain",
      locale: "characters",
      gender: "male",
      description: "A gravelly old sailor.",
    },
  ],
};

/**
 * `status` is what the fixture's build says of itself: by default a model that is built in and
 * can run. `?speech=missing` and `?speech=unsupported` show the two states without one.
 */
export function speechFixture(status: SpeechStatus = fixtureStatus()): SpeechFixture {
  let current: { id: string; emit: (event: SpeechEvent) => void } | null = null;
  const end = (event: SpeechEvent) => {
    const ended = current;
    current = null;
    ended?.emit(event);
  };
  const fixture: SpeechFixture = {
    said: [],
    stopped: [],
    prepared: [],
    finish: () => end({ state: "done" }),
    fail: (message) => end({ state: "failed", message, reason: "output" }),
    api: {
      status: async () => status,
      speak: async (id, text, voice, emit) => {
        // As the core does: whatever was being read stops first.
        end({ state: "stopped" });
        fixture.said.push({ id, text, voice });
        current = { id, emit };
        emit({ state: "preparing" });
        setTimeout(() => {
          if (current?.id === id) emit({ state: "speaking" });
        }, 60);
      },
      stop: async (id) => {
        fixture.stopped.push(id);
        if (current?.id === id) end({ state: "stopped" });
      },
      prepare: async (voice) => {
        fixture.prepared.push(voice);
      },
    },
  };
  return fixture;
}

function fixtureStatus(): SpeechStatus {
  const wanted = new URLSearchParams(location.search).get("speech");
  if (wanted === "missing") return { ...STATUS, installed: false, voices: [] };
  if (wanted === "unsupported") return { ...STATUS, supported: false, voices: [] };
  return STATUS;
}
