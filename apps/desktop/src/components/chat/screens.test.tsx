import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AgentEntry, ConfigChoice, ConfigOption, QueueWait } from "../../bindings";
import { agentLineup } from "../../lib/chat/agentLineup";
import { agentChoice, NewChatDraft } from "../../lib/chat/appStore";
import { type ChatState, ChatStore } from "../../lib/chat/chatStore";
import { createdChatOpens } from "../../lib/chat/newChatContext";
import { queueWaitView } from "../../lib/chat/queueWait";
import {
  attachResult,
  chatSummary,
  ev,
  FakeTransport,
  historyPage,
  items,
  lists,
  numbered,
  sessionInfo,
} from "../../lib/chat/testing";
import { applyEntries, emptyTranscript, renderTranscript } from "../../lib/chat/transcript";
import { primaryAgents } from "../setup/Onboarding";
import { Banners, envBanner, noticeItems, RecoveryBannerView, storageBanner } from "./Banners";
import { ChatList, chatShortcutIndex, projectName, projectsOf, visibleChatIds } from "./ChatList";
import { StillWorking } from "./ChatView";
import { SendControls, StopButton } from "./Composer";
import { Header } from "./Header";
import { downloadNote, InstallationStatus, NewChat } from "./NewChat";
import { foldedModels, Pickers, shownModelName } from "./Pickers";
import { QueueEditor, QueueList } from "./QueueList";
import { SidebarFooter } from "./SidebarFooter";
import { Transcript } from "./Transcript";
import { type TurnActions, TurnView } from "./TurnView";

const noop = (): void => {};

function buttons(html: string): { label: string; disabled: boolean }[] {
  return [...html.matchAll(/<button([^>]*)>(.*?)<\/button>/g)].map((match) => ({
    label: match[1]?.match(/aria-label="([^"]+)"/)?.[1] ?? (match[2] ?? "").replace(/<[^>]+>/g, ""),
    disabled: (match[1] ?? "").includes("disabled"),
  }));
}

function agent(
  id: string,
  availability: AgentEntry["availability"],
  extra: Partial<AgentEntry> = {},
): AgentEntry {
  return {
    id,
    name: id,
    description: `${id} agent`,
    availability,
    launch: id,
    needs_download: false,
    last_verified: "",
    installation: null,
    latest_release: null,
    ...extra,
  };
}

/** A chat view's state after attaching with `page`. */
async function stateWith(
  page = historyPage("c1", []),
  snapshot: Parameters<typeof attachResult>[3] = {},
): Promise<ChatState> {
  const fake = new FakeTransport();
  const store = new ChatStore("c1", fake);
  const opening = store.open();
  fake.lastAttach().resolve(attachResult("c1", 50, page, snapshot));
  await opening;
  return store.getSnapshot();
}

const ACTIONS: TurnActions = {
  blobs: new Map(),
  onLoadBlob: noop,
  requests: new Map(),
  onRespondPermission: noop,
  onRespondUi: noop,
  errorActions: () => ({ retry: false, startFresh: false, recreate: false }),
  onStart: noop,
  onShowLog: noop,
  canResume: true,
};

describe("New chat", () => {
  it("unavailable_agent_disabled: agents that can't run here are not offered and show their reason", () => {
    const agents = [
      agent("mock", { state: "ready", version: null, path: "built in" }),
      // Claude Code can run neither way: Rust's one reason names what each connection lacks.
      agent("claude", {
        state: "broken",
        reason:
          "Ghost needs Claude Code 2.1.310 or newer (found 2.1.309). ACP adapter: Node.js 22 or newer is required (found 20.11.0).",
      }),
      agent("hermes", { state: "broken", reason: "hermes --version failed." }),
    ];
    const html = renderToStaticMarkup(
      <NewChat
        agents={agents}
        refreshing={false}
        draft={new NewChatDraft(new FakeTransport())}
        onRefreshAgents={noop}
        onCreated={noop}
      />,
    );
    // Only agents that can run are offered as choices; the others wait under Other agents.
    expect(html).toMatch(
      /<input type="radio"(?=[^>]*value="mock")(?=[^>]*aria-label="mock, ready")/,
    );
    expect(html).not.toContain('value="claude"');
    expect(html).not.toContain('value="hermes"');
    expect(html).toContain("Other agents");
    expect(html).toMatch(
      /data-test-unavailable="claude".*ACP adapter: Node\.js 22 or newer is required \(found 20\.11\.0\)\./,
    );
    expect(html).toMatch(/data-test-unavailable="hermes".*hermes --version failed\./);
    // Nothing chosen yet: Create waits for an agent and a folder.
    expect(buttons(html).find((b) => b.label === "Start chat")?.disabled).toBe(true);
    expect(buttons(html).find((b) => b.label === "Refresh agents")?.disabled).toBe(false);
  });

  it("states the one-time download before the chat that needs it is created", () => {
    // Claude Code runs through its adapter here (its CLI is too old for the native driver).
    const adapter = agent(
      "claude",
      {
        state: "ready",
        version: "Claude Code 2.1.309 via claude-agent-acp 0.95.0",
        path: "/usr/local/bin/claude",
      },
      {
        name: "Claude Code",
        needs_download: true,
        launch: "@agentclientprotocol/claude-agent-acp 0.95.0 with your claude",
      },
    );
    expect(downloadNote(adapter)).toBe(
      "Creating this chat downloads @agentclientprotocol/claude-agent-acp 0.95.0 from npm (one time).",
    );
    expect(downloadNote({ ...adapter, needs_download: false })).toBeNull();
    const draft = new NewChatDraft(new FakeTransport());
    draft.chooseAgent("claude");
    const html = renderToStaticMarkup(
      <NewChat
        agents={[adapter]}
        refreshing={true}
        draft={draft}
        onRefreshAgents={noop}
        onCreated={noop}
      />,
    );
    expect(html).toContain(
      "Creating this chat downloads @agentclientprotocol/claude-agent-acp 0.95.0",
    );
    // The version line under "Claude Code is ready" does not repeat the name.
    expect(html).toContain(">2.1.309 via claude-agent-acp 0.95.0 (/usr/local/bin/claude)<");
    // One Claude Code, whichever way it runs.
    expect(html.match(/type="radio"/g)?.length).toBe(1);
    expect(buttons(html).find((b) => b.label === "Refreshing…")?.disabled).toBe(true);
  });

  it("shows untested version and bundle ownership without claiming compatibility", () => {
    const html = renderToStaticMarkup(
      <InstallationStatus
        installation={{
          agent_id: "codex",
          provider: "codex",
          host: { type: "wsl", distro: "Ubuntu" },
          launch_path: "/bin/codex",
          resolved_path: null,
          version: "0.170.0",
          channel: { type: "bundled", application: "Editor" },
          verification: { state: "untested", last_verified: "0.162.1" },
        }}
      />,
    );
    expect(html).toContain("Managed by Editor");
    expect(html).toContain("WSL Ubuntu");
    expect(html).toContain("Untested version");
    expect(html).toContain("Last verified: 0.162.1");
  });

  it("a chat created after the person moved on does not take the view", () => {
    const transport = new FakeTransport();
    const creator = new NewChatDraft(transport);
    const composing = { selected: null, composing: true, draft: creator };
    // Still on the New chat that made it: the new chat opens.
    expect(createdChatOpens(composing, creator)).toBe(true);
    // The person opened another chat while it was being made: it stays where they are.
    expect(createdChatOpens({ ...composing, selected: "other" }, creator)).toBe(false);
    // Or began another New chat: that draft keeps the screen.
    expect(createdChatOpens({ ...composing, draft: new NewChatDraft(transport) }, creator)).toBe(
      false,
    );
    // Or left for the start screen.
    expect(createdChatOpens({ ...composing, composing: false }, creator)).toBe(false);
  });
});

describe("the chat list", () => {
  it("groups chats into projects by folder, with titles, status, folder state and attention", () => {
    const chats = [
      chatSummary("a", {
        folder: "/code/app",
        title: "Fix the test",
        status: "running",
        needs_attention: true,
      }),
      chatSummary("b", { folder: "/code/site", title: null, folder_state: "missing" }),
      chatSummary("c", { folder: "/code/app", title: "Older", folder_state: "not_allowed" }),
    ];
    expect(projectsOf(chats).map((p) => [p.folder, p.chats.map((c) => c.id)])).toEqual([
      ["/code/app", ["a", "c"]],
      ["/code/site", ["b"]],
    ]);
    expect(projectName("/Users/me/code/app/")).toBe("app");
    expect(projectName("C:\\code\\site")).toBe("site");
    const html = renderToStaticMarkup(
      <ChatList chats={chats} error={null} selected="a" onSelect={noop} onNewChat={noop} />,
    );
    expect(html).toContain("Fix the test");
    expect(html).toContain("New chat</span>");
    expect(html).toContain("Demo agent · Waiting for your answer");
    expect(html).toContain("folder missing");
    expect(html).toContain("folder not allowed");
    expect(html).toContain('aria-label="Waiting for your answer"');
    expect(html.match(/aria-current="true"/g)).toHaveLength(1);
  });

  it("numbers the visible chats for Ctrl+1 to Ctrl+9, skipping folded projects", () => {
    const projects = [
      { folder: "app", chats: [chatSummary("a1"), chatSummary("a2")] },
      { folder: "site", chats: [chatSummary("s1")] },
      { folder: "docs", chats: [chatSummary("d1")] },
    ];
    expect(visibleChatIds(projects, new Set(), false)).toEqual(["a1", "a2", "s1", "d1"]);
    expect(visibleChatIds(projects, new Set(["site"]), false)).toEqual(["a1", "a2", "d1"]);
    expect(visibleChatIds(projects, new Set(), true)).toEqual([]);
    const key = (overrides: Partial<Parameters<typeof chatShortcutIndex>[0]>) =>
      chatShortcutIndex({
        key: "1",
        code: "Digit1",
        ctrlKey: true,
        metaKey: false,
        altKey: false,
        isComposing: false,
        ...overrides,
      });
    expect(key({})).toBe(0);
    expect(key({ key: "9", code: "Digit9", ctrlKey: false, metaKey: true })).toBe(8);
    // AZERTY: the 1 key without Shift types "&", and its code still says Digit1.
    expect(key({ key: "&" })).toBe(0);
    expect(key({ key: "0", code: "Digit0" })).toBeNull();
    expect(key({ ctrlKey: false })).toBeNull();
    expect(key({ altKey: true })).toBeNull();
    expect(key({ isComposing: true })).toBeNull();
    expect(key({ key: "k", code: "KeyK" })).toBeNull();
  });

  it("tells assistive technology the first nine chats' shortcuts", () => {
    const chats = Array.from({ length: 10 }, (_, index) =>
      chatSummary(`c${index}`, { folder: "/code/app", title: `Chat ${index}` }),
    );
    const html = renderToStaticMarkup(
      <ChatList chats={chats} error={null} selected={null} onSelect={noop} onNewChat={noop} />,
    );
    expect(html).toContain('aria-keyshortcuts="Control+1 Meta+1"');
    expect(html).toContain('aria-keyshortcuts="Control+9 Meta+9"');
    expect(html).not.toContain("Control+10");
  });
});

describe("the agent lineup", () => {
  const ready = { state: "ready", version: "1.0.0", path: "/bin/agent" } as const;
  const missing = { state: "missing", reason: "Not installed." } as const;
  const broken = { state: "broken", reason: "Damaged." } as const;
  const twoWays = (id: string, active: string) => ({
    connections: {
      preferred: null,
      active,
      options: [id, `${id}-acp`].map((option) => ({
        id: option,
        label: option === id ? "Direct" : "ACP adapter",
        description: "",
        availability: option === active ? ready : missing,
        launch: option,
        needs_download: false,
      })),
    },
  });
  // As Rust reports them: one entry per agent. Claude Code runs natively and Codex through its
  // adapter, each still one agent.
  const catalog = [
    agent("hermes", broken),
    agent("claude", ready, twoWays("claude", "claude")),
    agent("codex", ready, twoWays("codex", "codex-acp")),
    agent("pi", { state: "checking" }),
    agent("cursor", missing),
    agent("mock", ready),
  ];

  it("splits the catalog once: usable agents and the others, nothing counted twice", () => {
    const { usable, others } = agentLineup(catalog);
    expect(usable.map((entry) => entry.id)).toEqual(["claude", "codex", "pi", "mock"]);
    expect(others.map((entry) => entry.id)).toEqual(["hermes", "cursor"]);
    expect(usable.length + others.length).toBe(catalog.length);
  });

  it("offers each usable agent once, the demo only alone", () => {
    expect(agentLineup(catalog).products.map((entry) => entry.id)).toEqual([
      "claude",
      "codex",
      "pi",
    ]);
    expect(
      agentLineup([agent("claude", missing), agent("mock", ready)]).products.map(
        (entry) => entry.id,
      ),
    ).toEqual(["mock"]);
  });

  it("is what New chat, the picker and onboarding all count", () => {
    const html = renderToStaticMarkup(
      <NewChat
        agents={catalog}
        refreshing={false}
        draft={new NewChatDraft(new FakeTransport())}
        onRefreshAgents={noop}
        onCreated={noop}
      />,
    );
    const radios = [...html.matchAll(/type="radio"[^>]*value="([^"]+)"/g)].map((match) => match[1]);
    expect(radios).toEqual(["claude", "codex", "pi", "mock"]);
    expect(html).not.toContain("-acp");
    expect(html).toMatch(/new-chat-more-count">2</);
    // The picker offers New chat's real agents; onboarding counts the same rows.
    const { products, others } = agentLineup(catalog);
    expect(products.map((entry) => entry.id)).toEqual(radios.filter((id) => id !== "mock"));
    const roster = primaryAgents(catalog);
    expect(roster.map((entry) => entry.id)).toEqual(["hermes", "claude", "codex", "pi", "cursor"]);
    expect(roster.filter((entry) => entry.availability.state === "ready").length).toBe(2);
    expect(roster.filter((entry) => !agentChoice(entry).enabled).length).toBe(others.length);
  });
});

describe("the sidebar footer", () => {
  it("is the logo and one Settings button, with no agents or profile row", () => {
    const html = renderToStaticMarkup(<SidebarFooter onSettings={noop} />);
    expect(html).toContain("chat-sidebar-footer-logo");
    expect(html).toContain('aria-label="Open settings"');
    expect(html).toContain('aria-keyshortcuts="Control+, Meta+,"');
    expect(html).not.toContain("g-agent-mark");
    expect(html).not.toContain("Stored on this device");
    expect(html).not.toContain("data-attention");
  });
});

describe("the header", () => {
  it.each([
    {
      name: "keeps a Local folder beside its custom project name",
      project: "Personal notes",
      folder: "C:/work/notes",
      folderLabel: "notes",
      expected: "Personal notes · notes",
    },
    {
      name: "does not repeat equal Local project and folder names",
      project: "repo",
      folder: "C:/work/repo",
      folderLabel: "repo",
      expected: "repo",
    },
    {
      name: "names the managed branch and its custom source project",
      project: "Client portal",
      folder: "C:/profile/worktrees/9d6cd062e86e",
      folderLabel: "ghost/9d6cd062 (worktree from C:/work/repo)",
      expected: "Client portal · ghost/9d6cd062 (worktree from C:/work/repo)",
    },
    {
      name: "uses the folder name while workspace metadata is unavailable",
      project: null,
      folder: "C:/work/notes",
      folderLabel: null,
      expected: "notes",
    },
  ])("$name", async ({ project, folder, folderLabel, expected }) => {
    const state = await stateWith(historyPage("c1", []), {
      chat: chatSummary("c1", { folder }),
    });
    const html = renderToStaticMarkup(
      <Header
        state={state}
        project={project}
        folderLabel={folderLabel}
        agent={null}
        resumable={false}
        onStart={noop}
        onStop={noop}
        onShowLog={noop}
      />,
    );
    const location = html.match(/<span class="chat-header-folder"[^>]*>(.*?)<\/span>/)?.[1];
    expect(location).toBe(expected);
  });

  it("offers Start or Resume, and Start fresh, while nothing runs; Stop session while it runs", async () => {
    const stopped = await stateWith(historyPage("c1", []), {
      session: sessionInfo({ status: "stopped" }),
    });
    const fresh = renderToStaticMarkup(
      <Header
        state={stopped}
        agent={null}
        resumable={false}
        onStart={noop}
        onStop={noop}
        onShowLog={noop}
      />,
    );
    // The menu ends on the session's lifecycle, after the quieter actions.
    expect(buttons(fresh).map((b) => b.label)).toEqual(["Show log", "Start session"]);
    const resumable = renderToStaticMarkup(
      <Header
        state={stopped}
        agent={null}
        resumable={true}
        onStart={noop}
        onStop={noop}
        onShowLog={noop}
      />,
    );
    expect(buttons(resumable).map((b) => b.label)).toEqual([
      "Show log",
      "Resume session",
      "Start fresh",
    ]);
    // The list has the newer folder state (it reloads after a failed start); the snapshot is from
    // the join.
    expect(fresh).not.toContain("folder missing");
    const missing = renderToStaticMarkup(
      <Header
        state={stopped}
        listed={chatSummary("c1", { folder_state: "missing", status: "failed" })}
        agent={null}
        resumable={false}
        onStart={noop}
        onStop={noop}
        onShowLog={noop}
      />,
    );
    expect(missing).toContain("folder missing");

    const running = await stateWith(historyPage("c1", []), {
      session: sessionInfo({
        status: "running",
        detail: "Applying your settings",
        agent_version: "0.22.5",
      }),
      usage: {
        context_used: 12_000,
        context_size: 200_000,
        cost: { amount: 0.02, currency: "USD" },
      },
    });
    const html = renderToStaticMarkup(
      <Header
        state={running}
        agent={null}
        resumable={true}
        onStart={noop}
        onStop={noop}
        onShowLog={noop}
      />,
    );
    expect(buttons(html).map((b) => b.label)).toEqual(["Show log", "Stop session"]);
    expect(html).toContain("Working");
    expect(html).toContain("Applying your settings");
    expect(html).toContain("0.22.5");
    // With detection's answer, the binary that runs is what the header names.
    const detected = renderToStaticMarkup(
      <Header
        state={running}
        agent={agent("mock", {
          state: "ready",
          version: "Claude Code 2.1.310 via claude-agent-acp 0.95.0",
          path: "/bin/claude",
        })}
        resumable={true}
        onStart={noop}
        onStop={noop}
        onShowLog={noop}
      />,
    );
    expect(detected).toContain("Claude Code 2.1.310 via claude-agent-acp 0.95.0");
    expect(detected).toContain('class="ui-mono">/bin/claude</span>');
    expect(html).toContain("Context 12k of 200k · $0.0200");
    expect(html).toContain("/Users/me/code/app");
  });
});

describe("the pickers", () => {
  const accessLists = (revision: number, overrides: Partial<ReturnType<typeof lists>> = {}) =>
    lists(revision, {
      capabilities: {
        images: false,
        steer: false,
        interrupt: true,
        resume: true,
        fork: false,
        rewind: false,
        set_model: true,
        set_effort: true,
        permissions: true,
        plans: false,
        usage: false,
        limits: false,
        slash_commands: false,
        add_dirs: false,
        plan_edits: false,
        autonomy_levels: ["ask", "auto_edit", "full_auto"],
      },
      ...overrides,
    });
  const model: ConfigOption = {
    id: "model",
    name: "Model",
    description: null,
    category: "model",
    kind: {
      type: "select",
      current: "fast",
      choices: [
        { value: "fast", name: "Fast", description: null, group: null },
        { value: "smart", name: "Smart", description: null, group: null },
      ],
    },
    pending: { type: "choice", value: "smart" },
  };
  const toggle: ConfigOption = {
    id: "fast",
    name: "Fast mode",
    description: null,
    category: "other",
    kind: { type: "toggle", current: true },
    pending: null,
  };

  const mode: ConfigOption = {
    id: "mode",
    name: "Mode",
    description: null,
    category: "mode",
    kind: {
      type: "select",
      current: "ask",
      choices: [
        { value: "ask", name: "Ask", description: null, group: null },
        { value: "auto", name: "Auto", description: null, group: null },
      ],
    },
    pending: null,
  };
  const effort: ConfigOption = {
    id: "effort",
    name: "Effort",
    description: null,
    category: "effort",
    kind: {
      type: "select",
      current: "xhigh",
      choices: ["default", "low", "xhigh"].map((value) => ({
        value,
        name: value === "default" ? "Default" : value,
        description: null,
        group: null,
      })),
    },
    pending: null,
  };

  it("keep model and thinking choosable for the next start while stopped, none while starting, and mode only when idle", () => {
    const render = (status: Parameters<typeof Pickers>[0]["status"]) =>
      renderToStaticMarkup(
        <Pickers
          lists={lists(1, { options: [toggle, mode, model] })}
          autonomy="full_auto"
          status={status}
          error={null}
          onSetAutonomy={noop}
          onSetOption={noop}
        />,
      );
    const stopped = render("stopped");
    expect(stopped).not.toMatch(/<button[^>]*data-picker-value[^>]*disabled/);
    expect(stopped).not.toMatch(/<input[^>]*type="checkbox"[^>]*disabled/);
    expect(stopped).toMatch(/<button[^>]*data-test-option-id="mode"[^>]*disabled/);
    expect(stopped).toContain("Applies when the agent starts.");
    expect(stopped).toContain("Automatic checkpoints are optional in");
    expect(stopped).not.toContain("A local checkpoint is saved before each turn.");
    expect(stopped).toContain("(pending: Smart)");
    expect(stopped.indexOf("Model")).toBeLessThan(stopped.indexOf("Fast mode"));
    const starting = render("starting");
    expect(starting.match(/<button[^>]*data-picker-value[^>]*disabled/g)).toHaveLength(2);
    expect(starting).toMatch(/<input[^>]*type="checkbox"[^>]*disabled/);
    expect(starting).toContain("Changes open once the agent has started.");
    const running = render("running");
    expect(running).not.toMatch(/<button[^>]*data-picker-value[^>]*disabled/);
    expect(running).toMatch(/<button[^>]*data-test-option-id="mode"[^>]*disabled/);
    expect(running).not.toContain("Applies when the agent starts.");
    const ready = render("ready");
    expect(ready).not.toMatch(/disabled/);
  });

  it("read model, then thinking in plain words, then agents, and put the level on the pill", () => {
    const html = renderToStaticMarkup(
      <Pickers
        lists={lists(1, { options: [model, effort, toggle] })}
        autonomy="full_auto"
        status="ready"
        error={null}
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    // Owner, 4 November 2026: "model and thinking mode then harnesses".
    const thinking = html.indexOf(">Thinking</h3>");
    expect(thinking).toBeGreaterThan(-1);
    expect(html.indexOf('data-picker-value="fast"')).toBeLessThan(thinking);
    expect(thinking).toBeLessThan(html.indexOf(">Agent</h3>"));
    expect(html.indexOf(">Agent</h3>")).toBeLessThan(html.indexOf("More options"));
    // The big title over the menu is gone; the pill names the model.
    expect(html).not.toContain("chat-model-head");
    expect(html).not.toContain(">Effort<");
    expect(html).toMatch(/data-picker-value="xhigh" aria-pressed="true"[^>]*><span>Extra high</);
    expect(html).toMatch(/data-picker-value="low"[^>]*><span>Low</);
    expect(html).toContain('class="chat-pill-level">Extra high</span>');
    expect(html).toContain('class="chat-pill-level-short">X-high</span>');
    expect(html).toContain('aria-label="Fast, Extra high thinking. Agent, model and thinking"');
    // A model without levels says so where the levels would be.
    const plain = renderToStaticMarkup(
      <Pickers
        lists={lists(1, { options: [model] })}
        autonomy="full_auto"
        status="ready"
        error={null}
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    expect(plain).toContain("This model has no thinking levels.");
    expect(plain).not.toContain("chat-pill-effort");
    expect(plain).toMatch(/data-testid="picker-current-model">Fast</);
    // Before an agent has run, its stopped chat says where models will come from.
    const unknown = renderToStaticMarkup(
      <Pickers
        lists={null}
        autonomy="full_auto"
        status="stopped"
        error={null}
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    expect(unknown).toContain("Models show once this agent has run.");
    expect(unknown).toContain("Shown once the agent runs.");
    expect(unknown).not.toContain("Applies when the agent starts.");
  });

  it("fold a long model list after five, keeping the chosen model in view", () => {
    const names = Array.from({ length: 13 }, (_, index) => `Model ${index + 1}`);
    const long: ConfigOption = {
      ...model,
      pending: null,
      kind: {
        type: "select",
        current: "m9",
        choices: names.map((name, index) => ({
          value: `m${index + 1}`,
          name,
          description: null,
          group: null,
        })),
      },
    };
    const html = renderToStaticMarkup(
      <Pickers
        lists={lists(1, { options: [long] })}
        autonomy="full_auto"
        status="ready"
        error={null}
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    const shown = [...html.matchAll(/data-picker-value="(m\d+)"/g)].map((match) => match[1]);
    expect(shown).toEqual(["m1", "m2", "m3", "m4", "m5", "m9"]);
    expect(html).toMatch(
      /data-testid="picker-more-models"><span>More models<\/span><small>7<span class="sr-only"> more<\/span><\/small>/,
    );
    // One model past the fold is no fold.
    expect(
      foldedModels(
        names.slice(0, 6).map((value) => ({ value })),
        "x",
      ),
    ).toHaveLength(6);
    expect(
      foldedModels(
        names.slice(0, 7).map((value) => ({ value })),
        "x",
      ),
    ).toHaveLength(5);
  });

  it("name the model an alias stands for on the pill, and leave a Default level off it", () => {
    const alias: ConfigOption = {
      ...model,
      pending: null,
      kind: {
        type: "select",
        current: "default",
        choices: [
          {
            value: "default",
            name: "Default (recommended)",
            description: "Opus 5.5 · Best for everyday, complex tasks",
            group: null,
          },
          { value: "opus", name: "Opus 5.5", description: null, group: null },
        ],
      },
    };
    const atDefault: ConfigOption = {
      ...effort,
      kind: { ...effort.kind, current: "default" } as ConfigOption["kind"],
    };
    const html = renderToStaticMarkup(
      <Pickers
        lists={lists(1, { options: [alias, atDefault] })}
        autonomy="full_auto"
        status="ready"
        error={null}
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    expect(html).toMatch(/data-testid="picker-current-model">Opus 5\.5</);
    expect(html).not.toContain("chat-pill-effort");
    // Readers still hear the level in effect.
    expect(html).toContain('aria-label="Opus 5.5, Default thinking. Agent, model and thinking"');
    // A description naming no listed model leaves the alias's own name.
    if (alias.kind.type !== "select") throw new Error("select");
    const unlisted = { ...alias.kind.choices[0], description: "Mythos · Unreleased" };
    expect(shownModelName(alias, unlisted as ConfigChoice)).toBe("Default (recommended)");
  });

  it("say why a change was refused inside the model menu", () => {
    const html = renderToStaticMarkup(
      <Pickers
        lists={lists(1, { options: [model, effort] })}
        autonomy="full_auto"
        status="ready"
        error={null}
        optionError="That model isn't available on your plan."
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    const menu = html.slice(html.indexOf('class="chat-popover chat-model-popover"'));
    expect(menu).toMatch(
      /role="alert" data-testid="picker-option-error">That model isn&#x27;t available on your plan\./,
    );
    expect(html).toContain(
      'aria-label="A change was refused. Open the agent, model and thinking picker for details."',
    );
  });

  it("show the value a change on its way asked for, marked, instead of the old one", () => {
    const html = renderToStaticMarkup(
      <Pickers
        lists={accessLists(1, { options: [toggle, { ...model, pending: null }] })}
        autonomy="full_auto"
        status="ready"
        error={null}
        optionsSaving={
          new Map([
            ["model", { type: "choice" as const, value: "smart" }],
            ["fast", { type: "toggle" as const, value: false }],
          ])
        }
        autonomySaving="ask"
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    expect(html).toContain('aria-label="Ask for approval. Access level"');
    expect(html).toContain('class="chat-control-pending">Saving…');
    expect(html).toMatch(/data-picker-value="smart" aria-pressed="true"/);
    expect(html).not.toMatch(/<input[^>]*checked/);
    expect(html.match(/\(saving…\)/g)).toHaveLength(2);
    // Nothing on its way: the lists' own values.
    const settledHtml = renderToStaticMarkup(
      <Pickers
        lists={lists(1, { options: [toggle, { ...model, pending: null }] })}
        autonomy="full_auto"
        status="ready"
        error={null}
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    expect(settledHtml).toMatch(/data-picker-value="fast" aria-pressed="true"/);
    expect(settledHtml).toMatch(/<input[^>]*checked/);
    expect(settledHtml).not.toContain("(saving…)");
  });

  it("show the autonomy note and the agent mode in effect", () => {
    const html = renderToStaticMarkup(
      <Pickers
        lists={accessLists(2, {
          autonomy: {
            requested: "ask",
            native_mode: "acceptEdits",
            auto_answer: { type: "never" },
            note: "Applies when the agent starts.",
          },
        })}
        autonomy="full_auto"
        status="stopped"
        error="The agent refused that model."
        onSetAutonomy={noop}
        onSetOption={noop}
      />,
    );
    expect(html).toContain('aria-label="Ask for approval. Access level"');
    expect(html).not.toContain("changes are not checkpointed yet");
    expect(html).toContain("Applies when the agent starts.");
    expect(html).toContain("Native setting: acceptEdits");
    expect(html).toContain("The agent refused that model.");
  });
});

describe("the queue and the composer's stop button", () => {
  it("marks held prompts and offers Send queued for them", () => {
    const html = renderToStaticMarkup(
      <QueueList
        queue={[
          { prompt_id: "p1", preview: "first", held: true, created_at: 1 },
          { prompt_id: "p2", preview: "second", held: false, created_at: 2 },
        ]}
        held={true}
        onRemove={noop}
        onSendQueued={noop}
      />,
    );
    expect(html.match(/>held</g)).toHaveLength(1);
    expect(buttons(html).map((b) => b.label)).toEqual([
      "Send queued",
      "Remove from the queue",
      "Remove from the queue",
    ]);
    expect(
      renderToStaticMarkup(
        <QueueList queue={[]} held={false} onRemove={noop} onSendQueued={noop} />,
      ),
    ).toBe("");
  });

  it("offers Send queued whenever the session says prompts are held, marked or not", () => {
    const unmarked = [{ prompt_id: "p1", preview: "first", held: false, created_at: 1 }];
    const held = renderToStaticMarkup(
      <QueueList queue={unmarked} held={true} onRemove={noop} onSendQueued={noop} />,
    );
    expect(buttons(held).map((b) => b.label)).toEqual(["Send queued", "Remove from the queue"]);
    const none = renderToStaticMarkup(
      <QueueList queue={unmarked} held={false} onRemove={noop} onSendQueued={noop} />,
    );
    expect(buttons(none).map((b) => b.label)).toEqual(["Remove from the queue"]);
    // A held prompt the list does not know yet (its join is on its way) still gets the button.
    const unknown = renderToStaticMarkup(
      <QueueList queue={[]} held={true} onRemove={noop} onSendQueued={noop} />,
    );
    expect(buttons(unknown).map((b) => b.label)).toEqual(["Send queued"]);
  });

  it("edits and reorders queued messages with buttons, never by dragging alone", () => {
    const html = renderToStaticMarkup(
      <QueueList
        queue={[
          { prompt_id: "p1", preview: "first", held: false, created_at: 1 },
          { prompt_id: "p2", preview: "second", held: false, created_at: 2 },
        ]}
        held={false}
        onRemove={noop}
        onSendQueued={noop}
        onLoadText={async () => "first"}
        onEdit={async () => null}
        onMove={async () => {}}
      />,
    );
    expect(buttons(html)).toEqual([
      { label: "Edit queued message", disabled: false },
      { label: "Move up in the queue", disabled: true },
      { label: "Move down in the queue", disabled: false },
      { label: "Remove from the queue", disabled: false },
      { label: "Edit queued message", disabled: false },
      { label: "Move up in the queue", disabled: false },
      { label: "Move down in the queue", disabled: true },
      { label: "Remove from the queue", disabled: false },
    ]);
    expect(html).not.toContain("draggable");
  });

  it("opens a queued message's words in a labelled field with Cancel and Save", () => {
    const html = renderToStaticMarkup(
      <QueueEditor initial="first draft" onSave={async () => null} onCancel={noop} />,
    );
    expect(html).toContain('aria-label="Queued message"');
    expect(html).toContain(">first draft</textarea>");
    expect(buttons(html).map((b) => b.label)).toEqual(["Cancel", "Save"]);
  });

  it("escape_twice: Stop while a turn runs or starts, Force stop while it cancels", () => {
    const label = (status: Parameters<typeof StopButton>[0]["status"]) =>
      buttons(
        renderToStaticMarkup(<StopButton status={status} onStop={noop} onForceStop={noop} />),
      ).map((b) => b.label);
    expect(label("running")).toEqual(["Stop"]);
    expect(label("starting")).toEqual(["Stop"]);
    expect(label("cancelling")).toEqual(["Force stop"]);
    expect(label("ready")).toEqual([]);
    expect(label("stopped")).toEqual([]);
  });

  it("says an agent that cannot steer takes a typed message after the turn", () => {
    const controls = (status: "running" | "starting" | "ready", steerable: boolean) =>
      renderToStaticMarkup(
        <SendControls
          status={status}
          steerable={steerable}
          disabled={false}
          agentName="Claude Code"
          onSend={noop}
          onSteer={noop}
        />,
      );
    // Claude Code and Pi during a turn: calm words where Steer would be, Queue as the action.
    const claude = controls("running", false);
    expect(claude).toContain('data-testid="composer-after-turn"');
    expect(claude).toContain(">After this turn</span>");
    expect(claude).not.toContain("composer-steer");
    expect(buttons(claude)).toEqual([{ label: "Queue message", disabled: false }]);
    expect(claude).toMatch(/aria-describedby="[^"]+"/);
    expect(claude).toContain("Queue for Claude Code after this turn (Enter)");
    // Codex steers: Steer in words beside the Queue disc, and no after-turn note.
    const codex = controls("running", true);
    expect(buttons(codex).map((b) => b.label)).toEqual(["Steer", "Queue message"]);
    expect(codex).not.toContain("After this turn");
    // While the agent starts nothing can steer yet, and idle the disc simply sends.
    expect(controls("starting", false)).not.toContain("After this turn");
    expect(buttons(controls("ready", false))).toEqual([{ label: "Send message", disabled: false }]);
  });

  it("still_working_indicator: counts the minutes and offers Stop", () => {
    expect(renderToStaticMarkup(<StillWorking minutes={1} onStop={noop} />)).toContain(
      "Still working (no output for 1 minute)",
    );
    const two = renderToStaticMarkup(<StillWorking minutes={2} onStop={noop} />);
    expect(two).toContain("Still working (no output for 2 minutes)");
    expect(buttons(two).map((b) => b.label)).toEqual(["Stop"]);
    // With a message waiting behind the quiet turn, it also offers to send that one now.
    const waiting = renderToStaticMarkup(
      <StillWorking minutes={1} onStop={noop} onSendNow={noop} />,
    );
    expect(buttons(waiting).map((b) => b.label)).toEqual(["Send now", "Stop"]);
  });

  it("says why queued messages wait, with the one action that helps", () => {
    const queue = [
      { prompt_id: "p1", preview: "first", held: false, created_at: 1 },
      { prompt_id: "p2", preview: "second", held: false, created_at: 2 },
    ];
    const render = (wait: QueueWait, context = { canInterrupt: true, canRecreate: false }) =>
      renderToStaticMarkup(
        <QueueList
          queue={queue}
          held={false}
          wait={queueWaitView(wait, "Claude Code", context)}
          onRemove={noop}
          onSendQueued={noop}
          onSendNow={noop}
          onStart={noop}
          onRecreate={noop}
        />,
      );
    const working = render({ reason: "working" });
    expect(working).toContain('data-test-reason="working"');
    expect(working).toContain("Claude Code is still working");
    expect(working).toContain(" · 2 queued");
    expect(buttons(working).map((b) => b.label)).toEqual([
      "Send now",
      "Remove from the queue",
      "Remove from the queue",
    ]);
    const starting = render({ reason: "starting" });
    expect(starting).toContain("Starting Claude Code");
    expect(starting).not.toMatch(/>2 queued</);
    expect(buttons(starting).map((b) => b.label)).not.toContain("Send now");
    const finishing = render({ reason: "finishing_turn", since: 1, sends_at: 15_001 });
    expect(finishing).toContain("Waiting for Claude Code to finish its turn");
    expect(buttons(finishing)[0]?.label).toBe("Send now");
    expect(render({ reason: "needs_answer", question: false })).toContain(
      "Answer the permission request first",
    );
    const failed = render({ reason: "start_failed" });
    expect(failed).toContain("data-failed");
    expect(buttons(failed)[0]?.label).toBe("Retry");
    expect(buttons(render({ reason: "stopped" }))[0]?.label).toBe("Start");
    const missing = render({ reason: "folder_missing" }, { canInterrupt: true, canRecreate: true });
    expect(missing).toContain("This worktree&#x27;s folder is missing");
    expect(buttons(missing)[0]?.label).toBe("Recreate folder");
    // An agent that cannot interrupt gets no Send now while it works.
    expect(
      buttons(render({ reason: "working" }, { canInterrupt: false, canRecreate: false })).map(
        (b) => b.label,
      ),
    ).not.toContain("Send now");
  });

  it("says what held the messages and lets one be sent on its own", () => {
    const html = renderToStaticMarkup(
      <QueueList
        queue={[
          { prompt_id: "p1", preview: "first", held: true, created_at: 1 },
          { prompt_id: "p2", preview: "second", held: true, created_at: 2 },
        ]}
        held={true}
        wait={queueWaitView({ reason: "held", after: "restart" }, "Grok Build", {
          canInterrupt: true,
          canRecreate: false,
        })}
        onRemove={noop}
        onSendQueued={noop}
        onSendNow={noop}
      />,
    );
    expect(html).toContain("Held after a restart");
    expect(html).toContain('data-test-reason="held"');
    // The reason replaces the general note under the list.
    expect(html).not.toContain("chat-queue-note");
    // The head acts on the first held message; the later held one keeps its own Send.
    expect(buttons(html).map((b) => b.label)).toEqual([
      "Send it now",
      "Discard",
      "Send all",
      "Remove from the queue",
      "Send this message now",
      "Remove from the queue",
    ]);
  });

  it("keeps a new message behind a held one, offering Send it now or Discard for the held one", () => {
    const queue = [
      { prompt_id: "held", preview: "typed before the restart", held: true, created_at: 1 },
      { prompt_id: "new", preview: "typed after it", held: false, created_at: 2 },
    ];
    const wait = queueWaitView({ reason: "held", after: "restart" }, "Claude Code", {
      canInterrupt: true,
      canRecreate: false,
    });
    const html = renderToStaticMarkup(
      <QueueList
        queue={queue}
        held={true}
        wait={wait}
        onRemove={noop}
        onSendQueued={noop}
        onSendNow={noop}
      />,
    );
    expect(html).toContain("Held after a restart");
    expect(html).toContain("2 queued");
    // One held message: no Send all, and no second Send on its own row.
    expect(buttons(html).map((b) => b.label)).toEqual([
      "Send it now",
      "Discard",
      "Remove from the queue",
      "Remove from the queue",
    ]);
    expect(html).toContain('data-testid="queue-send-held"');
    expect(html).toContain('data-testid="queue-discard-held"');
    expect(html).not.toContain('data-testid="send-queued"');
    expect(wait?.detail).toContain("Send it now, or discard it.");
  });

  it("says a message typed while the agent starts goes once it has started", () => {
    const html = renderToStaticMarkup(
      <SendControls
        status="starting"
        steerable={false}
        disabled={false}
        agentName="Grok Build"
        onSend={noop}
        onSteer={noop}
      />,
    );
    expect(html).toContain('data-testid="composer-after-start"');
    expect(html).toContain(">Once started</span>");
    expect(buttons(html)).toEqual([{ label: "Send once started", disabled: false }]);
    expect(html).toContain("Send once Grok Build has started (Enter)");
  });
});

describe("turns and the transcript", () => {
  it("ends a turn with its stop reason, tokens with their scope, duration and cost", () => {
    const model = applyEntries(
      emptyTranscript(),
      numbered([
        ["t1", ev.turnStarted("t1")],
        ["t1", ev.usage(0.0123)],
        [
          "t1",
          {
            type: "turn_ended",
            turn_id: "t1",
            stop_reason: "max_tokens",
            usage: {
              input_tokens: 1200,
              output_tokens: 300,
              cache_read_tokens: null,
              cache_write_tokens: null,
              reasoning_tokens: null,
              scope: "turn",
            },
            duration_ms: 8200,
            recovered: false,
          },
        ],
        ["t2", ev.turnStarted("t2")],
        ["t2", ev.turnEnded("t2", "interrupted", { recovered: true })],
      ]),
    );
    const [first, second] = renderTranscript(model, false);
    if (!first || !second) throw new Error("two turns");
    const html = renderToStaticMarkup(<TurnView segment={first} actions={ACTIONS} />);
    expect(html).toContain("Stopped at the output limit");
    expect(html).toContain("1.2k in · 300 out (this turn)");
    expect(html).toContain("8.2 s");
    expect(html).toContain("$0.0123");
    const recovered = renderToStaticMarkup(<TurnView segment={second} actions={ACTIONS} />);
    expect(recovered).toContain("Interrupted when Ghost closed");
    expect(buttons(recovered).map((b) => b.label)).toEqual(["Copy turn receipt", "Resume session"]);
    const live = renderToStaticMarkup(
      <TurnView segment={second} actions={{ ...ACTIONS, canResume: false }} />,
    );
    expect(buttons(live)).toEqual([{ label: "Copy turn receipt", disabled: false }]);
  });

  it("partial_turn_page: Load earlier, and the marker of a turn the page cut", async () => {
    const page = historyPage(
      "c1",
      items(
        numbered(
          [
            ["t1", ev.message("t1.m1", "the end of a long turn")],
            ["t1", ev.turnEnded("t1")],
          ],
          40,
        ),
      ),
      { has_more: true, before_seq: 40, cut_turn_id: "t1" },
    );
    const state = await stateWith(page);
    const html = renderToStaticMarkup(
      <Transcript state={state} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    expect(buttons(html).map((b) => b.label)).toContain("Load earlier");
    expect(html).toContain("Earlier part of this turn not loaded");
    expect(html).toContain("the end of a long turn");
    expect(html.indexOf("Load earlier")).toBeLessThan(html.indexOf("Earlier part of this turn"));
  });

  it("shows an attachment-only sending message without inventing a text preview", async () => {
    const state = await stateWith(historyPage("c1", []), {
      session: sessionInfo({
        status: "running",
        sending: {
          prompt_id: "image-prompt",
          text: "",
          parts: [
            { type: "image", blob: "a".repeat(64), mime: "image/png" },
            { type: "mention", path: "src/main.rs" },
          ],
          created_at: 123,
        },
      }),
    });
    const html = renderToStaticMarkup(
      <Transcript state={state} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    expect(html).toContain('data-test-prompt-id="image-prompt"');
    expect(html).toContain("Attached image (image/png)");
    expect(html).toContain("Mentioned file: src/main.rs");
    expect(html).toContain("Sending");
    expect(html.match(/data-test-kind="user"/g)).toHaveLength(1);
  });

  it("labels each turn with the agent that ran it after the chat switched agents", async () => {
    const page = historyPage(
      "c1",
      items(
        numbered([
          ["t1", ev.turnStarted("t1")],
          ["t1", ev.message("t1.m1", "Claude Code answered")],
          ["t1", ev.turnEnded("t1")],
          ["t2", ev.turnStarted("t2")],
          ["t2", ev.message("t2.m1", "Codex answered")],
          ["t2", ev.turnEnded("t2")],
        ]),
      ),
      {
        turn_agents: [
          { turn_id: "t1", agent_id: "claude", agent_name: "Claude Code" },
          { turn_id: "t2", agent_id: "codex", agent_name: "Codex" },
        ],
      },
    );
    const state = await stateWith(page, {
      chat: chatSummary("c1", { agent_id: "codex", agent_name: "Codex" }),
      session: sessionInfo({ agent_name: "Codex" }),
    });
    const html = renderToStaticMarkup(
      <Transcript state={state} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    // Each turn's byline names its own agent.
    const bylines = [
      ...html.matchAll(/data-test-turn-id="(t\d)"[^>]*><span class="sr-only">([^<]+)</g),
    ].map((match) => [match[1], match[2]]);
    expect(bylines).toEqual([
      ["t1", "Claude Code"],
      ["t2", "Codex"],
    ]);
  });

  it("gathers reasoning into the stretch of calls around it, never a row of its own", async () => {
    const page = historyPage(
      "c1",
      items(
        numbered([
          ["t1", ev.turnStarted("t1")],
          ["t1", ev.user("t1.u", "Fix the sidebar")],
          ["t1", ev.thought("t1.th1", "")],
          ["t1", ev.thoughtDone("t1.th1", 0)],
          ["t1", ev.toolCall("a", { title: "Read Sidebar.tsx", status: "completed" })],
          ["t1", ev.thought("t1.th2", "Now look for its callers.")],
          ["t1", ev.thoughtDone("t1.th2", 1200)],
          [
            "t1",
            ev.toolCall("b", { kind: "search", title: "Search for Sidebar", status: "completed" }),
          ],
          ["t1", ev.toolCall("c", { title: "Read App.tsx", status: "completed" })],
          ["t1", ev.thought("t1.th3", "Done reading.")],
          ["t1", ev.message("t1.m1", "The sidebar is fixed.")],
          ["t1", ev.messageDone("t1.m1")],
          ["t1", ev.toolCall("d", { kind: "execute", title: "pnpm test", status: "completed" })],
          ["t1", ev.turnEnded("t1")],
        ]),
      ),
    );
    const state = await stateWith(page);
    const html = renderToStaticMarkup(
      <Transcript state={state} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    // Reasoning between calls is part of their stretch (owner, 23 November 2026): no row of its
    // own, so the closed stretch shows none of it, words or time.
    expect(html).not.toContain('class="chat-thought-label"');
    expect(html).not.toContain("Thinking");
    expect(html).not.toContain("Now look for its callers.");
    expect(html).not.toContain('data-testid="thought-preview"');
    // One stretch for the three calls and the reasoning among them, then the call after the answer.
    expect(
      [...html.matchAll(/data-testid="tool-group" data-test-count="(\d+)"/g)].map(
        (match) => match[1],
      ),
    ).toEqual(["3", "1"]);
    // A stretch leads with its kinds, as their icons.
    expect(html).toContain('class="chat-tool-group-kinds"');
    // A single call reads as its own line, past tense once done; settled stretches stay closed.
    expect(html).toMatch(
      /class="chat-tool-verb">Ran<\/span> <span class="chat-tool-object" data-literal="">pnpm test</,
    );
    expect(html).not.toContain('data-test-kind="tool"');
    expect(html).toContain('data-testid="turn-footer"');
    expect(html).not.toContain('data-testid="working-line"');
  });

  it("shows reasoning with no call in it as one row over the answer", async () => {
    const page = historyPage(
      "c1",
      items(
        numbered([
          ["t1", ev.turnStarted("t1")],
          ["t1", ev.user("t1.u", "Name the layout")],
          ["t1", ev.thought("t1.th1", "Look at the grid.")],
          ["t1", ev.thoughtDone("t1.th1", 1400)],
          ["t1", ev.thought("t1.th2", "It is a two-column grid.")],
          ["t1", ev.thoughtDone("t1.th2", 1700)],
          ["t1", ev.message("t1.m1", "A two-column grid.")],
          ["t1", ev.messageDone("t1.m1")],
          ["t1", ev.turnEnded("t1")],
        ]),
      ),
    );
    const html = renderToStaticMarkup(
      <Transcript
        state={await stateWith(page)}
        live={true}
        actions={ACTIONS}
        onLoadEarlier={noop}
      />,
    );
    // Both pieces read as one: their whole time, and every word in order once opened.
    expect(
      [...html.matchAll(/class="chat-thought-label">([^<]*)</g)].map((match) => match[1]),
    ).toEqual(["Thought for 3 s"]);
    expect(html.indexOf("Look at the grid.")).toBeLessThan(
      html.indexOf("It is a two-column grid."),
    );
    expect(html.indexOf("Thought for 3 s")).toBeLessThan(html.indexOf("A two-column grid.</"));
    expect(html).not.toContain('data-testid="tool-group"');
  });

  it("keeps the working line at a running turn's tail, aside while text streams", async () => {
    const running = (events: Parameters<typeof numbered>[0]) =>
      stateWith(historyPage("c1", items(numbered(events))), {
        session: sessionInfo({ status: "running" }),
      });
    const thinking = await running([
      ["t1", ev.turnStarted("t1")],
      ["t1", ev.user("t1.u", "Check the layout")],
      ["t1", ev.thought("t1.th", "Checking the narrow layout")],
    ]);
    const html = renderToStaticMarkup(
      <Transcript state={thinking} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    // Reasoning streaming in shows on the working line: its newest words under the verb, and no
    // row of its own.
    expect(html).toContain('data-testid="working-line"');
    expect(html).not.toMatch(/data-testid="working-line" data-hidden=""/);
    expect(html).toContain("is working");
    expect(html).toMatch(/data-testid="working-reasoning">Checking the narrow layout</);
    expect(html).not.toContain('class="chat-thought-label"');
    expect(html).not.toContain('data-testid="thought-preview"');
    // No second live row below it, and no footer yet.
    expect(html).not.toContain('class="chat-now"');
    expect(html).not.toContain('data-testid="turn-footer"');
    const streaming = await running([
      ["t1", ev.turnStarted("t1")],
      ["t1", ev.user("t1.u", "Check the layout")],
      ["t1", ev.message("t1.m", "The layout holds")],
    ]);
    const aside = renderToStaticMarkup(
      <Transcript state={streaming} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    expect(aside).toMatch(/data-testid="working-line" data-hidden=""/);
    const waiting = await running([
      ["t1", ev.turnStarted("t1")],
      ["t1", ev.toolCall("cmd", { kind: "execute", title: "pnpm test" })],
      ["t1", ev.permission("p")],
    ]);
    const asking = renderToStaticMarkup(
      <Transcript state={waiting} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    expect(asking).toContain("Waiting on you");
    expect(asking).not.toContain('data-testid="working-line"');
    const ended = await stateWith(
      historyPage(
        "c1",
        items(
          numbered([
            ["t1", ev.turnStarted("t1")],
            ["t1", ev.user("t1.u", "Check the layout")],
            ["t1", ev.thought("t1.th", "Checking the narrow layout")],
            ["t1", ev.message("t1.m", "The layout holds.")],
            ["t1", ev.messageDone("t1.m")],
            ["t1", ev.turnEnded("t1", "end_turn", { durationMs: 4200 })],
          ]),
        ),
      ),
      { session: sessionInfo({ status: "ready" }) },
    );
    const done = renderToStaticMarkup(
      <Transcript state={ended} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    expect(done).not.toContain('data-testid="working-line"');
    expect(done).toContain('data-testid="turn-footer"');
    expect(done).toContain("4.2 s");
  });

  it("unsaved_marker_and_reattach: marks the unsaved rows where they are missing", async () => {
    const page = historyPage("c1", items(numbered([[null, ev.notice("saved before the pause")]])), {
      unsaved_from: 1,
    });
    const state = await stateWith(page);
    const html = renderToStaticMarkup(
      <Transcript state={state} live={true} actions={ACTIONS} onLoadEarlier={noop} />,
    );
    expect(html).toContain(
      "Recent output isn&#x27;t saved yet; it appears here when saving resumes.",
    );
    expect(html.indexOf("saved before the pause")).toBeLessThan(html.indexOf("Recent output"));
  });
});

describe("the banners", () => {
  it("show the Terms of Service and Privacy Policy with their links until OK", () => {
    const html = renderToStaticMarkup(
      <Banners
        storage={null}
        legal={{
          title: "Ghost has Terms of Service and a Privacy Policy",
          detail: "From BASH.AI LLC.",
          onAccept: noop,
        }}
      />,
    );
    expect(html).toContain("Ghost has Terms of Service and a Privacy Policy");
    expect(html).toContain('href="https://onghost.dev/terms"');
    expect(html).toContain('href="https://onghost.dev/privacy"');
    expect(html).toContain('target="_blank"');
    expect(buttons(html)).toEqual([{ label: "OK", disabled: false }]);
    expect(renderToStaticMarkup(<Banners storage={null} legal={null} />)).toBe("");
  });

  it("show the database repair notice until the shell confirms its dismissal", () => {
    const message = "Your database was damaged, so Ghost rebuilt it.";
    const shown = renderToStaticMarkup(
      <RecoveryBannerView
        message={message}
        state={{ status: "shown", problem: null }}
        onDismiss={noop}
      />,
    );
    expect(shown).toContain(message);
    expect(buttons(shown)).toEqual([{ label: "Dismiss", disabled: false }]);
    const busy = renderToStaticMarkup(
      <RecoveryBannerView message={message} state={{ status: "dismissing" }} onDismiss={noop} />,
    );
    expect(buttons(busy)).toEqual([{ label: "Dismissing…", disabled: true }]);
    const failed = renderToStaticMarkup(
      <RecoveryBannerView
        message={message}
        state={{
          status: "shown",
          problem: "Could not dismiss the notice: the database is closed.",
        }}
        onDismiss={noop}
      />,
    );
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("the database is closed.");
    expect(
      renderToStaticMarkup(
        <RecoveryBannerView message={message} state={{ status: "dismissed" }} onDismiss={noop} />,
      ),
    ).toBe("");
  });

  it("float only paused or failed saving; the panel lists it with the env fallback and notices", () => {
    expect(storageBanner(null)).toBeNull();
    expect(storageBanner({ state: { state: "ok" }, unsaved_bytes: 0 })).toBeNull();
    expect(
      storageBanner({
        state: { state: "paused", reason: "disk_full", since: 1 },
        unsaved_bytes: 9,
      }),
    ).toBe(
      "The disk is full. Ghost paused saving and the agents are waiting. Free some space; nothing is lost unless you quit now.",
    );
    expect(
      storageBanner({ state: { state: "failed", message: "damaged" }, unsaved_bytes: 0 }),
    ).toBe("damaged");
    expect(envBanner({ state: { state: "resolving" }, removed: [] })).toBeNull();
    expect(
      envBanner({ state: { state: "fallback", reason: "the shell timed out" }, removed: [] }),
    ).toContain("(the shell timed out)");
    const paused = {
      state: { state: "paused" as const, reason: "busy" as const, since: 1 },
      unsaved_bytes: 9,
    };
    const floating = renderToStaticMarkup(<Banners storage={paused} />);
    expect(floating).toContain("database is busy");
    expect(buttons(floating)).toEqual([]);
    expect(renderToStaticMarkup(<Banners storage={null} />)).toBe("");
    const items = noticeItems({
      storage: paused,
      env: { state: { state: "fallback", reason: "no PATH" }, removed: [] },
      onRetryEnv: noop,
      recoveryNotice: "Ghost repaired its database; the damaged copy is kept.",
      onRecoveryDismissed: noop,
      notices: [
        "Ghost stopped 1 agent process left running by an earlier session that ended unexpectedly.",
      ],
      onDismissNotice: noop,
    });
    expect(items.map((item) => [item.key, item.level ?? null])).toEqual([
      ["storage:paused", "warning"],
      ["env", "warning"],
      ["recovery", "warning"],
      [
        "notice:Ghost stopped 1 agent process left running by an earlier session that ended unexpectedly.",
        null,
      ],
    ]);
    const panel = renderToStaticMarkup(<ul>{items.map((item) => item.node)}</ul>);
    expect(panel).toContain("database is busy");
    expect(panel).toContain("(no PATH)");
    expect(panel).toContain("Ghost stopped 1 agent process");
    expect(buttons(panel).map((b) => b.label)).toEqual(["Retry", "Dismiss", "Dismiss"]);
    const failed = noticeItems({
      storage: { state: { state: "failed", message: "damaged" }, unsaved_bytes: 0 },
      env: null,
      onRetryEnv: noop,
      recoveryNotice: null,
      onRecoveryDismissed: noop,
      notices: [],
      onDismissNotice: noop,
    });
    expect(failed.map((item) => [item.key, item.level])).toEqual([["storage:failed", "error"]]);
  });
});
