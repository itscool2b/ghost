/** Browser-only steering fixture. It never invokes the native transport or starts an agent. */
import { useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  Capabilities,
  EventBatch,
  O7Event,
  QueuedPrompt,
  SessionStatus,
  SteeringAck,
} from "../bindings";
import { ChatView } from "../components/chat/ChatView";
import {
  attachResult,
  chatSummary,
  ev,
  FakeTransport,
  frame,
  historyPage,
  items,
  lists,
  numbered,
  sessionInfo,
} from "../lib/chat/testing";
import { withPreviewContext } from "./previewContext";

function Preview() {
  const [fixture] = useState(() => {
    const scenario = new URLSearchParams(location.search).get("case");
    let status: SessionStatus = scenario === "idle" ? "ready" : "running";
    let turn: string | null =
      scenario === "unknown-turn" || scenario === "idle" ? null : "fixture-turn";
    // Codex steers; the unsupported case is Claude Code, which takes a message after the turn.
    const agent =
      scenario === "unsupported"
        ? { agent_id: "claude", agent_name: "Claude Code" }
        : { agent_id: "codex", agent_name: "Codex" };
    const chat = chatSummary("steering-chat", { ...agent, status });
    const events: O7Event[] = [
      ev.turnStarted("fixture-turn"),
      ev.user("original", "Review this change"),
    ];
    if (scenario === "idle") events.push(ev.turnEnded("fixture-turn"));
    if (scenario === "remote-history") {
      events.push(ev.turnEnded("fixture-turn"));
      status = "stopped";
      turn = null;
    }
    if (scenario === "missing-native") {
      events.push(
        ev.turnEnded("fixture-turn"),
        ev.error("agent_exited", "The saved native transcript is unavailable."),
      );
      status = "failed";
      turn = null;
    }
    if (scenario === "recovered") {
      events.push(
        { type: "steering_status", message_id: "recovered-steer", state: "pending", detail: null },
        ev.user("recovered-steer", "Keep this visible after recovery"),
        ev.turnEnded("fixture-turn", "interrupted", { recovered: true }),
      );
      status = "stopped";
      turn = null;
    }
    let receive: ((batch: EventBatch) => void) | null = null;
    let pending: ((value: SteeringAck) => void) | null = null;
    let message = "";
    const calls = { steer: 0, send: 0, expected: "" };
    const queue: QueuedPrompt[] = [];
    const words = new Map<string, string>();
    const recovery = { starts: 0, previews: [] as string[], applied: 0 };
    const publish = (updates: O7Event[]) => {
      const seq = events.length;
      events.push(...updates);
      receive?.(frame(chat.id, seq, updates));
    };
    const capabilities: Capabilities = {
      steer: scenario !== "unsupported",
      images: true,
      interrupt: true,
      resume: true,
      fork: false,
      rewind: false,
      set_model: false,
      set_effort: false,
      permissions: false,
      plans: false,
      usage: false,
      limits: false,
      slash_commands: false,
      add_dirs: false,
      plan_edits: false,
      autonomy_levels: ["full_auto"],
    };
    const transport = withPreviewContext(
      new FakeTransport({
        lists: async () => lists(1, { capabilities }),
        start: async () => {
          recovery.starts++;
        },
        attach: async (_id, onFrame) => {
          receive = onFrame;
          return attachResult(
            chat.id,
            events.length,
            historyPage(chat.id, items(numbered(events.map((event) => [null, event])))),
            {
              chat,
              session: sessionInfo({ status, agent_name: agent.agent_name }),
              queue: queue.map((row) => ({ ...row })),
              open_turn_id: turn,
            },
          );
        },
        interrupt: async () => {
          status = "cancelling";
          publish([ev.session({ status })]);
        },
        send: async (_id, text) => {
          calls.send++;
          if (status !== "running") return { prompt_id: "queued", queued: true };
          // While the turn runs a message waits in the queue, which can be edited and reordered.
          const prompt_id = `queued-${calls.send}`;
          words.set(prompt_id, text);
          queue.push({
            prompt_id,
            preview: text.slice(0, 256),
            held: false,
            created_at: calls.send,
          });
          publish([ev.lifecycle(prompt_id, "queued", text.slice(0, 256))]);
          return { prompt_id, queued: true };
        },
        queuedText: async (_id, promptId) => words.get(promptId) ?? "",
        editQueued: async (_id, promptId, text) => {
          const row = queue.find((item) => item.prompt_id === promptId);
          if (row && text.trim()) {
            words.set(promptId, text);
            row.preview = text.slice(0, 256);
          }
          if (!text.trim()) throw new Error("The message is empty.");
          return queue.map((item) => ({ ...item }));
        },
        moveQueued: async (_id, promptId, index) => {
          const from = queue.findIndex((item) => item.prompt_id === promptId);
          const [row] = queue.splice(from, 1);
          if (row) queue.splice(Math.min(index, queue.length), 0, row);
          return queue.map((item) => ({ ...item }));
        },
        cancelQueued: async (_id, promptId) => {
          const index = queue.findIndex((item) => item.prompt_id === promptId);
          if (index >= 0) queue.splice(index, 1);
          publish([ev.lifecycle(promptId, "cancelled")]);
        },
      }),
    );
    transport.composer.steer = (_id, expected, input) => {
      calls.steer++;
      calls.expected = expected;
      message = `steer-${calls.steer}`;
      publish([
        { type: "steering_status", message_id: message, state: "pending", detail: null },
        ev.user(
          message,
          [
            input.text,
            ...input.mentions.map((path) => `@${path}`),
            ...input.attachments.map(() => "[File: interface-notes.md]"),
          ].join("\n"),
        ),
      ]);
      return new Promise((resolve) => {
        pending = resolve;
      });
    };
    const originalPreview = transport.conversation.preview;
    const originalTurns = transport.conversation.turns;
    transport.conversation.turns = async (id, before) => ({
      ...(await originalTurns(id, before)),
      can_continue_here: scenario === "remote-history",
    });
    transport.conversation.preview = async (id, action) => {
      recovery.previews.push(action.type);
      const preview = await originalPreview(id, action);
      return action.type === "fresh_session" || action.type === "continue_here"
        ? { ...preview, agent_id: chat.agent_id }
        : preview;
    };
    const originalPending = transport.conversation.pending;
    transport.conversation.pending = async (id) => {
      const pending = await originalPending(id);
      return pending && ["fresh_session", "continue_here"].includes(recovery.previews.at(-1) ?? "")
        ? { ...pending, agent_id: chat.agent_id }
        : pending;
    };
    const originalApply = transport.conversation.apply;
    transport.conversation.apply = (id, token) => {
      recovery.applied++;
      return originalApply(id, token);
    };
    Object.assign(window, { steeringCalls: calls, sessionRecoveryCalls: recovery });
    return {
      chat,
      transport,
      answer(state: "accepted" | "rejected" | "unconfirmed") {
        if (!pending) return;
        const detail = state === "accepted" ? null : `Fixture ${state}; your draft is kept.`;
        publish([{ type: "steering_status", message_id: message, state, detail }]);
        pending({ message_id: message, state, detail });
        pending = null;
      },
      end() {
        turn = null;
        status = "ready";
        publish([ev.turnEnded("fixture-turn"), ev.session({ status })]);
      },
    };
  });
  return (
    <main className="chat-app" style={{ display: "flex", flexDirection: "column" }}>
      <h1 className="sr-only">Steering verification</h1>
      <nav
        aria-label="Fixture outcomes"
        style={{ position: "relative", display: "flex", flexShrink: 0, gap: 12, padding: 12 }}
      >
        <button type="button" className="g-button" onClick={() => fixture.answer("accepted")}>
          Accept
        </button>
        <button type="button" className="g-button" onClick={() => fixture.answer("rejected")}>
          Reject
        </button>
        <button type="button" className="g-button" onClick={() => fixture.answer("unconfirmed")}>
          Lose reply
        </button>
        <button type="button" className="g-button" onClick={() => fixture.end()}>
          End turn
        </button>
      </nav>
      <div className="chat-main" style={{ flex: 1, minHeight: 0 }}>
        <ChatView
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

export function mountSteeringPreview(container: HTMLElement) {
  createRoot(container).render(<Preview />);
}
