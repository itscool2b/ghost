import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  ChatSnapshot,
  GhostEvent,
  HistoryPage,
  PromptAck,
  QueuedPrompt,
  SendingPrompt,
  SessionLists,
} from "../../bindings";
import { CommandError } from "../ipc";
import {
  BLOB_PAGE,
  ChatStore,
  EARLIER_PAGE_BYTES,
  EARLIER_TURNS,
  LISTS_DEBOUNCE_MS,
  OPTION_HOLD_MS,
  type PageVisibility,
  STILL_WORKING_MS,
} from "./chatStore";
import {
  attachResult,
  autonomyState,
  chatSummary,
  ev,
  FakeTransport,
  frame,
  historyPage,
  items,
  lists,
  numbered,
  OK_STORAGE,
  sessionInfo,
  settled,
} from "./testing";
import { applyEntries, emptyTranscript } from "./transcript";

const CHAT = "c1";

/** A store whose first attach is answered with `page` at `nextSeq`. */
async function opened(
  fake: FakeTransport,
  page: HistoryPage = historyPage(CHAT, []),
  nextSeq = 0,
  snapshot: Partial<ChatSnapshot> = {},
): Promise<ChatStore> {
  const store = new ChatStore(CHAT, fake);
  const opening = store.open();
  fake.lastAttach().resolve(attachResult(CHAT, nextSeq, page, snapshot));
  await opening;
  await settled();
  return store;
}

function messageText(store: ChatStore, id: string): string | null {
  const item = store.getSnapshot().transcript.items.get(`message:${id}`);
  return item?.kind === "message" ? item.text : null;
}

describe("joining a chat (the join protocol)", () => {
  it("applies the page, then the frames buffered during the attach from next_seq on", async () => {
    const fake = new FakeTransport();
    const store = new ChatStore(CHAT, fake);
    const opening = store.open();
    expect(store.getSnapshot().phase).toBe("attaching");
    const pending = fake.lastAttach();
    // Frames arrive before the attach answers: one the page already holds, then newer ones.
    pending.onFrame(frame(CHAT, 2, [ev.message("t1.m1", " world")]));
    pending.onFrame(frame(CHAT, 3, [ev.message("t1.m1", "!")]));
    const page = historyPage(
      CHAT,
      items(
        numbered([
          ["t1", ev.turnStarted("t1")],
          ["t1", ev.message("t1.m1", "hello")],
        ]),
      ),
    );
    pending.resolve(attachResult(CHAT, 3, page));
    await opening;
    expect(store.getSnapshot().phase).toBe("live");
    // seq 2 was below next_seq (3): skipped; seq 3 applied.
    expect(messageText(store, "t1.m1")).toBe("hello!");
    // Live frames continue from there.
    pending.onFrame(frame(CHAT, 4, [ev.message("t1.m1", "!")]));
    expect(messageText(store, "t1.m1")).toBe("hello!!");
    expect(fake.methods()).toEqual(["attach", "lists"]);
  });

  it("gap_reattaches: a frame past the expected seq detaches and joins again", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake, historyPage(CHAT, []), 10);
    const first = fake.lastAttach();
    first.onFrame(frame(CHAT, 10, [ev.notice("ok")]));
    first.onFrame(frame(CHAT, 13, [ev.notice("after a gap")]));
    await settled();
    expect(fake.methods()).toEqual(["attach", "lists", "detach", "attach"]);
    expect(fake.callsOf("detach")).toEqual([["sub-c1-10"]]);
    expect(store.getSnapshot().phase).toBe("attaching");
    // The old channel is ignored from now on.
    first.onFrame(frame(CHAT, 11, [ev.notice("late")]));
    fake.lastAttach().resolve(attachResult(CHAT, 14, historyPage(CHAT, [])));
    await settled();
    expect(store.getSnapshot().phase).toBe("live");
    expect(store.getSnapshot().transcript.items.has("notice:11")).toBe(false);
  });

  it("gap_reattaches: the channel ending joins again", async () => {
    const fake = new FakeTransport();
    await opened(fake);
    fake.lastAttach().onEnd();
    await settled();
    expect(fake.methods()).toEqual(["attach", "lists", "detach", "attach"]);
  });

  it("reattach_detaches_first: the old subscription goes before the new attach", async () => {
    const order: string[] = [];
    let release: () => void = () => {};
    const fake = new FakeTransport({
      detach: (id) => {
        order.push(`detach ${id}`);
        return new Promise((resolve) => {
          release = resolve;
        });
      },
    });
    const store = await opened(fake);
    const rejoin = store.reattach();
    await settled();
    expect(fake.attaches).toHaveLength(1);
    release();
    await settled();
    order.push("attach");
    expect(fake.attaches).toHaveLength(2);
    expect(order).toEqual(["detach sub-c1-0", "attach"]);
    fake.lastAttach().resolve(attachResult(CHAT, 0, historyPage(CHAT, [])));
    await rejoin;
  });

  it("a refused attach shows why, without looping, and retries on Retry and storage changes", async () => {
    const fake = new FakeTransport();
    const store = new ChatStore(CHAT, fake);
    const opening = store.open();
    const pending = fake.lastAttach();
    // The shell drops the refused view's channel as well.
    pending.onEnd();
    pending.reject(new CommandError("storage_unavailable", "Ghost's database is not available."));
    await opening;
    await settled();
    expect(store.getSnapshot()).toMatchObject({
      phase: "failed",
      attachError: "Ghost's database is not available.",
    });
    expect(fake.attaches).toHaveLength(1);

    store.storageChanged(OK_STORAGE);
    await settled();
    expect(fake.attaches).toHaveLength(2);
    fake.lastAttach().reject(new CommandError("storage_unavailable", "still not"));
    await settled();
    // The same status again is no change.
    store.storageChanged(OK_STORAGE);
    await settled();
    expect(fake.attaches).toHaveLength(2);

    void store.reattach();
    await settled();
    expect(fake.attaches).toHaveLength(3);
    fake.lastAttach().resolve(attachResult(CHAT, 0, historyPage(CHAT, [])));
    await settled();
    expect(store.getSnapshot().phase).toBe("live");
  });

  it("backs off when joins keep losing their subscription", async () => {
    vi.useFakeTimers();
    try {
      const fake = new FakeTransport({
        attach: (chatId, _onFrame, onEnd) => {
          onEnd();
          return Promise.resolve(attachResult(chatId, 0, historyPage(chatId, [])));
        },
      });
      const store = new ChatStore(CHAT, fake);
      await store.open();
      await vi.advanceTimersByTimeAsync(0);
      // Two at once, then a growing pause instead of a spin.
      expect(fake.callsOf("attach")).toHaveLength(3);
      await vi.advanceTimersByTimeAsync(249);
      expect(fake.callsOf("attach")).toHaveLength(3);
      await vi.advanceTimersByTimeAsync(1);
      expect(fake.callsOf("attach")).toHaveLength(4);
      await vi.advanceTimersByTimeAsync(500);
      expect(fake.callsOf("attach")).toHaveLength(5);
      store.close();
      await vi.advanceTimersByTimeAsync(10_000);
      expect(fake.callsOf("attach")).toHaveLength(5);
    } finally {
      vi.useRealTimers();
    }
  });

  it("closing detaches and ignores whatever still arrives", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake);
    const channel = fake.lastAttach();
    store.close();
    expect(fake.callsOf("detach")).toEqual([["sub-c1-0"]]);
    channel.onFrame(frame(CHAT, 0, [ev.notice("late")]));
    channel.onEnd();
    await settled();
    expect(store.getSnapshot().transcript.segments).toEqual([]);
    expect(fake.attaches).toHaveLength(1);
  });

  it("detaches an attach answered after the view closed", async () => {
    const fake = new FakeTransport();
    const store = new ChatStore(CHAT, fake);
    const opening = store.open();
    store.close();
    fake.lastAttach().resolve(attachResult(CHAT, 5, historyPage(CHAT, [])));
    await opening;
    expect(fake.callsOf("detach")).toEqual([["sub-c1-5"]]);
  });
});

describe("the agent of each turn", () => {
  it("keeps the agent that ran each turn after the chat switches agents, through a reload", async () => {
    const fake = new FakeTransport();
    const page = historyPage(
      CHAT,
      items(
        numbered([
          ["t1", ev.turnStarted("t1")],
          ["t1", ev.turnEnded("t1", "refusal")],
        ]),
      ),
      {
        turn_agents: [{ turn_id: "t1", agent_id: "claude", agent_name: "Claude Code" }],
      },
    );
    // The chat now runs Codex: the failed Claude Code turn keeps its own agent.
    const store = await opened(fake, page, 2, {
      chat: chatSummary(CHAT, { agent_id: "codex", agent_name: "Codex" }),
      session: sessionInfo({ agent_name: "Codex" }),
    });
    expect(store.getSnapshot().turnAgents.get("t1")?.agent_name).toBe("Claude Code");
    // A turn that starts live runs on the chat's agent of that moment.
    fake.lastAttach().onFrame(frame(CHAT, 2, [ev.turnStarted("t2")]));
    expect(store.getSnapshot().turnAgents.get("t2")).toEqual({
      turn_id: "t2",
      agent_id: "codex",
      agent_name: "Codex",
    });
    expect(store.getSnapshot().turnAgents.get("t1")?.agent_id).toBe("claude");
  });

  it("adds the agents an older page names without losing the newer ones", async () => {
    const older = historyPage(CHAT, items(numbered([["t1", ev.turnStarted("t1")]])), {
      turn_agents: [{ turn_id: "t1", agent_id: "claude", agent_name: "Claude Code" }],
    });
    const fake = new FakeTransport({ history: () => Promise.resolve(older) });
    const newest = historyPage(CHAT, items(numbered([["t2", ev.turnStarted("t2")]], 5)), {
      has_more: true,
      before_seq: 5,
      turn_agents: [{ turn_id: "t2", agent_id: "codex", agent_name: "Codex" }],
    });
    const store = await opened(fake, newest, 6);
    await store.loadEarlier();
    const agents = store.getSnapshot().turnAgents;
    expect([...agents.keys()].sort()).toEqual(["t1", "t2"]);
    expect(agents.get("t1")?.agent_name).toBe("Claude Code");
    expect(agents.get("t2")?.agent_name).toBe("Codex");
  });
});

describe("a hidden page", () => {
  /** A page whose visibility the test flips, like a minimized window. */
  function page() {
    let hidden = false;
    const listeners = new Set<() => void>();
    return {
      visibility: {
        hidden: () => hidden,
        onChange: (listener: () => void) => {
          listeners.add(listener);
          return () => listeners.delete(listener);
        },
      } satisfies PageVisibility,
      set(value: boolean) {
        hidden = value;
        for (const listener of listeners) listener();
      },
      watchers: () => listeners.size,
    };
  }

  it("keeps every frame but renders once, in order, when the page shows again", async () => {
    const fake = new FakeTransport();
    const shown = page();
    const store = new ChatStore(CHAT, fake, { visibility: shown.visibility });
    let renders = 0;
    const stop = store.subscribe(() => {
      renders += 1;
    });
    expect(shown.watchers()).toBe(1);
    const opening = store.open();
    const attach = fake.lastAttach();
    attach.resolve(
      attachResult(CHAT, 2, historyPage(CHAT, items(numbered([["t1", ev.turnStarted("t1")]])))),
    );
    await opening;
    await settled();
    const before = renders;
    expect(before).toBeGreaterThan(0);

    shown.set(true);
    for (const [seq, text] of [
      [2, "one "],
      [3, "two "],
      [4, "three"],
    ] as const)
      attach.onFrame(frame(CHAT, seq, [ev.message("t1.m1", text)]));
    attach.onFrame(frame(CHAT, 5, [ev.turnEnded("t1")]));
    // Hidden: nothing rendered, but the state already holds every frame.
    expect(renders).toBe(before);
    expect(messageText(store, "t1.m1")).toBe("one two three");
    expect(store.getSnapshot().openTurnId).toBeNull();

    shown.set(false);
    expect(renders).toBe(before + 1);
    expect(messageText(store, "t1.m1")).toBe("one two three");
    // Showing again with nothing new renders nothing more.
    shown.set(true);
    shown.set(false);
    expect(renders).toBe(before + 1);

    stop();
    expect(shown.watchers()).toBe(0);
    store.close();
  });

  it("renders every change at once while the page is visible", async () => {
    const fake = new FakeTransport();
    const shown = page();
    const store = new ChatStore(CHAT, fake, { visibility: shown.visibility });
    let renders = 0;
    store.subscribe(() => {
      renders += 1;
    });
    const opening = store.open();
    const attach = fake.lastAttach();
    attach.resolve(
      attachResult(CHAT, 2, historyPage(CHAT, items(numbered([["t1", ev.turnStarted("t1")]])))),
    );
    await opening;
    await settled();
    const before = renders;
    attach.onFrame(frame(CHAT, 2, [ev.message("t1.m1", "a")]));
    attach.onFrame(frame(CHAT, 3, [ev.message("t1.m1", "b")]));
    expect(renders).toBe(before + 2);
    store.close();
  });
});

describe("history pages", () => {
  it("live frames do not finish a pending older-page restore", async () => {
    const older = historyPage(CHAT, items(numbered([[null, ev.notice("older")]])), {
      has_more: false,
      before_seq: 0,
    });
    let answer: (page: typeof older) => void = () => {};
    const fake = new FakeTransport({
      history: () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    const newest = historyPage(CHAT, [], { has_more: true, before_seq: 1 });
    const store = await opened(fake, newest, 1);
    const loading = store.loadEarlier();
    fake.lastAttach().onFrame(frame(CHAT, 1, [ev.notice("live while loading")]));
    expect(store.getSnapshot()).toMatchObject({ loadingEarlier: true, historyRevision: 0 });
    answer(older);
    await loading;
    expect(store.getSnapshot()).toMatchObject({ loadingEarlier: false, historyRevision: 1 });
    expect(store.getSnapshot().transcript.items.size).toBe(2);
    store.close();
  });

  it("partial_turn_page: a cut page starts inside its turn, and Load earlier continues it", async () => {
    const whole = numbered([
      ["t1", ev.turnStarted("t1")],
      ["t1", ev.user("t1.u", "Long")],
      ["t1", ev.message("t1.m1", "one ")],
      ["t1", ev.message("t1.m1", "two")],
      ["t1", ev.turnEnded("t1")],
    ]);
    const older = historyPage(CHAT, items(whole.slice(0, 2)), { has_more: false, before_seq: 0 });
    const fake = new FakeTransport({ history: () => Promise.resolve(older) });
    const newest = historyPage(CHAT, items(whole.slice(2)), {
      has_more: true,
      before_seq: 2,
      cut_turn_id: "t1",
    });
    const store = await opened(fake, newest, 5);
    let state = store.getSnapshot();
    expect(state.hasMore).toBe(true);
    expect(state.historyRevision).toBe(0);
    expect(state.transcript.segments[0]).toMatchObject({ key: "turn:t1", partial: true });

    await store.loadEarlier();
    expect(fake.callsOf("history")).toEqual([[CHAT, 2]]);
    state = store.getSnapshot();
    expect(state.hasMore).toBe(false);
    expect(state.historyRevision).toBe(1);
    expect(state.transcript.segments).toHaveLength(1);
    expect(state.transcript.segments[0]).toMatchObject({ key: "turn:t1", partial: false });
    expect(state.transcript.segments[0]?.itemKeys).toEqual(["user:t1.u", "message:t1.m1"]);
    expect(messageText(store, "t1.m1")).toBe("one two");
    await store.loadEarlier();
    expect(fake.callsOf("history")).toHaveLength(1);
  });

  it("Load earlier brings a chunk of turns at once, through a turn cut across many pages", async () => {
    // Seqs 0..: EARLIER_TURNS + 5 short turns, then turn "big" whose 300 tool updates the
    // history hands over 100 at a time, newest first.
    const short = EARLIER_TURNS + 5;
    const all = numbered([
      ...Array.from(
        { length: short },
        (_, i) =>
          [
            [`s${i}`, ev.turnStarted(`s${i}`)],
            [`s${i}`, ev.message(`s${i}.m`, `turn ${i}`)],
            [`s${i}`, ev.turnEnded(`s${i}`)],
          ] as const,
      ).flat(),
      ["big", ev.turnStarted("big")],
      ["big", ev.toolCall("sh", { kind: "execute" })],
      ...Array.from(
        { length: 300 },
        (_, i) =>
          [
            "big",
            ev.toolUpdate("sh", {
              content: [{ type: "terminal", output: `${i}`, exit_code: null, truncated: null }],
            }),
          ] as const,
      ),
      ["big", ev.turnEnded("big")],
    ]);
    const bigStart = short * 3;
    // The page below each cursor: inside "big", 100 rows; below it, three short turns at a time.
    const below = (cursor: number): HistoryPage => {
      const lo = cursor > bigStart ? Math.max(bigStart, cursor - 100) : Math.max(0, cursor - 9);
      return historyPage(CHAT, items(all.slice(lo, cursor)), {
        has_more: lo > 0,
        before_seq: lo,
        cut_turn_id: cursor > bigStart && lo > bigStart ? "big" : null,
      });
    };
    const fake = new FakeTransport({ history: (_chat, before) => Promise.resolve(below(before)) });
    const newest = below(all.length);
    const store = await opened(fake, newest, all.length);
    expect(store.getSnapshot().transcript.segments[0]).toMatchObject({ partial: true });

    await store.loadEarlier();
    // Through the rest of "big" (two pages and its start), then short turns until enough started.
    const calls = fake.callsOf("history");
    expect(calls.length).toBeGreaterThan(3);
    let state = store.getSnapshot();
    expect(state.historyRevision).toBe(1);
    expect(state.hasMore).toBe(true);
    const turns = state.transcript.segments.filter((s) => s.turnId !== null);
    expect(turns.length).toBeGreaterThanOrEqual(EARLIER_TURNS + 1);
    expect(turns.at(-1)).toMatchObject({ turnId: "big", partial: false });

    await store.loadEarlier();
    state = store.getSnapshot();
    expect(state.hasMore).toBe(false);
    expect(state.historyRevision).toBe(2);
    expect(state.transcript).toEqual(applyEntries(emptyTranscript(), all));
  });

  it("Load earlier asks for the largest pages", async () => {
    const asked: unknown[] = [];
    const fake = new FakeTransport({
      history: (chatId, _before, maxBytes) => {
        asked.push(maxBytes);
        return Promise.resolve(historyPage(chatId, [], { has_more: false, before_seq: 0 }));
      },
    });
    const store = await opened(fake, historyPage(CHAT, [], { has_more: true, before_seq: 3 }), 3);
    await store.loadEarlier();
    expect(asked).toEqual([EARLIER_PAGE_BYTES]);
  });

  it("keeps the pages fetched before one fails, and Load earlier goes on from there", async () => {
    let fail = false;
    const turn = (n: number) =>
      numbered(
        [
          [`t${n}`, ev.turnStarted(`t${n}`)],
          [`t${n}`, ev.turnEnded(`t${n}`)],
        ],
        n * 2,
      );
    const fake = new FakeTransport({
      history: (_chat, before) => {
        if (fail) return Promise.reject(new Error("disk busy"));
        fail = true;
        const n = before / 2 - 1;
        return Promise.resolve(
          historyPage(CHAT, items(turn(n)), { has_more: true, before_seq: n * 2 }),
        );
      },
    });
    const store = await opened(
      fake,
      historyPage(CHAT, items(turn(9)), { has_more: true, before_seq: 18 }),
      20,
    );
    await store.loadEarlier();
    const state = store.getSnapshot();
    expect(state.earlierError).toBe("disk busy");
    expect(state.hasMore).toBe(true);
    expect(state.transcript.segments.map((s) => s.turnId)).toEqual(["t8", "t9"]);
    expect(fake.callsOf("history")).toEqual([
      [CHAT, 18],
      [CHAT, 16],
    ]);
  });

  it("unsaved_marker_and_reattach: the gap is marked, and the view joins again once saving resumes", async () => {
    const fake = new FakeTransport();
    const page = historyPage(CHAT, items(numbered([[null, ev.notice("saved")]])), {
      unsaved_from: 1,
    });
    const paused = {
      state: { state: "paused" as const, reason: "disk_full" as const, since: 1 },
      unsaved_bytes: 300,
    };
    const store = await opened(fake, page, 6, { storage: paused });
    fake.lastAttach().onFrame(frame(CHAT, 6, [ev.notice("live")]));
    const state = store.getSnapshot();
    expect(state.unsavedFrom).toBe(1);
    expect(state.transcript.segments[0]?.itemKeys).toEqual(["notice:0", "gap:1", "notice:6"]);

    store.storageChanged(paused);
    await settled();
    expect(fake.attaches).toHaveLength(1);
    store.storageChanged(OK_STORAGE);
    await settled();
    expect(fake.methods().slice(-2)).toEqual(["detach", "attach"]);
  });
});

describe("the session's live state", () => {
  it.each(["before", "after"])(
    "keeps canonical prompt identity when acknowledgments arrive %s frames",
    async (ackOrder) => {
      const answers: ((ack: PromptAck) => void)[] = [];
      const fake = new FakeTransport({
        send: () => new Promise((resolve) => answers.push(resolve)),
      });
      const store = await opened(fake);
      const channel = fake.lastAttach();
      const prefix = "🐢 identical prefix ".repeat(30);
      const texts = Array.from({ length: 12 }, (_, index) => `${prefix} message ${index}`);
      const pending = texts.map((text) => store.send(text));
      const acknowledge = async () => {
        answers.forEach((resolve, index) => {
          resolve({ prompt_id: `p${index}`, queued: true });
        });
        expect(await Promise.all(pending)).toEqual(texts.map(() => true));
      };
      if (ackOrder === "before") await acknowledge();
      let seq = 0;
      const emit = (events: GhostEvent[]) => {
        channel.onFrame(frame(CHAT, seq, events));
        seq += events.length;
      };
      emit(texts.map((text, index) => ev.lifecycle(`p${index}`, "queued", text.slice(0, 256))));
      for (const index of [10, 0, 11, 3]) {
        const sending: SendingPrompt = {
          prompt_id: `p${index}`,
          text: texts[index] ?? "",
          parts: [],
          created_at: index + 1,
        };
        emit([
          ev.lifecycle(sending.prompt_id, "sending", prefix.slice(0, 256)),
          ev.session({ status: "running", sending }),
        ]);
        expect(store.getSnapshot().sending).toEqual(sending);
        expect(store.getSnapshot().queue.some((row) => row.prompt_id === sending.prompt_id)).toBe(
          false,
        );
        // The canonical turn ID clears only its own pending message before the lifecycle acknowledgment.
        emit([{ type: "turn_started", turn_id: `t${index}`, prompt_id: sending.prompt_id }]);
        expect(store.getSnapshot().sending).toBeNull();
        emit([ev.lifecycle(sending.prompt_id, "started"), ev.session({ status: "running" })]);
      }
      if (ackOrder === "after") await acknowledge();
      expect(store.getSnapshot().sending).toBeNull();
      store.close();
    },
  );

  it("reattaches with exact attachments and text without a local send cache", async () => {
    const sending: SendingPrompt = {
      prompt_id: "attachment-only",
      text: "",
      created_at: 123,
      parts: [
        { type: "image", blob: "a".repeat(64), mime: "image/png" },
        { type: "mention", path: "src/main.rs" },
      ],
    };
    const fake = new FakeTransport();
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "running", sending }),
    });
    expect(store.getSnapshot().sending).toEqual(sending);
    const changed = { ...sending, text: `${"long body 🐢 ".repeat(100)}end` };
    fake
      .lastAttach()
      .onFrame(frame(CHAT, 0, [ev.session({ status: "running", sending: changed })]));
    expect(store.getSnapshot().sending).toEqual(changed);
    fake
      .lastAttach()
      .onFrame(
        frame(CHAT, 1, [{ type: "turn_started", turn_id: "other", prompt_id: "another-prompt" }]),
      );
    expect(store.getSnapshot().sending).toEqual(changed);
    store.close();
  });

  it("follows the queue through the prompt lifecycle and marks the prompts a stop holds", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake);
    const channel = fake.lastAttach();
    // As the actor flushes: each flush ends with the session's update when it changed.
    channel.onFrame(frame(CHAT, 0, [ev.session({ status: "running" })]));
    channel.onFrame(
      frame(CHAT, 1, [ev.lifecycle("p2", "queued", "second"), ev.session({ status: "running" })]),
    );
    channel.onFrame(
      frame(CHAT, 3, [ev.lifecycle("p3", "queued", "third"), ev.session({ status: "running" })]),
    );
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.preview, row.held])).toEqual([
      ["p2", "second", false],
      ["p3", "third", false],
    ]);
    // A stop holds every prompt queued until now; one typed afterwards is not held.
    channel.onFrame(frame(CHAT, 5, [ev.session({ status: "cancelling", queue_held: true })]));
    channel.onFrame(
      frame(CHAT, 6, [
        ev.lifecycle("p4", "queued", "fourth"),
        ev.lifecycle("p2", "cancelled"),
        ev.session({ status: "cancelling", queue_held: true }),
      ]),
    );
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["p3", true],
      ["p4", false],
    ]);
    // Send queued releases them.
    channel.onFrame(frame(CHAT, 9, [ev.session({ status: "ready", queue_held: false })]));
    expect(store.getSnapshot().queue.every((row) => !row.held)).toBe(true);
    channel.onFrame(frame(CHAT, 10, [ev.lifecycle("p3", "started"), ev.usage(0.5), ev.limits()]));
    expect(store.getSnapshot().queue.map((row) => row.prompt_id)).toEqual(["p4"]);
    expect(store.getSnapshot().usage?.cost?.amount).toBe(0.5);
    expect(store.getSnapshot().limits).toEqual([]);
    await settled();
    expect(fake.methods(), "every hold was placed without joining again").toEqual([
      "attach",
      "lists",
    ]);
  });

  it("shows a message whose checkpoint saves as sending, in full, never as queued", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake);
    const channel = fake.lastAttach();
    const long = `${"word ".repeat(80)}end`;
    const sent = store.send(long);
    // The frame can come before the send's answer: the session carries the exact persisted body.
    const sending = {
      prompt_id: "p1",
      text: long,
      parts: [],
      created_at: 1,
    };
    channel.onFrame(
      frame(CHAT, 0, [
        ev.lifecycle("p1", "sending", long.slice(0, 256)),
        ev.session({ status: "running", sending }),
      ]),
    );
    expect(await sent).toBe(true);
    expect(store.getSnapshot().queue).toEqual([]);
    expect(store.getSnapshot().sending).toEqual(sending);
    // The turn starts: the message is the turn's now.
    channel.onFrame(
      frame(CHAT, 2, [ev.lifecycle("p1", "started"), ev.session({ status: "running" })]),
    );
    expect(store.getSnapshot().sending).toBeNull();
    expect(store.getSnapshot().queue).toEqual([]);
    await settled();
    expect(fake.methods()).not.toContain("detach");
  });

  it("joins again when a Stop during the checkpoint puts the sending message back, held", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake);
    const channel = fake.lastAttach();
    const sending = { prompt_id: "p1", text: "hello", parts: [], created_at: 1 };
    channel.onFrame(
      frame(CHAT, 0, [
        ev.lifecycle("p1", "sending", "hello"),
        ev.session({ status: "running", sending }),
      ]),
    );
    expect(store.getSnapshot().sending?.text).toBe("hello");
    channel.onFrame(
      frame(CHAT, 2, [
        ev.session({
          status: "ready",
          queue_held: true,
          queue_wait: { reason: "held", after: "stop" },
        }),
      ]),
    );
    expect(store.getSnapshot().sending).toBeNull();
    await settled();
    expect(fake.methods().slice(-2)).toEqual(["detach", "attach"]);
    fake.lastAttach().resolve(
      attachResult(CHAT, 3, historyPage(CHAT, []), {
        session: sessionInfo({ status: "ready", queue_held: true }),
        queue: [{ prompt_id: sending.prompt_id, preview: sending.text, held: true, created_at: 1 }],
      }),
    );
    await settled();
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["p1", true],
    ]);
  });

  it("returns an authoritatively held Sending row to the queue head without losing followers", async () => {
    const fake = new FakeTransport();
    const sending = { prompt_id: "p1", text: "capturing", parts: [], created_at: 1 };
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "ready", sending }),
      queue: [{ prompt_id: "p2", preview: "follows", held: false, created_at: 2 }],
    });
    fake
      .lastAttach()
      .onFrame(
        frame(CHAT, 0, [
          ev.lifecycle("p1", "queued", "capturing", true),
          ev.lifecycle("p2", "queued", "follows", true),
          ev.session({ status: "ready", queue_held: true, sending: null }),
        ]),
      );
    await settled();
    expect(
      store.getSnapshot().queue.map((row) => [row.prompt_id, row.held, row.created_at]),
    ).toEqual([
      ["p1", true, 1],
      ["p2", true, 2],
    ]);
    expect(store.getSnapshot().sending).toBeNull();
    expect(fake.callsOf("attach")).toHaveLength(1);
    store.close();
  });

  it("shows a message already on its way when the window joins", async () => {
    const fake = new FakeTransport();
    const sending = { prompt_id: "p1", text: "fix the tests", parts: [], created_at: 1 };
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "running", sending }),
    });
    expect(store.getSnapshot().sending).toEqual(sending);
    expect(store.getSnapshot().queue).toEqual([]);
  });

  it("restores the unheld queue when background work delays dispatch after capture", async () => {
    const fake = new FakeTransport();
    const sending: SendingPrompt = {
      prompt_id: "attachment-only",
      text: "",
      parts: [{ type: "image", blob: "a".repeat(64), mime: "image/png" }],
      created_at: 123,
    };
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "ready", sending }),
    });
    fake.lastAttach().onFrame(frame(CHAT, 0, [ev.session({ status: "ready", queue_held: false })]));
    expect(store.getSnapshot().sending).toBeNull();
    await settled();
    expect(fake.methods().slice(-2)).toEqual(["detach", "attach"]);
    const queued = {
      prompt_id: sending.prompt_id,
      preview: "[image]",
      held: false,
      created_at: sending.created_at,
    };
    fake.lastAttach().resolve(
      attachResult(CHAT, 1, historyPage(CHAT, []), {
        session: sessionInfo({ status: "ready", queue_held: false }),
        queue: [queued],
      }),
    );
    await settled();
    expect(store.getSnapshot().queue).toEqual([queued]);
    expect(store.getSnapshot().sending).toBeNull();
    store.close();
  });

  it.each(["failed", "interrupted"] as const)(
    "keeps authoritative queued holds during %s setup without rejoining or duplicating IDs",
    async (setupState) => {
      const fake = new FakeTransport();
      const setup = {
        worktree_id: "tree",
        generation: 1,
        state: setupState,
        detail: "Retry setup.",
      };
      const session = sessionInfo({
        status: "stopped",
        queue_held: true,
        worktree_setup: setup,
      });
      const store = await opened(
        fake,
        historyPage(CHAT, [], { has_more: true, before_seq: 17 }),
        0,
        {
          session,
          queue: [{ prompt_id: "a", preview: "older", held: true, created_at: 1 }],
        },
      );
      const channel = fake.lastAttach();
      channel.onFrame(
        frame(CHAT, 0, [ev.lifecycle("b", "queued", "new held", true), ev.session(session)]),
      );
      expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
        ["a", true],
        ["b", true],
      ]);
      // A duplicate old frame cannot replace a newer row's authoritative hold or preview.
      channel.onFrame(frame(CHAT, 0, [ev.lifecycle("b", "queued", "stale", false)]));
      channel.onFrame(
        frame(CHAT, 2, [ev.lifecycle("b", "queued", "same identity", true), ev.session(session)]),
      );
      const succeeded = sessionInfo({
        status: "ready",
        queue_held: true,
        worktree_setup: { ...setup, generation: 2, state: "succeeded" },
      });
      // Queue truth wins at a setup boundary, independent of either session's setup policy.
      channel.onFrame(
        frame(CHAT, 4, [ev.lifecycle("c", "queued", "after setup", false), ev.session(succeeded)]),
      );
      channel.onFrame(
        frame(CHAT, 6, [
          ev.lifecycle("d", "queued", "before setup finished", true),
          ev.session(succeeded),
        ]),
      );
      await settled();
      expect(
        store.getSnapshot().queue.map((row) => [row.prompt_id, row.preview, row.held]),
      ).toEqual([
        ["a", "older", true],
        ["b", "same identity", true],
        ["c", "after setup", false],
        ["d", "before setup finished", true],
      ]);
      expect(store.getSnapshot().hasMore).toBe(true);
      expect(store.getSnapshot().historyRevision).toBe(0);
      expect(fake.callsOf("attach")).toHaveLength(1);
      store.close();
    },
  );

  it("updates existing mixed queue holds by stable ID when a setup retry fails", async () => {
    const fake = new FakeTransport();
    const setup = { worktree_id: "tree", generation: 2, state: "running" as const, detail: null };
    const session = sessionInfo({ status: "stopped", queue_held: true, worktree_setup: setup });
    const queue = [
      { prompt_id: "a", preview: "held from first failure", held: true, created_at: 1 },
      { prompt_id: "b", preview: "during retry", held: false, created_at: 2 },
      { prompt_id: "c", preview: "also during retry", held: false, created_at: 3 },
    ];
    const store = await opened(fake, historyPage(CHAT, [], { has_more: true, before_seq: 17 }), 0, {
      session,
      queue,
    });
    const failed = { ...session, worktree_setup: { ...setup, state: "failed" as const } };
    const channel = fake.lastAttach();
    channel.onFrame(
      frame(CHAT, 0, [ev.lifecycle("b", "queued", "during retry", true), ev.session(failed)]),
    );
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["a", true],
      ["b", true],
      ["c", false],
    ]);
    channel.onFrame(
      frame(CHAT, 2, [ev.lifecycle("c", "queued", "also during retry", true), ev.session(failed)]),
    );
    await settled();
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["a", true],
      ["b", true],
      ["c", true],
    ]);
    expect(fake.callsOf("attach")).toHaveLength(1);
    expect(store.getSnapshot().hasMore).toBe(true);
    expect(store.getSnapshot().queue.map((row) => row.created_at)).toEqual([1, 2, 3]);
    // A later attach restores the same holds and order from the authoritative snapshot.
    const rejoin = store.reattach();
    await settled();
    fake.lastAttach().resolve(
      attachResult(CHAT, 4, historyPage(CHAT, []), {
        session: failed,
        queue: queue.map((row) => ({ ...row, held: true })),
      }),
    );
    await rejoin;
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["a", true],
      ["b", true],
      ["c", true],
    ]);
    store.close();
  });

  it("joins again for the exact queue when an error holds prompts while others were held", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "running", queue_held: true }),
      queue: [{ prompt_id: "a", preview: "older", held: true, created_at: 1 }],
    });
    const channel = fake.lastAttach();
    // A prompt typed during the turn waits behind it, unheld...
    channel.onFrame(
      frame(CHAT, 0, [
        ev.lifecycle("c", "queued", "typed during the turn"),
        ev.session({ status: "running", queue_held: true }),
      ]),
    );
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["a", true],
      ["c", false],
    ]);
    // ...until the turn ends with an error, which holds it too: queue_held was true already, so
    // nothing in the frames says the typed prompt is held now.
    channel.onFrame(
      frame(CHAT, 2, [
        ev.error("agent_error", "The agent failed."),
        ev.session({ status: "ready", queue_held: true }),
      ]),
    );
    await settled();
    expect(fake.methods().slice(-2)).toEqual(["detach", "attach"]);
    expect(store.getSnapshot().session?.queue_held, "Send queued shows meanwhile").toBe(true);
    fake.lastAttach().resolve(
      attachResult(CHAT, 4, historyPage(CHAT, []), {
        session: sessionInfo({ status: "ready", queue_held: true }),
        queue: [
          { prompt_id: "a", preview: "older", held: true, created_at: 1 },
          { prompt_id: "c", preview: "typed during the turn", held: true, created_at: 2 },
        ],
      }),
    );
    await settled();
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["a", true],
      ["c", true],
    ]);
    // With the older one removed, the typed prompt still shows held and Send queued still shows.
    fake
      .lastAttach()
      .onFrame(
        frame(CHAT, 4, [
          ev.lifecycle("a", "cancelled"),
          ev.session({ status: "ready", queue_held: true }),
        ]),
      );
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["c", true],
    ]);
    expect(store.getSnapshot().session?.queue_held).toBe(true);
    await settled();
    expect(fake.callsOf("attach")).toHaveLength(2);
  });

  it("joins again when a prompt was queued as the hold ran", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "running" }),
    });
    const channel = fake.lastAttach();
    channel.onFrame(
      frame(CHAT, 0, [ev.lifecycle("p1", "queued", "first"), ev.session({ status: "running" })]),
    );
    // The flush that reports the stop also carries a prompt typed in the same moment: it may
    // have come before the hold or after it.
    channel.onFrame(
      frame(CHAT, 2, [
        ev.lifecycle("p2", "queued", "second"),
        ev.session({ status: "cancelling", queue_held: true }),
      ]),
    );
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["p1", true],
      ["p2", false],
    ]);
    await settled();
    expect(fake.methods().slice(-2)).toEqual(["detach", "attach"]);
  });

  it("marks nothing held for a start that fails, which holds nothing", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "stopped", queue_held: true }),
      queue: [{ prompt_id: "a", preview: "held at the restart", held: true, created_at: 1 }],
    });
    const channel = fake.lastAttach();
    // A prompt sent to a stopped chat starts it; the start fails.
    channel.onFrame(
      frame(CHAT, 0, [
        ev.lifecycle("b", "queued", "new"),
        ev.session({ status: "starting", queue_held: true }),
      ]),
    );
    channel.onFrame(
      frame(CHAT, 2, [
        ev.error("spawn_failed", "Could not start the agent."),
        ev.session({ status: "failed", queue_held: true }),
      ]),
    );
    expect(store.getSnapshot().queue.map((row) => [row.prompt_id, row.held])).toEqual([
      ["a", true],
      ["b", false],
    ]);
    await settled();
    expect(fake.methods()).toEqual(["attach", "lists"]);
  });

  it("lists_refetch_on_revision: a new revision refetches the lists, debounced", async () => {
    vi.useFakeTimers();
    try {
      let revision = 1;
      const fake = new FakeTransport({ lists: () => Promise.resolve(lists(revision)) });
      const store = await opened(fake, historyPage(CHAT, []), 0, {
        session: sessionInfo({ revision: 1 }),
      });
      await vi.advanceTimersByTimeAsync(0);
      expect(fake.callsOf("lists")).toHaveLength(1);
      const channel = fake.lastAttach();
      revision = 3;
      channel.onFrame(frame(CHAT, 0, [ev.session({ revision: 2 })]));
      await vi.advanceTimersByTimeAsync(LISTS_DEBOUNCE_MS - 50);
      channel.onFrame(frame(CHAT, 1, [ev.session({ revision: 3 })]));
      channel.onFrame(frame(CHAT, 2, [ev.session({ revision: 3, status: "running" })]));
      await vi.advanceTimersByTimeAsync(LISTS_DEBOUNCE_MS - 1);
      expect(fake.callsOf("lists")).toHaveLength(1);
      await vi.advanceTimersByTimeAsync(1);
      expect(fake.callsOf("lists")).toHaveLength(2);
      expect(store.getSnapshot().lists?.revision).toBe(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it("auth_card_lists_commands: entering auth_required joins again for the sign-in commands", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "running" }),
    });
    fake.lastAttach().onFrame(frame(CHAT, 0, [ev.session({ status: "stopping" })]));
    fake
      .lastAttach()
      .onFrame(frame(CHAT, 1, [ev.session({ status: "auth_required", queue_held: true })]));
    await settled();
    expect(fake.methods().slice(-2)).toEqual(["detach", "attach"]);
    const auth = {
      message: "Codex needs you to sign in.",
      commands: [
        {
          label: "Sign in with ChatGPT",
          argv: ["codex", "login"],
          env: [["CODEX_HOME", "/tmp/x"] as [string, string]],
        },
      ],
    };
    fake.lastAttach().resolve(
      attachResult(CHAT, 2, historyPage(CHAT, []), {
        session: sessionInfo({ status: "auth_required", queue_held: true }),
        auth,
      }),
    );
    await settled();
    expect(store.getSnapshot().auth).toEqual(auth);
    // Already fresh: another auth_required update joins no more.
    fake
      .lastAttach()
      .onFrame(frame(CHAT, 2, [ev.session({ status: "auth_required", revision: 2 })]));
    await settled();
    expect(fake.attaches).toHaveLength(2);
    // Leaving auth_required drops the prompt.
    fake.lastAttach().onFrame(frame(CHAT, 3, [ev.session({ status: "starting" })]));
    expect(store.getSnapshot().auth).toBeNull();
  });
});

describe("the pickers", () => {
  it("set an option or the autonomy and show the lists the shell answers, or why not", async () => {
    const fake = new FakeTransport({
      setOption: (_chat, optionId) =>
        optionId === "bad"
          ? Promise.reject(new CommandError("option_invalid", "The agent refused that value."))
          : Promise.resolve(lists(7)),
      setAutonomy: () =>
        Promise.resolve(
          lists(8, {
            autonomy: {
              requested: "ask",
              native_mode: "default",
              auto_answer: { type: "never" },
              note: "Applies when the agent starts.",
            },
          }),
        ),
    });
    const store = await opened(fake);
    await store.setOption("model", { type: "choice", value: "smart" });
    expect(store.getSnapshot().lists?.revision).toBe(7);
    await store.setOption("bad", { type: "toggle", value: true });
    // A refused option change says why in the model menu; the access picker's error is untouched.
    expect(store.getSnapshot().optionError).toBe("The agent refused that value.");
    expect(store.getSnapshot().listsError).toBeNull();
    expect(store.getSnapshot().lists?.revision).toBe(7);
    await store.setAutonomy("ask");
    expect(store.getSnapshot().lists?.autonomy.note).toBe("Applies when the agent starts.");
    expect(store.getSnapshot().listsError).toBeNull();
    expect(fake.calls.slice(2).map((call) => [call.method, ...call.args])).toEqual([
      ["setOption", CHAT, "model", { type: "choice", value: "smart" }],
      ["setOption", CHAT, "bad", { type: "toggle", value: true }],
      ["setAutonomy", CHAT, "ask"],
    ]);
  });

  it("keep the newest change's lists when an older answer arrives last, and show what is on its way", async () => {
    const answers = new Map<string, (answer: SessionLists) => void>();
    const fake = new FakeTransport({
      setOption: (_chat, _option, value) =>
        new Promise((resolve) => {
          answers.set(value.type === "choice" ? value.value : String(value.value), resolve);
        }),
      setAutonomy: (_chat, autonomy) => new Promise((resolve) => answers.set(autonomy, resolve)),
    });
    const store = await opened(fake);
    const fast = store.setOption("model", { type: "choice", value: "fast" });
    const smart = store.setOption("model", { type: "choice", value: "smart" });
    // The picker shows the newest value asked for while the calls run.
    expect(store.getSnapshot().optionsSaving.get("model")).toEqual({
      type: "choice",
      value: "smart",
    });
    answers.get("smart")?.(lists(11));
    await smart;
    expect(store.getSnapshot().lists?.revision).toBe(11);
    expect(store.getSnapshot().optionsSaving.size).toBe(0);
    // The first change answers last: its lists are older, so they are ignored.
    answers.get("fast")?.(lists(10));
    await fast;
    expect(store.getSnapshot().lists?.revision).toBe(11);
    expect(store.getSnapshot().optionsSaving.size).toBe(0);

    // The same for the autonomy; a refusal clears what was on its way and says why.
    const ask = store.setAutonomy("ask");
    expect(store.getSnapshot().autonomySaving).toBe("ask");
    answers.get("ask")?.(lists(12, { autonomy: autonomyState("ask") }));
    await ask;
    expect(store.getSnapshot()).toMatchObject({ autonomySaving: null, listsError: null });
    expect(store.getSnapshot().lists?.autonomy.requested).toBe("ask");
    fake.handlers.setOption = () =>
      Promise.reject(
        new CommandError("session_not_ready", "The agent isn't running in this chat."),
      );
    await store.setOption("model", { type: "choice", value: "fast" });
    expect(store.getSnapshot()).toMatchObject({
      optionError: "The agent isn't running in this chat.",
    });
    expect(store.getSnapshot().optionsSaving.size).toBe(0);
    expect(store.getSnapshot().lists?.revision).toBe(12);
  });

  it("keep the newest lists when an older refetch answers late", async () => {
    let answer: (value: ReturnType<typeof lists>) => void = () => {};
    let calls = 0;
    const fake = new FakeTransport({
      lists: () => {
        calls += 1;
        if (calls === 1) return new Promise((resolve) => (answer = resolve));
        return Promise.resolve(lists(1));
      },
      setOption: () => Promise.resolve(lists(9)),
    });
    const store = await opened(fake);
    await store.setOption("model", { type: "choice", value: "fast" });
    answer(lists(2));
    await settled();
    expect(store.getSnapshot().lists?.revision).toBe(9);
  });

  it("keep a change in view when its answer's lists do not show it yet, until the refetch", async () => {
    vi.useFakeTimers();
    try {
      const model = (current: string) => ({
        id: "model",
        name: "Model",
        description: null,
        category: "model" as const,
        kind: {
          type: "select" as const,
          current,
          choices: ["fast", "smart"].map((value) => ({
            value,
            name: value,
            description: null,
            group: null,
          })),
        },
        pending: null,
      });
      let refetched = lists(8, { options: [model("smart")] });
      const fake = new FakeTransport({
        lists: () => Promise.resolve(refetched),
        // A driver answers before its lists arrive: the answer still names the old model.
        setOption: () => Promise.resolve(lists(7, { options: [model("fast")] })),
      });
      const store = await opened(fake, historyPage(CHAT, []), 0, {
        session: sessionInfo({ revision: 7 }),
      });
      await vi.advanceTimersByTimeAsync(0);
      await store.setOption("model", { type: "choice", value: "smart" });
      // The picker keeps showing the model asked for instead of snapping back.
      expect(store.getSnapshot().optionsSaving.get("model")).toEqual({
        type: "choice",
        value: "smart",
      });
      fake.lastAttach().onFrame(frame(CHAT, 0, [ev.session({ revision: 8 })]));
      await vi.advanceTimersByTimeAsync(LISTS_DEBOUNCE_MS);
      expect(store.getSnapshot().optionsSaving.size).toBe(0);
      expect(store.getSnapshot().lists?.revision).toBe(8);
      // Without a refetch, the answer's own lists show after a moment.
      refetched = lists(9, { options: [model("smart")] });
      await store.setOption("model", { type: "choice", value: "smart" });
      expect(store.getSnapshot().optionsSaving.has("model")).toBe(true);
      await vi.advanceTimersByTimeAsync(OPTION_HOLD_MS);
      expect(store.getSnapshot().optionsSaving.size).toBe(0);
      // An answer that shows the change ends the wait at once.
      fake.handlers.setOption = () => Promise.resolve(lists(10, { options: [model("fast")] }));
      await store.setOption("model", { type: "choice", value: "fast" });
      expect(store.getSnapshot().optionsSaving.size).toBe(0);
      expect(store.getSnapshot().optionError).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("history errors", () => {
  it("shows why an older page could not load, and Load earlier tries again", async () => {
    let fail = true;
    const fake = new FakeTransport({
      history: () =>
        fail
          ? Promise.reject(
              new CommandError("storage_unavailable", "Ghost's database is not available: busy"),
            )
          : Promise.resolve(historyPage(CHAT, [], { has_more: false })),
    });
    const store = await opened(fake, historyPage(CHAT, [], { has_more: true, before_seq: 10 }), 20);
    await store.loadEarlier();
    expect(store.getSnapshot()).toMatchObject({
      loadingEarlier: false,
      earlierError: "Ghost's database is not available: busy",
      historyRevision: 0,
      hasMore: true,
    });
    fail = false;
    await store.loadEarlier();
    expect(store.getSnapshot()).toMatchObject({
      earlierError: null,
      hasMore: false,
      historyRevision: 1,
    });
  });
});

describe("prompts and the session", () => {
  it("sends text as typed, refuses blank text, and keeps a failure for the composer", async () => {
    const fake = new FakeTransport({
      send: (_chat, text) =>
        text === "too much"
          ? Promise.reject(
              new CommandError("queue_full", "20 messages are already waiting in this chat."),
            )
          : Promise.resolve({ prompt_id: "p", queued: false }),
    });
    const store = await opened(fake);
    expect(await store.send("   \n")).toBe(false);
    expect(fake.callsOf("send")).toEqual([]);
    expect(await store.send("  keep my spaces  ")).toBe(true);
    expect(fake.callsOf("send")).toEqual([[CHAT, "  keep my spaces  "]]);
    expect(await store.send("too much")).toBe(false);
    expect(store.getSnapshot().sendError).toBe("20 messages are already waiting in this chat.");
    expect(await store.send("fine")).toBe(true);
    expect(store.getSnapshot().sendError).toBeNull();
  });

  it("escape_twice: Esc interrupts a running turn, and the next Esc force-stops", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      session: sessionInfo({ status: "running" }),
    });
    await store.escape();
    await store.escape();
    expect(fake.methods().filter((m) => m === "interrupt" || m === "forceStop")).toEqual([
      "interrupt",
      "forceStop",
    ]);
    // While the agent cancels, Esc force-stops; with no turn, Esc does nothing.
    fake.lastAttach().onFrame(frame(CHAT, 0, [ev.session({ status: "cancelling" })]));
    await store.escape();
    fake.lastAttach().onFrame(frame(CHAT, 1, [ev.session({ status: "ready" })]));
    expect(store.escape()).toBeNull();
    expect(fake.methods().filter((m) => m === "interrupt" || m === "forceStop")).toEqual([
      "interrupt",
      "forceStop",
      "forceStop",
    ]);
    // A start is interrupted (the shell treats it as a force stop).
    fake.lastAttach().onFrame(frame(CHAT, 2, [ev.session({ status: "starting" })]));
    await store.escape();
    expect(fake.methods().at(-1)).toBe("interrupt");
  });

  it("starts, stops, resumes the queue and removes queued prompts", async () => {
    const fake = new FakeTransport({
      cancelQueued: () => Promise.reject(new CommandError("request_not_pending", "gone")),
      stop: () =>
        Promise.reject(new CommandError("chat_not_found", "There is no chat with id c1.")),
    });
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      queue: [{ prompt_id: "p1", preview: "hi", held: true, created_at: 1 }],
    });
    await store.start(true);
    await store.resumeQueue();
    await store.cancelQueued("p1");
    expect(store.getSnapshot().queue).toEqual([]);
    expect(store.getSnapshot().actionError).toBeNull();
    await store.stop();
    expect(store.getSnapshot().actionError).toBe("There is no chat with id c1.");
    await store.revealLog();
    expect(store.getSnapshot().actionError).toBeNull();
    expect(fake.calls.slice(2).map((call) => [call.method, ...call.args])).toEqual([
      ["start", CHAT, true],
      ["resumeQueue", CHAT],
      ["cancelQueued", CHAT, "p1"],
      ["stop", CHAT],
      ["revealLog", CHAT],
    ]);
  });

  it("edits and reorders queued prompts from the shell's exact queue", async () => {
    const row = (prompt_id: string, preview: string, held = false) => ({
      prompt_id,
      preview,
      held,
      created_at: 1,
    });
    const fake = new FakeTransport({
      queuedText: async (_chat, id) => (id === "p1" ? "frist draft" : "later"),
      editQueued: async (_chat, _id, text) =>
        text.trim() === ""
          ? Promise.reject(new CommandError("prompt_empty", "The message is empty."))
          : [row("p1", text), row("p2", "later", true)],
      moveQueued: async () => [row("p2", "later", true), row("p1", "first draft")],
    });
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      queue: [row("p1", "frist draft"), row("p2", "later", true)],
    });
    expect(await store.queuedText("p1")).toBe("frist draft");
    expect(await store.editQueued("p1", "first draft")).toBeNull();
    expect(store.getSnapshot().queue.map((item) => item.preview)).toEqual(["first draft", "later"]);
    // A refused edit keeps the queue and says why, for the editor to show; the header stays quiet.
    expect(await store.editQueued("p1", " ")).toBe("The message is empty.");
    expect(store.getSnapshot().queue[0]?.preview).toBe("first draft");
    expect(store.getSnapshot().actionError).toBeNull();
    await store.moveQueued("p2", 0);
    expect(store.getSnapshot().queue).toEqual([row("p2", "later", true), row("p1", "first draft")]);
    expect(fake.calls.slice(2).map((call) => [call.method, ...call.args])).toEqual([
      ["queuedText", CHAT, "p1"],
      ["editQueued", CHAT, "p1", "first draft"],
      ["editQueued", CHAT, "p1", " "],
      ["moveQueued", CHAT, "p2", 0],
    ]);
  });

  it("drops a queued prompt that started or was removed before an edit or a move", async () => {
    const gone = () => Promise.reject(new CommandError("request_not_pending", "gone"));
    const fake = new FakeTransport({ queuedText: gone, editQueued: gone, moveQueued: gone });
    const queue = [
      { prompt_id: "p1", preview: "one", held: false, created_at: 1 },
      { prompt_id: "p2", preview: "two", held: false, created_at: 2 },
      { prompt_id: "p3", preview: "three", held: false, created_at: 3 },
    ];
    const store = await opened(fake, historyPage(CHAT, []), 0, { queue });
    expect(await store.queuedText("p1")).toBeNull();
    expect(await store.editQueued("p2", "x")).toBeNull();
    await store.moveQueued("p3", 0);
    expect(store.getSnapshot().queue).toEqual([]);
    expect(store.getSnapshot().actionError).toBeNull();
  });

  it("Send now takes the shell's exact queue, and the session's word on why the rest waits", async () => {
    const row = (prompt_id: string, preview: string, held = false) => ({
      prompt_id,
      preview,
      held,
      created_at: 1,
    });
    let gone = false;
    const fake = new FakeTransport({
      sendNow: async (_chat, id) =>
        gone
          ? Promise.reject(new CommandError("request_not_pending", "gone"))
          : id === "p2"
            ? [row("p2", "two"), row("p1", "one")]
            : Promise.reject(new CommandError("invalid_argument", "Wait for the chat change.")),
    });
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      queue: [row("p1", "one"), row("p2", "two", true)],
      session: sessionInfo({
        status: "running",
        queue_held: true,
        queue_wait: { reason: "working" },
      }),
    });
    expect(store.getSnapshot().session?.queue_wait).toEqual({ reason: "working" });
    await store.sendNow("p2");
    expect(store.getSnapshot().queue).toEqual([row("p2", "two"), row("p1", "one")]);
    expect(fake.callsOf("sendNow")).toEqual([[CHAT, "p2"]]);
    // The interrupt that follows says the turn is stopping; the header carries the new wait.
    fake
      .lastAttach()
      .onFrame(
        frame(CHAT, 0, [ev.session({ status: "cancelling", queue_wait: { reason: "stopping" } })]),
      );
    expect(store.getSnapshot().session?.queue_wait).toEqual({ reason: "stopping" });
    // A refusal says why in the header; a prompt that already went is simply dropped.
    await store.sendNow("p1");
    expect(store.getSnapshot().actionError).toBe("Wait for the chat change.");
    gone = true;
    await store.sendNow("p1");
    expect(store.getSnapshot().queue).toEqual([row("p2", "two")]);
  });
});

describe("queue action reply ordering", () => {
  const row = (id: string, preview = id): QueuedPrompt => ({
    prompt_id: id,
    preview,
    held: false,
    created_at: 1,
  });

  function deferred<T>() {
    let resolve: (value: T) => void = () => {};
    let reject: (error: unknown) => void = () => {};
    const promise = new Promise<T>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    return { promise, resolve, reject };
  }

  it("does not restore completed prompts when Send now answers after both turns finish", async () => {
    const reply = deferred<QueuedPrompt[]>();
    const fake = new FakeTransport({ sendNow: () => reply.promise });
    const store = await opened(fake, historyPage(CHAT, []), 0, {
      queue: [row("heldAtStop"), row("afterStop")],
    });
    const sending = store.sendNow("heldAtStop");
    const events = [
      ev.lifecycle("heldAtStop", "started"),
      ev.turnStarted("t1", "heldAtStop"),
      ev.message("t1.m1", "held at the stop"),
      ev.turnEnded("t1"),
      ev.lifecycle("afterStop", "started"),
      ev.turnStarted("t2", "afterStop"),
      ev.message("t2.m1", "typed after the stop"),
      ev.turnEnded("t2"),
    ];
    fake.lastAttach().onFrame(frame(CHAT, 0, events));
    expect(store.getSnapshot().queue).toEqual([]);
    expect(messageText(store, "t2.m1")).toBe("typed after the stop");
    reply.resolve([row("afterStop")]);
    await sending;
    expect(store.getSnapshot().queue).toEqual([]);
    await settled();
    expect(fake.attaches).toHaveLength(2);
    fake.lastAttach().resolve(attachResult(CHAT, events.length, historyPage(CHAT, [])));
    await settled();
    expect(store.getSnapshot().queue).toEqual([]);
    store.close();
  });

  it.each(["editQueued", "moveQueued"] as const)(
    "%s recovers the saved change without dropping a prompt queued during the request",
    async (method) => {
      const reply = deferred<QueuedPrompt[]>();
      const fake = new FakeTransport({ [method]: () => reply.promise });
      const store = await opened(fake, historyPage(CHAT, []), 0, {
        queue: [row("p1"), row("p2")],
      });
      const changing =
        method === "editQueued" ? store.editQueued("p1", "edited") : store.moveQueued("p2", 0);
      fake.lastAttach().onFrame(frame(CHAT, 0, [ev.lifecycle("p3", "queued", "third")]));
      const changed =
        method === "editQueued" ? [row("p1", "edited"), row("p2")] : [row("p2"), row("p1")];
      reply.resolve(changed);
      await changing;
      expect(store.getSnapshot().queue.map((item) => item.prompt_id)).toContain("p3");
      await settled();
      expect(fake.attaches).toHaveLength(2);
      const canonical = [...changed, row("p3", "third")];
      fake.lastAttach().resolve(attachResult(CHAT, 1, historyPage(CHAT, []), { queue: canonical }));
      await settled();
      expect(store.getSnapshot().queue).toEqual(canonical);
      store.close();
    },
  );

  it("refreshes again when a queue reply belongs to a replaced subscription", async () => {
    const reply = deferred<QueuedPrompt[]>();
    const fake = new FakeTransport({ moveQueued: () => reply.promise });
    const initial = [row("p1"), row("p2")];
    const store = await opened(fake, historyPage(CHAT, []), 0, { queue: initial });
    const moving = store.moveQueued("p2", 0);
    const rejoining = store.reattach();
    await settled();
    fake.lastAttach().resolve(attachResult(CHAT, 0, historyPage(CHAT, []), { queue: initial }));
    await rejoining;
    reply.resolve([row("p2"), row("p1")]);
    await moving;
    expect(store.getSnapshot().queue).toEqual(initial);
    await settled();
    expect(fake.attaches).toHaveLength(3);
    const canonical = [row("p2"), row("p1"), row("p3")];
    fake.lastAttach().resolve(attachResult(CHAT, 1, historyPage(CHAT, []), { queue: canonical }));
    await settled();
    expect(store.getSnapshot().queue).toEqual(canonical);
    store.close();
  });

  it.each(["reply first", "snapshot first"] as const)(
    "refreshes an action begun during attachment (%s)",
    async (order) => {
      const reply = deferred<QueuedPrompt[]>();
      const fake = new FakeTransport({ moveQueued: () => reply.promise });
      const initial = [row("p1"), row("p2")];
      const store = await opened(fake, historyPage(CHAT, []), 0, { queue: initial });
      const rejoining = store.reattach();
      await settled();
      const olderAttach = fake.lastAttach();
      const moving = store.moveQueued("p2", 0);
      const canonical = [row("p2"), row("p1")];
      const finishOlderAttach = async () => {
        // An unchanged snapshot may reuse the current queue: attachment itself makes ordering
        // ambiguous, independently of whether applying the snapshot changes its contents.
        olderAttach.resolve(attachResult(CHAT, 0, historyPage(CHAT, []), { queue: initial }));
        await rejoining;
      };
      if (order === "snapshot first") await finishOlderAttach();
      reply.resolve(canonical);
      await moving;
      await settled();
      expect(store.getSnapshot().queue).toEqual(initial);
      expect(fake.attaches).toHaveLength(3);
      if (order === "reply first") await finishOlderAttach();
      fake.lastAttach().resolve(attachResult(CHAT, 0, historyPage(CHAT, []), { queue: canonical }));
      await settled();
      expect(store.getSnapshot().queue).toEqual(canonical);
      store.close();
    },
  );

  it("resolves overlapping queue replies with a snapshot instead of reverting a later change", async () => {
    const edit = deferred<QueuedPrompt[]>();
    const move = deferred<QueuedPrompt[]>();
    const fake = new FakeTransport({
      editQueued: () => edit.promise,
      moveQueued: () => move.promise,
    });
    const store = await opened(fake, historyPage(CHAT, []), 0, { queue: [row("p1"), row("p2")] });
    const editing = store.editQueued("p1", "edited");
    const moving = store.moveQueued("p2", 0);
    const canonical = [row("p2"), row("p1", "edited")];
    move.resolve(canonical);
    await moving;
    expect(store.getSnapshot().queue).toEqual(canonical);
    edit.resolve([row("p1", "edited"), row("p2")]);
    await editing;
    expect(store.getSnapshot().queue).toEqual(canonical);
    await settled();
    expect(fake.attaches).toHaveLength(2);
    fake.lastAttach().resolve(attachResult(CHAT, 0, historyPage(CHAT, []), { queue: canonical }));
    await settled();
    expect(store.getSnapshot().queue).toEqual(canonical);
    store.close();
  });

  it.each(["editQueued", "moveQueued"] as const)(
    "%s applies its reply without rejoining for unrelated text, usage or session updates",
    async (method) => {
      const reply = deferred<QueuedPrompt[]>();
      const fake = new FakeTransport({ [method]: () => reply.promise });
      const store = await opened(fake, historyPage(CHAT, []), 0, {
        queue: [row("p1"), row("p2")],
      });
      const changing =
        method === "editQueued" ? store.editQueued("p1", "edited") : store.moveQueued("p2", 0);
      fake
        .lastAttach()
        .onFrame(frame(CHAT, 0, [ev.message("t1.m1", "text"), ev.usage(1), ev.session()]));
      const changed =
        method === "editQueued" ? [row("p1", "edited"), row("p2")] : [row("p2"), row("p1")];
      reply.resolve(changed);
      await changing;
      await settled();
      expect(store.getSnapshot().queue).toEqual(changed);
      expect(fake.attaches).toHaveLength(1);
      store.close();
    },
  );

  it.each(["cancelQueued", "queuedText", "editQueued", "moveQueued", "sendNow"] as const)(
    "%s does not remove a returned prompt using an older request_not_pending reply",
    async (method) => {
      const reply = deferred<never>();
      const fake = new FakeTransport({ [method]: () => reply.promise });
      const store = await opened(fake, historyPage(CHAT, []), 0, { queue: [row("p1")] });
      const changing =
        method === "editQueued"
          ? store.editQueued("p1", "edited")
          : method === "moveQueued"
            ? store.moveQueued("p1", 0)
            : store[method]("p1");
      fake
        .lastAttach()
        .onFrame(
          frame(CHAT, 0, [
            ev.lifecycle("p1", "sending"),
            ev.lifecycle("p1", "queued", "returned", true),
          ]),
        );
      reply.reject(new CommandError("request_not_pending", "already sending"));
      await changing;
      expect(store.getSnapshot().queue[0]?.preview).toBe("returned");
      await settled();
      expect(fake.attaches).toHaveLength(2);
      const canonical = [{ ...row("p1", "returned"), held: true }];
      fake.lastAttach().resolve(attachResult(CHAT, 2, historyPage(CHAT, []), { queue: canonical }));
      await settled();
      expect(store.getSnapshot().queue).toEqual(canonical);
      store.close();
    },
  );

  it("ignores a queue reply after the view closes without attaching again", async () => {
    const reply = deferred<QueuedPrompt[]>();
    const fake = new FakeTransport({ sendNow: () => reply.promise });
    const initial = [row("p1")];
    const store = await opened(fake, historyPage(CHAT, []), 0, { queue: initial });
    const sending = store.sendNow("p1");
    store.close();
    reply.resolve([]);
    await sending;
    await settled();
    expect(store.getSnapshot().queue).toEqual(initial);
    expect(fake.attaches).toHaveLength(1);
  });
});

describe("cards and questions", () => {
  it("permission_card_flow: buttons disable on click and the resolution closes the card", async () => {
    let answer: () => void = () => {};
    const fake = new FakeTransport({
      respondPermission: () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    const store = await opened(fake);
    fake.lastAttach().onFrame(frame(CHAT, 0, [ev.permission("r1")]));
    const responding = store.respondPermission("r1", "allow");
    expect(store.getSnapshot().requests.get("r1")).toEqual({
      busy: true,
      gone: false,
      error: null,
    });
    // A second click while the answer is on its way sends nothing more.
    await store.respondPermission("r1", "reject");
    expect(fake.callsOf("respondPermission")).toEqual([[CHAT, "r1", "allow"]]);
    answer();
    await responding;
    expect(store.getSnapshot().requests.get("r1")?.busy).toBe(true);
    fake.lastAttach().onFrame(frame(CHAT, 1, [ev.permissionResolved("r1", "allow")]));
    expect(store.getSnapshot().requests.has("r1")).toBe(false);
    const card = store.getSnapshot().transcript.items.get("permission:r1");
    expect(card?.kind === "permission" && card.resolution?.by).toBe("user");
  });

  it("permission_card_flow: request_not_pending closes the card; another error re-enables it", async () => {
    const fake = new FakeTransport({
      respondPermission: (_chat, id) =>
        id === "gone"
          ? Promise.reject(
              new CommandError(
                "request_not_pending",
                "This request is no longer waiting for an answer.",
              ),
            )
          : Promise.reject(new CommandError("option_invalid", "That option is not offered.")),
    });
    const store = await opened(fake);
    fake.lastAttach().onFrame(frame(CHAT, 0, [ev.permission("gone"), ev.permission("bad")]));
    await store.respondPermission("gone", "allow");
    expect(store.getSnapshot().requests.get("gone")).toEqual({
      busy: false,
      gone: true,
      error: null,
    });
    await store.respondPermission("bad", "nope");
    expect(store.getSnapshot().requests.get("bad")).toEqual({
      busy: false,
      gone: false,
      error: "That option is not offered.",
    });
  });

  it("permission_without_options: Dismiss answers cancelled", async () => {
    const fake = new FakeTransport();
    const store = await opened(fake);
    fake.lastAttach().onFrame(frame(CHAT, 0, [ev.permission("r", [])]));
    await store.respondPermission("r", null);
    expect(fake.callsOf("respondPermission")).toEqual([[CHAT, "r", null]]);
  });

  it("question_card_flow: an answer, a dismissal, and request_not_pending", async () => {
    const fake = new FakeTransport({
      respondUi: (_chat, id) =>
        id === "q3"
          ? Promise.reject(
              new CommandError(
                "request_not_pending",
                "This request is no longer waiting for an answer.",
              ),
            )
          : Promise.resolve(),
    });
    const store = await opened(fake);
    const channel = fake.lastAttach();
    channel.onFrame(frame(CHAT, 0, [ev.question("q1"), ev.question("q2"), ev.question("q3")]));
    await store.respondUi("q1", { type: "select", index: 1 });
    await store.respondUi("q2", null);
    await store.respondUi("q3", { type: "select", index: 0 });
    expect(fake.callsOf("respondUi")).toEqual([
      [CHAT, "q1", { type: "select", index: 1 }],
      [CHAT, "q2", null],
      [CHAT, "q3", { type: "select", index: 0 }],
    ]);
    expect(store.getSnapshot().requests.get("q3")?.gone).toBe(true);
    channel.onFrame(
      frame(CHAT, 3, [
        ev.questionResolved("q1", { type: "select", index: 1 }),
        ev.questionResolved("q2", null),
      ]),
    );
    const q1 = store.getSnapshot().transcript.items.get("question:q1");
    expect(q1?.kind === "question" && q1.resolution?.answer).toEqual({ type: "select", index: 1 });
    expect(store.getSnapshot().requests.has("q1")).toBe(false);
  });
});

describe("full outputs", () => {
  it("load_full_output: pages through the blob by next_offset, ending at eof", async () => {
    const sha = "a".repeat(64);
    const pages = new Map([
      [
        0,
        {
          sha256: sha,
          offset: 0,
          next_offset: 262_142,
          total: 300_000,
          text: "first ",
          eof: false,
        },
      ],
      [
        262_142,
        {
          sha256: sha,
          offset: 262_142,
          next_offset: 300_000,
          total: 300_000,
          text: "second",
          eof: true,
        },
      ],
    ]);
    const fake = new FakeTransport({
      blob: (_chat, _sha, offset) => {
        const page = pages.get(offset);
        return page ? Promise.resolve(page) : Promise.reject(new Error(`no page at ${offset}`));
      },
    });
    const store = await opened(fake);
    await store.loadBlob(sha);
    expect(store.getSnapshot().blobs.get(sha)).toMatchObject({
      text: "first ",
      nextOffset: 262_142,
      eof: false,
    });
    await store.loadBlob(sha);
    expect(store.getSnapshot().blobs.get(sha)).toMatchObject({ text: "first second", eof: true });
    await store.loadBlob(sha);
    // Each page asks from where the last one ended (a character boundary), 256 KiB at most.
    expect(fake.callsOf("blob")).toEqual([
      [CHAT, sha, 0, BLOB_PAGE],
      [CHAT, sha, 262_142, BLOB_PAGE],
    ]);
  });

  it("load_full_output: a missing blob says why and can be tried again", async () => {
    const sha = "b".repeat(64);
    let fail = true;
    const fake = new FakeTransport({
      blob: () =>
        fail
          ? Promise.reject(
              new CommandError(
                "blob_missing",
                "The full output isn't saved yet because the disk is full.",
              ),
            )
          : Promise.resolve({
              sha256: sha,
              offset: 0,
              next_offset: 2,
              total: 2,
              text: "ok",
              eof: true,
            }),
    });
    const store = await opened(fake);
    await store.loadBlob(sha);
    expect(store.getSnapshot().blobs.get(sha)?.error).toBe(
      "The full output isn't saved yet because the disk is full.",
    );
    fail = false;
    await store.loadBlob(sha);
    expect(store.getSnapshot().blobs.get(sha)).toMatchObject({
      text: "ok",
      eof: true,
      error: null,
    });
  });
});

describe("still working", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("still_working_indicator: shows after a minute without frames during a turn, counting up", async () => {
    const fake = new FakeTransport();
    const store = new ChatStore(CHAT, fake, { now: () => Date.now() });
    const opening = store.open();
    fake.lastAttach().resolve(
      attachResult(CHAT, 0, historyPage(CHAT, []), {
        session: sessionInfo({ status: "running" }),
      }),
    );
    await opening;
    await vi.advanceTimersByTimeAsync(STILL_WORKING_MS - 1);
    expect(store.getSnapshot().stillWorkingMinutes).toBeNull();
    await vi.advanceTimersByTimeAsync(1);
    expect(store.getSnapshot().stillWorkingMinutes).toBe(1);
    await vi.advanceTimersByTimeAsync(STILL_WORKING_MS);
    expect(store.getSnapshot().stillWorkingMinutes).toBe(2);
    // A frame resets it; nothing was ever stopped.
    fake.lastAttach().onFrame(frame(CHAT, 0, [ev.message("m", "still here")]));
    expect(store.getSnapshot().stillWorkingMinutes).toBeNull();
    await vi.advanceTimersByTimeAsync(STILL_WORKING_MS);
    expect(store.getSnapshot().stillWorkingMinutes).toBe(1);
    expect(fake.methods()).not.toContain("interrupt");
    expect(fake.methods()).not.toContain("forceStop");
    // Not while no turn runs.
    fake.lastAttach().onFrame(frame(CHAT, 1, [ev.session({ status: "ready" })]));
    expect(store.getSnapshot().stillWorkingMinutes).toBeNull();
    await vi.advanceTimersByTimeAsync(5 * STILL_WORKING_MS);
    expect(store.getSnapshot().stillWorkingMinutes).toBeNull();
    store.close();
  });
});

describe("native controls", () => {
  it("keeps requested and effective state distinct and reports native rejection", async () => {
    let answer!: (value: SessionLists) => void;
    const fake = new FakeTransport({
      setControl: () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    const store = await opened(fake);
    const change = { type: "workflow", value: "plan", implementation_approved: false } as const;
    const pending = store.setControl(change);
    expect(store.getSnapshot().controlSaving).toEqual(change);
    answer(lists(8));
    await pending;
    expect(store.getSnapshot()).toMatchObject({
      controlSaving: null,
      listsError: null,
      lists: { revision: 8 },
    });
    fake.handlers.setControl = () =>
      Promise.reject(new CommandError("option_invalid", "The native mode is unavailable."));
    await store.setControl(change);
    expect(store.getSnapshot()).toMatchObject({
      controlSaving: null,
      listsError: "The native mode is unavailable.",
      lists: { revision: 8 },
    });
  });
  it("ignores a late native control reply after a fresh subscription", async () => {
    let answer!: (value: SessionLists) => void;
    const fake = new FakeTransport({
      setControl: () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    });
    const store = await opened(fake);
    const pending = store.setControl({ type: "access", value: "native-custom-access" });
    const joining = store.reattach();
    await settled();
    fake.lastAttach().resolve(attachResult(CHAT, 0, historyPage(CHAT, [])));
    await joining;
    answer(lists(100));
    await pending;
    expect(store.getSnapshot().lists?.revision).not.toBe(100);
    expect(store.getSnapshot().controlSaving).toBeNull();
  });
  it("keeps the newer control reply when older work finishes last", async () => {
    const answers = new Map<string, (value: SessionLists) => void>();
    const fake = new FakeTransport({
      setControl: (_chat, change) => new Promise((resolve) => answers.set(change.value, resolve)),
    });
    const store = await opened(fake);
    const older = store.setControl({ type: "access", value: "older" });
    const newer = store.setControl({ type: "access", value: "newer" });
    answers.get("newer")!(lists(20));
    await newer;
    answers.get("older")!(lists(10));
    await older;
    expect(store.getSnapshot()).toMatchObject({ controlSaving: null, lists: { revision: 20 } });
  });
});
