import type { AgentBrandId } from "@ghost/ui";
import type {
  AgentEffort,
  AgentInfo,
  AgentModel,
  Autonomy,
  ChatSummary,
  Composer,
  Project,
  Workspace,
} from "./types";

/**
 * The workspace around the golden chat: three projects, their chats, and the agent dock. Fixed
 * text throughout (relative times included), so every direction and every screenshot shows the same
 * sidebar. Brand ids are those of packages/ui/src/brand.ts.
 */

export const PROJECT_AURACODE = "project-auracode";
export const PROJECT_SITE = "project-auracode-si";
export const PROJECT_DOTFILES = "project-dotfiles";

/** The golden chat: Claude Code fixing a flaky test (the `streaming`, `settled`, `picker` states). */
export const CHAT_PACING = "chat-orb-pacing";
/** Codex waiting for approval to run the tests (the `permission` state). */
export const CHAT_ACP = "chat-acp-client";
export const CHAT_HERO = "chat-landing-hero";
export const CHAT_TRAY = "chat-tray-icons";
export const CHAT_DOWNLOAD = "chat-download-page";
export const CHAT_SIGNIN = "chat-signin-email";
export const CHAT_ZSH = "chat-zsh-startup";

const effort = (id: string, label: string): AgentEffort => ({ id, label });
const LOW = effort("low", "Low");
const MEDIUM = effort("medium", "Medium");
const HIGH = effort("high", "High");
const XHIGH = effort("xhigh", "Extra high");
const MAX = effort("max", "Max");

function model(
  id: string,
  label: string,
  description: string,
  efforts: readonly AgentEffort[],
  defaultEffort: string | null,
): AgentModel {
  return { id, label, description, efforts, defaultEffort };
}

const ALL_AUTONOMY: readonly Autonomy[] = ["full_auto", "auto_edit", "ask"];

/** The agent dock, in display order: installed agents first, then the ones to install. */
export const AGENTS: readonly AgentInfo[] = [
  {
    id: "claude-code",
    name: "Claude Code",
    installed: true,
    version: "2.1.310",
    update: null,
    signedIn: true,
    models: [
      model(
        "claude-opus-5-5",
        "Opus 5.5",
        "Most capable, for long and complex work",
        [LOW, MEDIUM, HIGH, XHIGH, MAX],
        "high",
      ),
      model(
        "claude-sonnet-5-5",
        "Sonnet 5.5",
        "Fast and strong for everyday coding",
        [LOW, MEDIUM, HIGH, XHIGH, MAX],
        "medium",
      ),
      model(
        "claude-haiku-5-5",
        "Haiku 5.5",
        "Quickest, for small edits",
        [LOW, MEDIUM, HIGH],
        "low",
      ),
    ],
    defaultModel: "claude-opus-5-5",
    autonomy: ALL_AUTONOMY,
  },
  {
    id: "codex",
    name: "Codex",
    installed: true,
    version: "0.164.0",
    update: "0.165.0",
    signedIn: true,
    models: [
      model(
        "gpt-5.5-codex",
        "GPT-5.5 Codex",
        "Tuned for agentic coding",
        [LOW, MEDIUM, HIGH, XHIGH],
        "medium",
      ),
      model("gpt-5.5", "GPT-5.5", "General reasoning", [LOW, MEDIUM, HIGH, XHIGH], "medium"),
      model("gpt-5.5-mini", "GPT-5.5 mini", "Fast and light", [LOW, MEDIUM, HIGH], "low"),
    ],
    defaultModel: "gpt-5.5-codex",
    autonomy: ALL_AUTONOMY,
  },
  {
    id: "pi",
    name: "Pi",
    installed: true,
    version: "0.62.0",
    update: null,
    signedIn: true,
    models: [
      model(
        "claude-sonnet-5-5",
        "Sonnet 5.5",
        "Through your Anthropic key",
        [effort("off", "Off"), effort("minimal", "Minimal"), LOW, MEDIUM, HIGH],
        "medium",
      ),
      model(
        "gpt-5.5",
        "GPT-5.5",
        "Through your OpenAI key",
        [effort("minimal", "Minimal"), LOW, MEDIUM, HIGH],
        "medium",
      ),
    ],
    defaultModel: "claude-sonnet-5-5",
    // Pi never asks before acting.
    autonomy: ["full_auto"],
  },
  {
    id: "antigravity",
    name: "Antigravity",
    installed: true,
    version: "1.4.7",
    update: null,
    signedIn: true,
    models: [
      model("gemini-3.5-pro", "Gemini 3.5 Pro", "Deep reasoning", [LOW, HIGH], "high"),
      model("gemini-3.5-flash", "Gemini 3.5 Flash", "Fast", [], null),
    ],
    defaultModel: "gemini-3.5-pro",
    autonomy: ALL_AUTONOMY,
  },
  {
    id: "hermes",
    name: "Hermes",
    installed: true,
    version: "0.22.5",
    update: null,
    signedIn: true,
    models: [
      model("hermes-4.5-405b", "Hermes 4.5 405B", "Largest open model", [], null),
      model("hermes-4.5-70b", "Hermes 4.5 70B", "Faster, lighter", [], null),
    ],
    defaultModel: "hermes-4.5-405b",
    autonomy: ALL_AUTONOMY,
  },
  {
    id: "cursor",
    name: "Cursor",
    installed: false,
    version: null,
    update: null,
    signedIn: null,
    models: [],
    defaultModel: null,
    autonomy: ALL_AUTONOMY,
  },
  {
    id: "grok",
    name: "Grok Build",
    installed: false,
    version: null,
    update: null,
    signedIn: null,
    models: [],
    defaultModel: null,
    autonomy: ALL_AUTONOMY,
  },
];

function chat(
  id: string,
  projectId: string,
  title: string,
  agent: AgentBrandId,
  status: ChatSummary["status"],
  updated: string,
  changes?: ChatSummary["changes"],
): ChatSummary {
  return changes
    ? { id, projectId, title, agent, status, updated, changes }
    : { id, projectId, title, agent, status, updated };
}

export const PROJECTS: readonly Project[] = [
  {
    id: PROJECT_AURACODE,
    name: "auracode",
    color: "#9db7c9",
    expanded: true,
    chats: [
      // A running chat's time is how long it has been running.
      chat(
        CHAT_PACING,
        PROJECT_AURACODE,
        "Fix the flaky orb pacing test",
        "claude-code",
        "running",
        "3m",
      ),
      chat(CHAT_ACP, PROJECT_AURACODE, "Wire up the ACP client", "codex", "needs_approval", "2m"),
      chat(CHAT_HERO, PROJECT_AURACODE, "Landing page hero", "antigravity", "done_unseen", "14m", {
        added: 248,
        removed: 31,
      }),
      chat(CHAT_TRAY, PROJECT_AURACODE, "Tray icon states", "pi", "idle", "1h", {
        added: 36,
        removed: 9,
      }),
    ],
  },
  {
    id: PROJECT_SITE,
    name: "onghost.dev",
    color: "#8c7dff",
    expanded: false,
    chats: [
      chat(CHAT_DOWNLOAD, PROJECT_SITE, "Download page copy", "claude-code", "idle", "Tue"),
      chat(CHAT_SIGNIN, PROJECT_SITE, "Sign-in email template", "hermes", "error", "Mon"),
    ],
  },
  {
    id: PROJECT_DOTFILES,
    name: "dotfiles",
    color: "#f0a63a",
    expanded: false,
    chats: [chat(CHAT_ZSH, PROJECT_DOTFILES, "Speed up zsh startup", "codex", "idle", "Oct 12")],
  },
];

/** The composer for each chat: the agent, model, effort and autonomy its next message uses. */
export const COMPOSERS: Readonly<Record<string, Composer>> = {
  [CHAT_PACING]: {
    agent: "claude-code",
    model: "claude-opus-5-5",
    effort: "high",
    autonomy: "full_auto",
    draft: "",
    pickerOpen: false,
  },
  [CHAT_ACP]: {
    agent: "codex",
    model: "gpt-5.5-codex",
    effort: "medium",
    // Auto-edit: Codex edits files on its own but asks before running commands.
    autonomy: "auto_edit",
    draft: "",
    pickerOpen: false,
  },
};

/** A new chat starts on the default agent with Full auto. */
export const NEW_CHAT_COMPOSER: Composer = {
  agent: "claude-code",
  model: "claude-opus-5-5",
  effort: "high",
  autonomy: "full_auto",
  draft: "",
  pickerOpen: false,
};

export const BASE_WORKSPACE: Workspace = {
  projects: PROJECTS,
  agents: AGENTS,
  activeChatId: CHAT_PACING,
  activeProjectId: PROJECT_AURACODE,
  composer: COMPOSERS[CHAT_PACING] ?? NEW_CHAT_COMPOSER,
  account: { kind: "local", label: "Local" },
  rightPanel: null,
  discovery: null,
};

/** Every chat in sidebar order. */
export function allChats(workspace: Workspace): ChatSummary[] {
  return workspace.projects.flatMap((project) => project.chats);
}

export function findChat(workspace: Workspace, chatId: string | null): ChatSummary | null {
  if (chatId === null) return null;
  return allChats(workspace).find((candidate) => candidate.id === chatId) ?? null;
}

export function findProject(workspace: Workspace, projectId: string): Project | null {
  return workspace.projects.find((project) => project.id === projectId) ?? null;
}

export function findAgent(workspace: Workspace, agentId: string): AgentInfo | null {
  return workspace.agents.find((agent) => agent.id === agentId) ?? null;
}

export function findModel(agent: AgentInfo | null, modelId: string | null): AgentModel | null {
  if (!agent || modelId === null) return null;
  return agent.models.find((candidate) => candidate.id === modelId) ?? null;
}

/** "Opus 5.5" for "claude-opus-5-5"; the id itself when the workspace does not know the model. */
export function modelLabel(workspace: Workspace, agentId: string, modelId: string | null): string {
  if (modelId === null) return "";
  return findModel(findAgent(workspace, agentId), modelId)?.label ?? modelId;
}

/** "High" for "high"; the id itself when unknown, "" for none. */
export function effortLabel(
  workspace: Workspace,
  agentId: string,
  modelId: string | null,
  effortId: string | null,
): string {
  if (effortId === null) return "";
  const known = findModel(findAgent(workspace, agentId), modelId)?.efforts.find(
    (candidate) => candidate.id === effortId,
  );
  return known?.label ?? effortId;
}

/** The composer pill: "Claude Code · Opus 5.5 · High" (parts without a value are left out). */
export function composerLabel(
  workspace: Workspace,
  composer: Composer = workspace.composer,
): string {
  const agent = findAgent(workspace, composer.agent);
  return [
    agent?.name ?? composer.agent,
    modelLabel(workspace, composer.agent, composer.model),
    effortLabel(workspace, composer.agent, composer.model, composer.effort),
  ]
    .filter((part) => part !== "")
    .join(" · ");
}

export const AUTONOMY_LABELS: Readonly<Record<Autonomy, string>> = {
  full_auto: "Full auto",
  auto_edit: "Auto-edit",
  ask: "Ask",
};
