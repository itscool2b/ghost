/** Owned image fixture. Uses generated PNGs and an in-memory transport, never native files. */
import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import type { ChatImageSource, ComposerInput, EventBatch, GhostEvent } from "../bindings";
import { ChatView } from "../components/chat/ChatView";
import {
  attachResult,
  chatSummary,
  ev,
  FakeTransport,
  frame,
  historyPage,
  items,
  numbered,
  sessionInfo,
} from "../lib/chat/testing";
import type { ChatImagesApi } from "../lib/chatImages";
import { withPreviewContext } from "./previewContext";

function Preview() {
  const [revision, setRevision] = useState(0);
  const [fixture] = useState(() => {
    const canvas = document.createElement("canvas");
    canvas.width = 640;
    canvas.height = 400;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("Canvas unavailable");
    context.fillStyle = "#dfe9ee";
    context.fillRect(0, 0, 640, 400);
    context.fillStyle = "#152e3a";
    context.fillRect(32, 32, 160, 336);
    context.fillStyle = "#ffffff";
    context.fillRect(216, 32, 392, 336);
    context.fillStyle = "#526f80";
    context.fillRect(244, 68, 270, 20);
    context.fillRect(244, 112, 320, 9);
    const png = canvas.toDataURL("image/png").split(",")[1] ?? "";
    const chat = chatSummary("images-chat", { status: "ready" });
    const events: GhostEvent[] = [
      ev.turnStarted("saved"),
      ev.user("saved.u", "Review the attached screen.\n\n[Image: screen.png]"),
      ev.message(
        "answer",
        `Here is the revised screen.\n\n![Revised screen](<images/revised (1).png>)\n\n![Generated image](<aura-image:${"ab".repeat(32)}>)\n\n![Missing output](missing.png)\n\n![Remote reference](https://example.invalid/tracker.png)`,
      ),
      ev.turnEnded("saved"),
    ];
    let receive: ((batch: EventBatch) => void) | null = null;
    const transport = withPreviewContext(
      new FakeTransport({
        attach: async (_id, onFrame) => {
          receive = onFrame;
          return attachResult(
            chat.id,
            events.length,
            historyPage(chat.id, items(numbered(events.map((event) => [null, event])))),
            { chat, session: sessionInfo({ status: "ready" }) },
          );
        },
      }),
    );
    const attachment = {
      id: "draft-image",
      name: "Draft screen.png",
      mime: "image/png",
      bytes: 2048,
      image: true,
    };
    let draft: ComposerInput = {
      text: "Keep the controls compact",
      attachments: [attachment.id],
      mentions: [],
    };
    let failing = false;
    const calls: { source: ChatImageSource; full: boolean }[] = [];
    const sent: ComposerInput[] = [];
    const images: ChatImagesApi = {
      list: async (_id, context) => [
        { source: { type: "message", context, index: 0 }, name: "Attached image 1" },
      ],
      preview: async (_id, source, full) => {
        calls.push({ source, full });
        await new Promise((resolve) => setTimeout(resolve, 70));
        if (source.type === "project" && source.path === "missing.png")
          throw new Error("The file is missing from this chat folder.");
        if (failing) throw new Error("The image could not be read. Try again.");
        return { png_base64: png, width: 640, height: 400 };
      },
    };
    transport.composer = {
      ...transport.composer,
      draft: async () => ({ input: draft, files: draft.attachments.length ? [attachment] : [] }),
      saveDraft: async (_id, input) => {
        draft = input;
      },
      pick: async () => attachment,
      paste: async () => attachment,
      send: async (_id, input) => {
        sent.push(input);
        const update = [
          ev.turnStarted("sent"),
          ev.user("sent.u", `${input.text}\n\n[Image: Draft screen.png]`),
          ev.turnEnded("sent"),
        ];
        const start = events.length;
        events.push(...update);
        receive?.(frame(chat.id, start, update));
        return { prompt_id: "sent-prompt", queued: false };
      },
    };
    return {
      chat,
      calls,
      sent,
      transport: Object.assign(transport, { images }),
      fail: () => {
        failing = true;
      },
      recover: () => {
        failing = false;
      },
    };
  });
  useEffect(() => {
    Object.assign(window, { imageCalls: fixture.calls, sentImages: fixture.sent });
  }, [fixture]);
  return (
    <main className="chat-app" style={{ display: "flex", flexDirection: "column" }}>
      <h1 className="sr-only">Image preview verification</h1>
      <nav
        className="ui-row"
        aria-label="Fixture controls"
        style={{ padding: 12, flexWrap: "wrap" }}
      >
        <button type="button" className="g-button" onClick={() => setRevision(revision + 1)}>
          Reattach chat
        </button>
        <button type="button" className="g-button" onClick={fixture.fail}>
          Fail image reads
        </button>
        <button type="button" className="g-button" onClick={fixture.recover}>
          Recover image reads
        </button>
      </nav>
      <div className="chat-main" style={{ flex: 1, minHeight: 0 }}>
        <ChatView
          key={revision}
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

export function mountImagesPreview(container: HTMLElement) {
  createRoot(container).render(
    <StrictMode>
      <Preview />
    </StrictMode>,
  );
}
