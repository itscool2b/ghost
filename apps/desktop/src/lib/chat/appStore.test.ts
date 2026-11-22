import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentEntry, ChatSummary, EnvStatus, StorageStatus } from "../../bindings";
import { CommandError } from "../ipc";
import {
  AppStore,
  agentChoice,
  CATALOG_POLL_MS,
  CHATS_RETRY_MS,
  DETECTION_POLL_MS,
  NewChatDraft,
  RELEASE_POLL_WINDOW_MS,
  STATUS_RESUBSCRIBE_MS,
  STORAGE_ALERT_POLL_MS,
  STORAGE_POLL_MS,
} from "./appStore";
import { chatSummary, FakeTransport, OK_STORAGE, settled } from "./testing";

function agent(id: string, availability: AgentEntry["availability"]): AgentEntry {
  return {
    id,
    name: id,
    description: "",
    availability,
    launch: id,
    needs_download: false,
    last_verified: "",
    installation: null,
    latest_release: null,
  };
}

const PAUSED: StorageStatus = {
  state: { state: "paused", reason: "disk_full", since: 1 },
  unsaved_bytes: 10,
};

describe("the app store", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("shows the list kept from the last launch until the first read answers, and never over a real one", async () => {
    let answer: (chats: ChatSummary[]) => void = () => {};
    const fake = new FakeTransport({
      chatsList: () =>
        new Promise<ChatSummary[]>((resolve) => {
          answer = resolve;
        }),
    });
    const store = new AppStore(fake);
    store.hydrate([chatSummary("old", { updated_at: 1 }), chatSummary("kept", { updated_at: 5 })]);
    // Newest first, as the real list is, and marked as not the database's word.
    expect(store.getSnapshot().chats?.map((chat) => chat.id)).toEqual(["kept", "old"]);
    expect(store.getSnapshot().chatsCached).toBe(true);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot().chatsCached).toBe(true);
    expect(store.getSnapshot().chatsError).toBeNull();
    // The database's list replaces it whole: a chat deleted since is gone, a new one is there.
    answer([chatSummary("kept", { updated_at: 5 }), chatSummary("new", { updated_at: 9 })]);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot().chats?.map((chat) => chat.id)).toEqual(["new", "kept"]);
    expect(store.getSnapshot().chatsCached).toBe(false);
    store.hydrate([chatSummary("stale")]);
    expect(store.getSnapshot().chats?.map((chat) => chat.id)).toEqual(["new", "kept"]);
    expect(store.getSnapshot().chatsCached).toBe(false);
    store.stop();
  });

  it("reads the chat list again until the database answers, as after an update", async () => {
    let unavailable = 2;
    const fake = new FakeTransport({
      chatsList: () =>
        unavailable-- > 0
          ? Promise.reject(
              new CommandError("storage_unavailable", "o7's database is not available."),
            )
          : Promise.resolve([chatSummary("kept", { updated_at: 1 })]),
    });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot().chatsError).toMatch(/not available/);
    expect(store.getSnapshot().chats).toBeNull();
    await vi.advanceTimersByTimeAsync(CHATS_RETRY_MS[0] + CHATS_RETRY_MS[1]);
    expect(store.getSnapshot().chats?.map((chat) => chat.id)).toEqual(["kept"]);
    expect(store.getSnapshot().chatsError).toBeNull();
    // Once it answered, it stops reading on a timer.
    const reads = fake.callsOf("chatsList").length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fake.callsOf("chatsList")).toHaveLength(reads);
    store.stop();
  });

  it("loads chats, env, storage and agents, and keeps the list current from the status channel", async () => {
    const fake = new FakeTransport({
      chatsList: () =>
        Promise.resolve([
          chatSummary("old", { updated_at: 1 }),
          chatSummary("new", { updated_at: 5 }),
        ]),
    });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.methods().slice(0, 5)).toEqual([
      "statusSubscribe",
      "chatsList",
      "storageStatus",
      "envStatus",
      "agentsList",
    ]);
    expect(store.getSnapshot().chats?.map((chat) => chat.id)).toEqual(["new", "old"]);

    fake.statusChannels[0]?.onUpdate({
      chat_id: "old",
      status: "running",
      needs_attention: true,
      title: "Fix the test",
      updated_at: 9,
    });
    const chats = store.getSnapshot().chats ?? [];
    expect(chats.map((chat) => chat.id)).toEqual(["old", "new"]);
    expect(chats[0]).toMatchObject({
      status: "running",
      needs_attention: true,
      title: "Fix the test",
    });

    // A chat this window does not list yet (created elsewhere) reloads the list.
    fake.statusChannels[0]?.onUpdate({
      chat_id: "other",
      status: "starting",
      needs_attention: false,
      title: null,
      updated_at: 10,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("chatsList")).toHaveLength(2);
    store.stop();
  });

  it("reloads the list when a start fails or gets past a folder problem, for the folder's state", async () => {
    let row: Partial<ChatSummary> = { status: "starting", folder_state: "ok" };
    const fake = new FakeTransport({ chatsList: () => Promise.resolve([chatSummary("c1", row)]) });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    const update = (status: ChatSummary["status"], at: number) =>
      fake.statusChannels[0]?.onUpdate({
        chat_id: "c1",
        status,
        needs_attention: false,
        title: null,
        updated_at: at,
      });

    // The start fails because the folder is gone: no update says so, the list does.
    row = { status: "failed", folder_state: "missing" };
    update("failed", 2);
    expect(store.getSnapshot().chats?.[0]?.status).toBe("failed");
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("chatsList")).toHaveLength(2);
    expect(store.getSnapshot().chats?.[0]).toMatchObject({
      status: "failed",
      folder_state: "missing",
    });

    // Retry, with the folder back: a start that gets ready passed the folder check.
    row = { status: "starting", folder_state: "ok" };
    update("starting", 3);
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("chatsList")).toHaveLength(2);
    row = { status: "ready", folder_state: "ok" };
    update("ready", 4);
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("chatsList")).toHaveLength(3);
    expect(store.getSnapshot().chats?.[0]).toMatchObject({ status: "ready", folder_state: "ok" });

    // Other changes keep the list as it is.
    update("running", 5);
    update("ready", 6);
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("chatsList")).toHaveLength(3);
    store.stop();
  });

  it("applies updates that arrive during a reload on top of its answer, and only the newest", async () => {
    const answers: ((chats: ChatSummary[]) => void)[] = [];
    const fake = new FakeTransport({
      chatsList: () => new Promise((resolve) => answers.push(resolve)),
    });
    const store = new AppStore(fake);
    store.start();
    answers[0]?.([chatSummary("c1", { status: "stopped", updated_at: 1 })]);
    await vi.advanceTimersByTimeAsync(0);
    const update = (chat: string, status: ChatSummary["status"], at: number) =>
      fake.statusChannels[0]?.onUpdate({
        chat_id: chat,
        status,
        needs_attention: false,
        title: null,
        updated_at: at,
      });

    // A chat the list lacks reloads it; c1 starts while the list is read, and the list was read
    // before that.
    update("c2", "starting", 2);
    update("c1", "starting", 3);
    answers[1]?.([
      chatSummary("c1", { status: "stopped", updated_at: 1 }),
      chatSummary("c2", { status: "starting", updated_at: 2 }),
    ]);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot().chats?.map((chat) => [chat.id, chat.status])).toEqual([
      ["c1", "starting"],
      ["c2", "starting"],
    ]);

    // Two reloads on their way: the older one answering last changes nothing.
    void store.reloadChats();
    void store.reloadChats();
    answers[3]?.([chatSummary("c1", { status: "ready" }), chatSummary("c3", { status: "ready" })]);
    await vi.advanceTimersByTimeAsync(0);
    answers[2]?.([chatSummary("c1", { status: "stopped" })]);
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot().chats?.map((chat) => [chat.id, chat.status])).toEqual([
      ["c1", "ready"],
      ["c3", "ready"],
    ]);
    store.stop();
  });

  it("subscribes again when the status channel ends, and catches up", async () => {
    const fake = new FakeTransport();
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    fake.statusChannels[0]?.onEnd();
    await vi.advanceTimersByTimeAsync(STATUS_RESUBSCRIBE_MS);
    expect(fake.callsOf("statusSubscribe")).toHaveLength(2);
    expect(fake.callsOf("chatsList")).toHaveLength(2);
    store.stop();
  });

  it("ignores the end of a subscription it replaced, so a restart never loops", async () => {
    const fake = new FakeTransport();
    const store = new AppStore(fake);
    // React's development mode starts, stops and starts again.
    store.start();
    store.stop();
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("statusSubscribe")).toHaveLength(2);
    // The shell ends the first channel, replaced by the second: nothing follows.
    fake.statusChannels[0]?.onEnd();
    fake.statusChannels[0]?.onUpdate({
      chat_id: "stale",
      status: "ready",
      needs_attention: false,
      title: null,
      updated_at: 1,
    });
    await vi.advanceTimersByTimeAsync(10 * STATUS_RESUBSCRIBE_MS);
    expect(fake.callsOf("statusSubscribe")).toHaveLength(2);
    expect(fake.callsOf("chatsList")).toHaveLength(2);
    store.stop();
  });

  it("a restart keeps one of each poll, even with answers still on their way", async () => {
    const fake = new FakeTransport({
      envStatus: () => Promise.resolve({ state: { state: "resolving" }, removed: [] }),
      agentsList: () => Promise.resolve([agent("mock", { state: "checking" })]),
    });
    const store = new AppStore(fake);
    // React's development mode starts, stops and starts again before the first answers arrive.
    store.start();
    store.stop();
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("storageStatus")).toHaveLength(2);
    expect(fake.callsOf("envStatus")).toHaveLength(2);
    expect(fake.callsOf("agentsList")).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(10 * STORAGE_POLL_MS);
    // Only the second run polls: storage every 5 s, the env and the agents every second.
    expect(fake.callsOf("storageStatus")).toHaveLength(2 + 10);
    const detections = (10 * STORAGE_POLL_MS) / DETECTION_POLL_MS;
    expect(fake.callsOf("envStatus")).toHaveLength(2 + detections);
    expect(fake.callsOf("agentsList")).toHaveLength(2 + detections);
    store.stop();
    await vi.advanceTimersByTimeAsync(10 * STORAGE_POLL_MS);
    expect(fake.callsOf("storageStatus")).toHaveLength(2 + 10);
  });

  it("polls storage faster while saving is paused", async () => {
    let storage = OK_STORAGE;
    const fake = new FakeTransport({ storageStatus: () => Promise.resolve(storage) });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(fake.callsOf("storageStatus")).toHaveLength(1);
    storage = PAUSED;
    await vi.advanceTimersByTimeAsync(STORAGE_POLL_MS);
    expect(store.getSnapshot().storage).toEqual(PAUSED);
    await vi.advanceTimersByTimeAsync(STORAGE_ALERT_POLL_MS);
    expect(fake.callsOf("storageStatus")).toHaveLength(3);
    storage = OK_STORAGE;
    await vi.advanceTimersByTimeAsync(STORAGE_ALERT_POLL_MS);
    expect(store.getSnapshot().storage).toEqual(OK_STORAGE);
    store.stop();
    await vi.advanceTimersByTimeAsync(10 * STORAGE_POLL_MS);
    expect(fake.callsOf("storageStatus")).toHaveLength(4);
  });

  it("follows the env while it resolves, and Retry starts a new resolution", async () => {
    const resolving: EnvStatus = { state: { state: "resolving" }, removed: [] };
    const fallback: EnvStatus = {
      state: { state: "fallback", reason: "the login shell timed out" },
      removed: ["CLAUDECODE"],
    };
    let env = resolving;
    const fake = new FakeTransport({
      envStatus: () => Promise.resolve(env),
      envRefresh: () => Promise.resolve(resolving),
    });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    env = fallback;
    await vi.advanceTimersByTimeAsync(DETECTION_POLL_MS);
    expect(store.getSnapshot().env).toEqual(fallback);
    await vi.advanceTimersByTimeAsync(10 * DETECTION_POLL_MS);
    expect(fake.callsOf("envStatus")).toHaveLength(2);

    await store.retryEnv();
    expect(store.getSnapshot().env).toEqual(resolving);
    env = {
      state: { state: "ready", source: "zsh login shell", took_ms: 80, vars: 30 },
      removed: [],
    };
    await vi.advanceTimersByTimeAsync(DETECTION_POLL_MS);
    expect(store.getSnapshot().env?.state.state).toBe("ready");
    store.stop();
  });

  it("follows a release check quickly only shortly after a start", async () => {
    const ready = agent("mock", { state: "ready", version: "1", path: "builtin" });
    let harness: AgentEntry["harness"] = { state: "checking" };
    const fake = new FakeTransport({ agentsList: () => Promise.resolve([{ ...ready, harness }]) });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    // Checking: every second, so an update notice shows soon after the start.
    await vi.advanceTimersByTimeAsync(3 * DETECTION_POLL_MS);
    expect(fake.callsOf("agentsList")).toHaveLength(4);
    // Past the window, a check that never ends (an agent installed later) is polled slowly.
    await vi.advanceTimersByTimeAsync(RELEASE_POLL_WINDOW_MS);
    const late = fake.callsOf("agentsList").length;
    await vi.advanceTimersByTimeAsync(10 * DETECTION_POLL_MS);
    expect(fake.callsOf("agentsList")).toHaveLength(late);
    // Within the window, a finished check also slows down.
    store.stop();
    harness = { state: "up_to_date", version: "1", checked_at: 1 };
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    const started = fake.callsOf("agentsList").length;
    await vi.advanceTimersByTimeAsync(10 * DETECTION_POLL_MS);
    expect(fake.callsOf("agentsList")).toHaveLength(started);
    store.stop();
  });

  it("follows agents still being checked, and Refresh probes them again", async () => {
    let agents = [agent("mock", { state: "checking" })];
    const fake = new FakeTransport({
      agentsList: () => Promise.resolve(agents),
      agentsRefresh: () =>
        Promise.resolve([agent("mock", { state: "ready", version: "1", path: "builtin" })]),
    });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    agents = [agent("mock", { state: "ready", version: null, path: "builtin" })];
    await vi.advanceTimersByTimeAsync(DETECTION_POLL_MS);
    expect(store.getSnapshot().agents?.[0]?.availability.state).toBe("ready");
    await vi.advanceTimersByTimeAsync(10 * DETECTION_POLL_MS);
    expect(fake.callsOf("agentsList")).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(CATALOG_POLL_MS);
    expect(fake.callsOf("agentsList")).toHaveLength(3);
    const refreshing = store.refreshAgents();
    expect(store.getSnapshot().agentsRefreshing).toBe(true);
    await refreshing;
    expect(store.getSnapshot().agentsRefreshing).toBe(false);
    expect(fake.callsOf("agentsRefresh")).toHaveLength(1);
    store.stop();
  });

  it("applies a Connection setting to the agents at once, without probing again", async () => {
    const adapter = {
      ...agent("claude", { state: "ready", version: "Claude Code 2.1.310", path: "/bin/claude" }),
      connections: {
        preferred: "claude-acp",
        active: "claude-acp",
        options: [],
      },
    };
    const fake = new FakeTransport({
      agentSetConnection: () => Promise.resolve([adapter]),
    });
    const store = new AppStore(fake);
    await store.setAgentConnection("claude", "claude-acp");
    expect(fake.callsOf("agentSetConnection")).toEqual([["claude", "claude-acp"]]);
    expect(store.getSnapshot().agents?.[0]?.connections?.active).toBe("claude-acp");
    expect(fake.callsOf("agentsRefresh")).toHaveLength(0);
    // A refusal reaches the caller, and the agents stay as they were.
    const refusing = new FakeTransport({
      agentSetConnection: () => Promise.reject(new Error("This agent has one connection.")),
    });
    const other = new AppStore(refusing);
    await expect(other.setAgentConnection("hermes", "hermes")).rejects.toThrow("one connection");
    expect(other.getSnapshot().agents).toBeNull();
  });

  it("adds a chat this window created at the top", async () => {
    const fake = new FakeTransport({
      chatsList: () => Promise.resolve([chatSummary("a", { updated_at: 1 })]),
    });
    const store = new AppStore(fake);
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    store.addChat(chatSummary("b", { updated_at: 2 }));
    expect(store.getSnapshot().chats?.map((chat) => chat.id)).toEqual(["b", "a"]);
    store.stop();
  });
});

describe("choosing an agent", () => {
  it("unavailable_agent_disabled: a missing or broken agent is disabled with its reason", () => {
    expect(
      agentChoice(agent("hermes", { state: "missing", reason: "hermes was not found." })),
    ).toEqual({
      enabled: false,
      reason: "hermes was not found.",
    });
    expect(
      agentChoice(
        agent("claude", {
          state: "broken",
          reason: "Node.js 22 or newer is required (found 20.11.0).",
        }),
      ),
    ).toEqual({ enabled: false, reason: "Node.js 22 or newer is required (found 20.11.0)." });
    // Ready and still-checking agents can be chosen: the start waits for the detection.
    expect(agentChoice(agent("mock", { state: "checking" })).enabled).toBe(true);
    expect(agentChoice(agent("mock", { state: "ready", version: null, path: "x" })).enabled).toBe(
      true,
    );
  });
});

describe("the New chat draft", () => {
  it("runs auth and updates only after explicit actions, and refuses unsupported updates", async () => {
    const fake = new FakeTransport({
      agentMaintenance: () =>
        Promise.resolve({ auth_status: true, login_available: true, update_unavailable: null }),
      agentAuthStatus: () => Promise.resolve({ state: "credentials_present" }),
    });
    const draft = new NewChatDraft(fake);
    draft.chooseAgent("claude");
    await settled();
    expect(fake.methods()).toEqual(["agentMaintenance"]);
    await draft.checkAuth();
    expect(draft.getSnapshot().auth).toEqual({ state: "credentials_present" });
    expect(await draft.updateAgent()).toBe(true);
    expect(fake.callsOf("agentUpdate")).toEqual([["claude"]]);
    fake.handlers.agentMaintenance = () =>
      Promise.resolve({
        auth_status: true,
        login_available: true,
        update_unavailable: "Managed by Editor.",
      });
    draft.chooseAgent("codex");
    await settled();
    expect(await draft.updateAgent()).toBe(false);
    expect(fake.callsOf("agentUpdate")).toHaveLength(1);
  });

  it("does not show a delayed auth response on another agent", async () => {
    let finish: ((value: { state: "sign_in_required" }) => void) | undefined;
    const fake = new FakeTransport({
      agentMaintenance: () =>
        Promise.resolve({ auth_status: true, login_available: true, update_unavailable: null }),
      agentAuthStatus: () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    });
    const draft = new NewChatDraft(fake);
    draft.chooseAgent("claude");
    await settled();
    const pending = draft.checkAuth();
    draft.chooseAgent("codex");
    finish?.({ state: "sign_in_required" });
    await pending;
    await settled();
    expect(draft.getSnapshot().agentId).toBe("codex");
    expect(draft.getSnapshot().auth).toBeNull();
    expect(draft.getSnapshot().maintenanceBusy).toBeNull();
  });
  it("picker_cancel: cancelling the folder picker changes nothing", async () => {
    let answer: string | null = "/Users/me/code/app";
    const fake = new FakeTransport({ pickFolder: () => Promise.resolve(answer) });
    const draft = new NewChatDraft(fake);
    await draft.chooseFolder();
    expect(draft.getSnapshot().folder).toBe("/Users/me/code/app");
    const before = draft.getSnapshot();
    answer = null;
    await draft.chooseFolder();
    expect(draft.getSnapshot()).toEqual(before);
    expect(fake.methods()).toEqual(["pickFolder", "pickFolder"]);
  });

  it("creates the chat with the agent and folder chosen, and shows a refusal inline", async () => {
    const fake = new FakeTransport({
      pickFolder: () => Promise.resolve("/tmp/missing"),
      chatCreate: (agentId) =>
        agentId === "hermes"
          ? Promise.reject(
              new CommandError(
                "invalid_folder",
                "/tmp/missing can't be used as the chat's folder: it doesn't exist.",
              ),
            )
          : Promise.resolve(chatSummary("c9")),
    });
    const draft = new NewChatDraft(fake);
    expect(await draft.create()).toBeNull();
    expect(fake.methods()).toEqual([]);
    draft.chooseAgent("hermes");
    await draft.chooseFolder();
    expect(await draft.create()).toBeNull();
    expect(draft.getSnapshot().error).toBe(
      "/tmp/missing can't be used as the chat's folder: it doesn't exist.",
    );
    draft.chooseAgent("mock");
    expect(draft.getSnapshot().error).toBeNull();
    const chat = await draft.create();
    expect(chat?.id).toBe("c9");
    expect(fake.callsOf("chatCreate")).toEqual([
      ["hermes", "/tmp/missing"],
      ["mock", "/tmp/missing"],
    ]);
    await settled();
  });

  it("shows a picker that could not open", async () => {
    const fake = new FakeTransport({
      pickFolder: () =>
        Promise.reject(new CommandError("dialog_failed", "The folder picker could not be shown.")),
    });
    const draft = new NewChatDraft(fake);
    await draft.chooseFolder();
    expect(draft.getSnapshot()).toMatchObject({
      picking: false,
      folder: null,
      error: "The folder picker could not be shown.",
    });
  });
});
