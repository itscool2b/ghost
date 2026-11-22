/**
 * Scripted UI fixture. Never opens a microphone or invokes Rust: the transport plays a recording
 * from a script, a word at a time, as the native engine reports one. `case` picks a failure
 * (`denied`, `missing`, `unsupported`); `auto=1` plays the whole recording by itself; `draft`
 * starts the message box with text.
 */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type { DictationEvent, LiveText } from "../bindings";
import { Composer } from "../components/chat/Composer";
import type { ComposerTransport } from "../lib/composer";
import type { DictationApi } from "../lib/dictation";

const SCRIPT = "Add a regression test for the dictation bar and make sure it stays fast on battery";
const FINAL =
  "Add a regression test for the dictation bar, and make sure it stays fast on battery.";
const TICK = 80;

function fixture() {
  const params = new URLSearchParams(location.search);
  const scenario = params.get("case") ?? "";
  const auto = params.get("auto") === "1";
  const words = SCRIPT.split(" ");
  let receive: (event: DictationEvent) => void = () => {};
  let timer = 0;
  let began = 0;
  let tick = 0;
  let heard = 0;
  let settled = 0;
  let speaking = false;
  const text = (): LiveText => {
    const forming = words.slice(settled, heard);
    const agreed = Math.max(0, forming.length - 2);
    return {
      settled: words.slice(0, settled).join(" "),
      stable: forming.slice(0, agreed).join(" "),
      tentative: forming.slice(agreed).join(" "),
    };
  };
  const final = () => {
    if (heard === words.length) return FINAL;
    return heard ? `${words.slice(0, heard).join(" ")}.` : "";
  };
  const report = () => {
    // A voice's level rises and falls; the room is nearly silent.
    const level = speaking
      ? 0.2 + 0.6 * Math.abs(Math.sin(tick * 1.7) * Math.cos(tick * 0.45))
      : 0.02;
    receive({
      state: "listening",
      seconds: Math.floor((Date.now() - began) / 1000),
      level,
      speaking,
      text: text(),
    });
  };
  const hear = (count = 3) => {
    heard = Math.min(words.length, heard + count);
    speaking = true;
    report();
  };
  const pause = () => {
    settled = heard;
    speaking = false;
    report();
  };
  const halt = () => {
    window.clearInterval(timer);
    timer = 0;
  };
  const complete = () => {
    halt();
    const said = final();
    receive(
      said
        ? { state: "complete", text: said }
        : {
            state: "failed",
            reason: "no_speech",
            message:
              "No speech was detected. Check that the right microphone is selected and not muted.",
          },
    );
  };
  const api: DictationApi = {
    model: async () => ({
      installed: scenario !== "missing",
      name: "Parakeet Ultra",
      bytes: 487521152,
      supported: scenario !== "unsupported",
    }),
    start: async (_id, event) => {
      receive = event;
      halt();
      began = Date.now();
      tick = 0;
      heard = 0;
      settled = 0;
      speaking = false;
      if (scenario === "denied") {
        event({
          state: "failed",
          reason: "microphone",
          message:
            "Microphone access is turned off for desktop apps. Allow it in your system privacy settings, then try again.",
        });
        return;
      }
      event({ state: "preparing" });
      report();
      timer = window.setInterval(() => {
        tick += 1;
        if (auto && tick % 4 === 0 && heard < words.length) {
          if (heard && heard % 7 === 0 && settled < heard) pause();
          else hear(1);
        }
        report();
      }, TICK);
    },
    stop: async () => {
      halt();
      receive({ state: "finishing", text: text() });
      if (auto) window.setTimeout(complete, 600);
    },
    cancel: async () => {
      halt();
      receive({ state: "cancelled" });
    },
  };
  const composer: ComposerTransport = {
    dictation: api,
    draft: async () => ({
      input: { text: params.get("draft") ?? "", attachments: [], mentions: [] },
      files: [],
    }),
    saveDraft: async () => {},
    pick: async () => null,
    import: async () => {
      throw new Error("Not used by this fixture");
    },
    paste: async () => {
      throw new Error("Not used by this fixture");
    },
    mentions: async () => [],
    send: async () => {
      throw new Error("No agent runs in this fixture");
    },
  };
  return {
    composer,
    hear,
    pause,
    complete,
    drop: () => {
      halt();
      receive({
        state: "failed",
        reason: "microphone",
        message: "The microphone disconnected or stopped responding.",
      });
    },
  };
}

function Preview() {
  const [sent, setSent] = useState(0);
  const [mounted, setMounted] = useState(true);
  const [script] = useState(fixture);
  return (
    <main
      className="chat-main"
      style={{ height: "100vh", padding: 24, justifyContent: "flex-end" }}
    >
      <h1>Dictation preview</h1>
      <nav aria-label="Fixture controls" style={{ display: "flex", flexWrap: "wrap", gap: 8 }}>
        <button type="button" className="g-button" onClick={() => script.hear()}>
          Hear words
        </button>
        <button type="button" className="g-button" onClick={script.pause}>
          Pause
        </button>
        <button type="button" className="g-button" onClick={script.complete}>
          Complete transcription
        </button>
        <button type="button" className="g-button" onClick={script.drop}>
          Drop microphone
        </button>
        <button type="button" className="g-button" onClick={() => setMounted((value) => !value)}>
          Switch chat
        </button>
        <output aria-label="Messages sent">{sent}</output>
      </nav>
      <div className="chat-compose-slot" style={{ marginTop: "auto" }}>
        {mounted ? (
          <Composer
            key="original"
            chatId="dictation-preview"
            contextApi={script.composer}
            commands={[]}
            status="ready"
            error={null}
            onSend={async () => {
              setSent((n) => n + 1);
              return true;
            }}
            onStop={() => {}}
            onForceStop={() => {}}
          />
        ) : (
          <p>Another chat</p>
        )}
      </div>
    </main>
  );
}

export function mountDictationPreview(container: HTMLElement) {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
