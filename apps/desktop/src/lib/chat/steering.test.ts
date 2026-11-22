import { describe, expect, it, vi } from "vitest";
import type { Capabilities, SteeringAck, SteeringState } from "../../bindings";
import type { ComposerTransport } from "../composer";
import { ChatStore } from "./chatStore";
import {
  attachResult,
  ev,
  FakeTransport,
  frame,
  historyPage,
  lists,
  sessionInfo,
  settled,
} from "./testing";

const capabilities: Capabilities = {
  steer: true,
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

async function fixture(state: SteeringState = "accepted", supported = true) {
  const steer = vi.fn(
    async (): Promise<SteeringAck> => ({ message_id: "steer", state, detail: null }),
  );
  const composer: ComposerTransport = {
    steer,
    draft: async () => ({ input: { text: "", attachments: [], mentions: [] }, files: [] }),
    saveDraft: async () => {},
    pick: async () => null,
    import: async () => {
      throw new Error("Not used");
    },
    paste: async () => {
      throw new Error("Not used");
    },
    mentions: async () => [],
    send: vi.fn(async () => ({ prompt_id: "queued", queued: true })),
  };
  const fake = Object.assign(
    new FakeTransport({
      lists: async () => lists(1, { capabilities: { ...capabilities, steer: supported } }),
    }),
    { composer },
  );
  const store = new ChatStore("chat", fake);
  const opening = store.open();
  fake.lastAttach().resolve(
    attachResult("chat", 0, historyPage("chat", []), {
      session: sessionInfo({ status: "running" }),
      open_turn_id: "turn-1",
    }),
  );
  await opening;
  await settled();
  return { store, fake, steer, composer };
}

describe("explicit steering", () => {
  it("passes the observed turn and exact context, then acknowledges accepted input only", async () => {
    const { store, steer } = await fixture();
    expect(
      await store.steer("turn-1", "  visible text  ", {
        attachments: ["image"],
        mentions: ["src/main.rs"],
      }),
    ).toBe(true);
    expect(steer).toHaveBeenCalledWith("chat", "turn-1", {
      text: "  visible text  ",
      attachments: ["image"],
      mentions: ["src/main.rs"],
    });
    expect(store.getSnapshot().steeringPending).toBe(false);
    store.close();
  });

  it.each(["pending", "rejected", "unconfirmed"] as const)(
    "keeps the draft on %s without a send fallback or retry",
    async (state) => {
      const { store, fake, steer, composer } = await fixture(state);
      expect(await store.steer("turn-1", "keep my draft")).toBe(false);
      expect(store.getSnapshot().sendError).toContain(
        state === "rejected" ? "did not accept" : "unconfirmed",
      );
      expect(steer).toHaveBeenCalledTimes(1);
      expect(fake.callsOf("send")).toEqual([]);
      expect(composer.send).not.toHaveBeenCalled();
      store.close();
    },
  );

  it("refuses unsupported, obsolete and settled turns before invoking transport", async () => {
    const unsupported = await fixture("accepted", false);
    expect(await unsupported.store.steer("turn-1", "text")).toBe(false);
    expect(unsupported.steer).not.toHaveBeenCalled();
    unsupported.store.close();
    const { store, fake, steer } = await fixture();
    expect(await store.steer("different", "text")).toBe(false);
    fake.lastAttach().onFrame(frame("chat", 0, [ev.turnEnded("turn-1"), ev.turnStarted("turn-2")]));
    expect(await store.steer("turn-1", "text")).toBe(false);
    fake.lastAttach().onFrame(frame("chat", 2, [ev.session({ status: "ready" })]));
    expect(await store.steer("turn-2", "text")).toBe(false);
    expect(steer).not.toHaveBeenCalled();
    store.close();
  });

  it("retains one in-flight request while Stop and late acceptance remain meaningful", async () => {
    const { store, fake, steer } = await fixture();
    let answer: (value: SteeringAck) => void = () => {};
    steer.mockImplementation(
      () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    );
    const first = store.steer("turn-1", "visible");
    expect(store.getSnapshot().steeringPending).toBe(true);
    expect(await store.steer("turn-1", "duplicate")).toBe(false);
    await store.interrupt();
    fake.lastAttach().onFrame(frame("chat", 0, [ev.turnEnded("turn-1")]));
    answer({ message_id: "steer", state: "accepted", detail: null });
    expect(await first).toBe(true);
    expect(steer).toHaveBeenCalledTimes(1);
    expect(fake.callsOf("interrupt")).toEqual([["chat"]]);
    expect(store.getSnapshot().steeringPending).toBe(false);
    store.close();
  });

  it("keeps input on a lost reply and does not resubmit it", async () => {
    const { store, steer } = await fixture();
    steer.mockRejectedValue(new Error("Connection lost; delivery is unconfirmed."));
    expect(await store.steer("turn-1", "keep")).toBe(false);
    expect(store.getSnapshot().sendError).toContain("unconfirmed");
    expect(store.getSnapshot().steeringPending).toBe(false);
    expect(steer).toHaveBeenCalledTimes(1);
    store.close();
  });
});
