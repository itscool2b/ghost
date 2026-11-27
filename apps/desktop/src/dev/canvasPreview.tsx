/**
 * Canvas fixture (`?preview=canvas`): a chat over an in-memory transport, with a scripted
 * microphone and a paste that records what the canvas attaches. No agent, files or microphone.
 * The check says phrases by setting `window.canvasSaid`; the microphone reports them as settled,
 * and `window.canvasForming` as the words still forming after them.
 */
import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { ComposerAttachment, ComposerInput, DictationEvent } from "../bindings";
import { ChatView } from "../components/chat/ChatView";
import {
  attachResult,
  chatSummary,
  FakeTransport,
  historyPage,
  sessionInfo,
} from "../lib/chat/testing";
import type { DictationApi } from "../lib/dictation";
import { withPreviewContext } from "./previewContext";

declare global {
  interface Window {
    canvasSaid?: string[];
    canvasForming?: string;
    canvasPastes?: { name: string | undefined; bytes: number; png: boolean }[];
    canvasDraft?: () => ComposerInput;
  }
}

/** A microphone that hears whatever the check put in `window.canvasSaid`. */
function scriptedMicrophone(): DictationApi {
  let timer: ReturnType<typeof setInterval> | undefined;
  let emit: ((event: DictationEvent) => void) | null = null;
  let settled = "";
  return {
    model: async () => ({
      installed: true,
      supported: true,
      name: "fixture",
      bytes: 0,
      larger: {
        name: "fixture",
        bytes: 0,
        state: "absent",
        available: false,
        received: 0,
        chosen: false,
      },
    }),
    start: async (_id, onEvent) => {
      emit = onEvent;
      settled = "";
      window.canvasSaid = [];
      window.canvasForming = "";
      let tick = 0;
      onEvent({ state: "preparing" });
      timer = setInterval(() => {
        tick += 1;
        settled = (window.canvasSaid ?? []).join(" ");
        onEvent({
          state: "listening",
          seconds: Math.floor(tick * 0.08),
          level: 0.4,
          speaking: true,
          text: {
            settled: settled && window.canvasForming ? `${settled} ` : settled,
            stable: "",
            tentative: window.canvasForming ?? "",
          },
        });
      }, 80);
    },
    stop: async () => {
      clearInterval(timer);
      emit?.({ state: "finishing", text: { settled, stable: "", tentative: "" } });
      emit?.({ state: "complete", text: settled });
    },
    cancel: async () => {
      clearInterval(timer);
      emit?.({ state: "cancelled" });
    },
  };
}

function Preview() {
  const [fixture] = useState(() => {
    const chat = chatSummary("canvas-chat", { status: "ready" });
    const transport = withPreviewContext(
      new FakeTransport({
        attach: async () =>
          attachResult(chat.id, 0, historyPage(chat.id, []), {
            chat,
            session: sessionInfo({ status: "ready" }),
          }),
      }),
    );
    let draft: ComposerInput = { text: "", attachments: [], mentions: [] };
    const files: ComposerAttachment[] = [];
    const pastes: { name: string | undefined; bytes: number; png: boolean }[] = [];
    transport.composer = {
      ...transport.composer,
      draft: async () => ({
        input: draft,
        files: files.filter((file) => draft.attachments.includes(file.id)),
      }),
      saveDraft: async (_id, input) => {
        draft = input;
      },
      paste: async (_id, bytes, name) => {
        // A PNG starts with its signature: 0x89 "PNG".
        const png =
          bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47;
        pastes.push({ name, bytes: bytes.length, png });
        const item: ComposerAttachment = {
          id: `canvas-${pastes.length}`,
          name: name ?? "Pasted image.png",
          mime: "image/png",
          bytes: bytes.length,
          image: true,
        };
        files.push(item);
        return item;
      },
      dictation: scriptedMicrophone(),
    };
    return { chat, transport, pastes, draft: () => draft };
  });
  useEffect(() => {
    window.canvasPastes = fixture.pastes;
    window.canvasDraft = fixture.draft;
  }, [fixture]);
  return (
    <main className="chat-app" style={{ display: "flex", flexDirection: "column" }}>
      <h1 className="sr-only">Canvas verification</h1>
      <div className="chat-main" style={{ flex: 1, minHeight: 0 }}>
        <ChatView
          chatId={fixture.chat.id}
          listed={fixture.chat}
          transport={fixture.transport}
          agents={[]}
          storage={null}
        />
      </div>
    </main>
  );
}

export function mountCanvasPreview(container: HTMLElement) {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
