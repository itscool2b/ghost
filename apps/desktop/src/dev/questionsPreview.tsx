/** Owned browser fixture for pending questions. It makes no native or agent calls. */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import type { EventBatch, GhostEvent, UiAnswer, UiPrompt } from "../bindings";
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
import { withPreviewContext } from "./previewContext";

function Preview() {
  const [revision, setRevision] = useState(0);
  const [fixture] = useState(() => {
    const scenario = new URLSearchParams(location.search).get("case");
    const status = scenario === "startup" ? "starting" : "running";
    const chat = chatSummary("questions-chat", { status });
    const events: GhostEvent[] = [];
    for (let i = 0; i < 90; i++)
      events.push(
        ev.turnStarted(`old-${i}`),
        ev.user(`user-${i}`, `Older prompt ${i}`),
        ev.message(`message-${i}`, `Earlier saved answer ${i}`),
        ev.turnEnded(`old-${i}`),
      );
    if (status === "running")
      events.push(ev.turnStarted("current"), ev.user("current-user", "Inspect the checkout"));
    let receive: ((value: EventBatch) => void) | null = null;
    let pending: {
      id: string;
      event: GhostEvent;
      resolve: () => void;
      reject: (error: Error) => void;
    } | null = null;
    const calls: { id: string; answer: UiAnswer | null }[] = [];
    const permissionCalls: { id: string; optionId: string | null }[] = [];
    let sent = false;
    const publish = (updates: GhostEvent[]) => {
      const start = events.length;
      events.push(...updates);
      receive?.(frame(chat.id, start, updates));
    };
    const transport = withPreviewContext(
      new FakeTransport({
        attach: async (_id, onFrame) => {
          receive = onFrame;
          return attachResult(
            chat.id,
            events.length,
            historyPage(chat.id, items(numbered(events.map((event) => [null, event])))),
            {
              chat,
              session: sessionInfo({ status }),
              open_turn_id: status === "running" ? "current" : null,
            },
          );
        },
        respondUi: async (_chat, id, answer) => {
          calls.push({ id, answer });
          return new Promise<void>((resolve, reject) => {
            pending = { id, event: ev.questionResolved(id, answer), resolve, reject };
          });
        },
        respondPermission: async (_chat, id, optionId) => {
          permissionCalls.push({ id, optionId });
          return new Promise<void>((resolve, reject) => {
            pending = { id, event: ev.permissionResolved(id, optionId), resolve, reject };
          });
        },
      }),
    );
    Object.assign(window, { questionCalls: calls, permissionCalls });
    return {
      chat,
      transport,
      ask() {
        if (sent) return;
        sent = true;
        const prompt: UiPrompt =
          scenario === "form"
            ? {
                type: "form",
                message: "Set up the deployment",
                source: "deploy",
                fields: [
                  {
                    id: "name",
                    label: "Project name",
                    description: "Lowercase letters and dashes.",
                    required: true,
                    kind: {
                      type: "text",
                      format: null,
                      min_length: 2,
                      max_length: 40,
                      default: null,
                    },
                  },
                  {
                    id: "region",
                    label: "Region",
                    description: null,
                    required: true,
                    kind: {
                      type: "choice",
                      options: ["Ireland", "Virginia", "Tokyo"],
                      multiple: false,
                      min_items: null,
                      max_items: null,
                      default: [],
                    },
                  },
                  {
                    id: "replicas",
                    label: "Replicas",
                    description: null,
                    required: false,
                    kind: { type: "number", integer: true, minimum: 1, maximum: 5, default: 2 },
                  },
                  {
                    id: "notify",
                    label: "Email me when it is live",
                    description: null,
                    required: false,
                    kind: { type: "boolean", default: null },
                  },
                  {
                    id: "email",
                    label: "Email",
                    description: null,
                    required: false,
                    kind: {
                      type: "text",
                      format: "email",
                      min_length: null,
                      max_length: null,
                      default: null,
                    },
                  },
                  {
                    id: "features",
                    label: "Features",
                    description: "Up to two.",
                    required: false,
                    kind: {
                      type: "choice",
                      options: ["Logs", "Metrics", "Tracing"],
                      multiple: true,
                      min_items: null,
                      max_items: 2,
                      default: [0],
                    },
                  },
                ],
              }
            : scenario === "link"
              ? {
                  type: "link",
                  message: "Sign in to GitHub to continue",
                  source: "github",
                  url: "https://github.com/login/device?code=AB12",
                }
              : scenario === "startup"
                ? {
                    type: "select",
                    message:
                      "Codex could not open this session: original startup error. Try the advertised default?",
                    options: ["Use advertised default", "Keep current model and stop"],
                  }
                : {
                    type: "questions",
                    message: "Choose the review scope",
                    questions: [
                      {
                        id: "scope",
                        header: "Scope",
                        question: "Which tests should run?",
                        options: [
                          { label: "Unit tests", description: "Run the focused suite" },
                          { label: "All tests", description: "Run all suites" },
                        ],
                        multi_select: false,
                        allow_free_text: true,
                      },
                    ],
                  };
        if (scenario === "group" && prompt.type === "questions")
          prompt.questions.push(
            {
              id: "details",
              header: "Details",
              question: "Which details should I report?",
              options: [
                { label: "Failures", description: "Include failing checks." },
                { label: "Timing", description: "Include elapsed times." },
              ],
              multi_select: true,
              allow_free_text: true,
            },
            {
              id: "notes",
              header: "Notes",
              question: "What else should I know?",
              options: [],
              multi_select: false,
              allow_free_text: true,
            },
          );
        if (scenario === "permission-first") publish([ev.permission("permission-1")]);
        publish([
          { type: "ui_request", request_id: "question-1", prompt },
          {
            type: "ui_request",
            request_id: "notification",
            prompt: { type: "notify", message: "Informational update only" },
          },
        ]);
        if (scenario === "mixed") publish([ev.permission("permission-1")]);
      },
      resolve() {
        if (!pending) return;
        publish([pending.event]);
        pending.resolve();
        pending = null;
      },
      fail() {
        pending?.reject(new Error("Answer was not delivered. Try again."));
        pending = null;
      },
      withdraw() {
        publish([ev.questionResolved("question-1", null, "agent")]);
        if (pending?.id === "question-1") {
          pending.resolve();
          pending = null;
        }
      },
      askAnother() {
        publish([
          {
            type: "ui_request",
            request_id: "question-2",
            prompt: { type: "input", message: "Any final notes?", placeholder: null },
          },
        ]);
      },
      update() {
        publish([ev.notice("New background update")]);
      },
    };
  });
  return (
    <main className="chat-app" style={{ display: "flex", flexDirection: "column" }}>
      <h1 className="sr-only">Question dock verification</h1>
      <nav
        aria-label="Fixture controls"
        className="ui-row"
        style={{ position: "relative", flexShrink: 0, padding: 12 }}
      >
        <button type="button" className="g-button" onClick={fixture.ask}>
          Ask question
        </button>
        <button type="button" className="g-button" onClick={fixture.resolve}>
          Resolve answer
        </button>
        <button type="button" className="g-button" onClick={fixture.fail}>
          Fail answer
        </button>
        <button type="button" className="g-button" onClick={fixture.withdraw}>
          Withdraw question
        </button>
        <button type="button" className="g-button" onClick={fixture.update}>
          Background update
        </button>
        <button type="button" className="g-button" onClick={fixture.askAnother}>
          Ask another question
        </button>
        <button
          type="button"
          className="g-button"
          onClick={() => setRevision((value) => value + 1)}
        >
          Reattach chat
        </button>
      </nav>
      <div className="chat-main" style={{ flex: 1, minHeight: 0 }}>
        <ChatView
          key={revision}
          chatId={fixture.chat.id}
          transport={fixture.transport}
          listed={fixture.chat}
          storage={null}
          agents={[]}
        />
      </div>
    </main>
  );
}
export function mountQuestionsPreview(container: HTMLElement) {
  createRoot(container).render(<Preview />);
}
