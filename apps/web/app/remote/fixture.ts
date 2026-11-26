/**
 * A stand-in for a paired computer, so the remote page can be looked at in development without
 * one: `/remote?fixture` on the development server, with `&theme=` (one of the desktop's themes)
 * and `&appearance=light|dark`. Nothing connects anywhere, nothing is stored, and every action
 * that would reach a computer is refused. The page loads this module in development only
 * (pages/remote.tsx), so no build ships it.
 */
import type {
  ChatSummary,
  ProjectSummary,
  SessionStatus,
  WorkspaceChat,
} from "../../../desktop/src/bindings";
import type { RemoteClient, State } from "./client";

const now = Date.now();

const chat = (
  id: string,
  title: string,
  agent: readonly [string, string],
  status: SessionStatus,
  minutes: number,
  needs = false,
): WorkspaceChat => {
  const summary: ChatSummary = {
    id,
    title,
    agent_id: agent[0],
    agent_host: { type: "native" },
    agent_name: agent[1],
    folder: "/Users/me/code/orbit",
    folder_state: "ok",
    status,
    needs_attention: needs,
    created_at: now - 9_000_000,
    updated_at: now - minutes * 60_000,
  };
  return {
    chat: summary,
    project_id: id.startsWith("docs") ? "docs" : "orbit",
    folder_id: null,
    pinned: false,
    archived: false,
  };
};

const project = (id: string, name: string): ProjectSummary => ({
  id,
  name,
  color: null,
  default_agent: null,
  default_model: null,
  folders: [],
  created_at: 0,
  updated_at: 0,
});

const CLAUDE = ["claude", "Claude Code"] as const;
const CODEX = ["codex", "Codex"] as const;
const PI = ["pi", "Pi"] as const;

export function fixtureClient(search: URLSearchParams): RemoteClient {
  const appearance = search.get("appearance");
  const state: State = {
    phase: { name: "ready" },
    hosts: [],
    host: { id: "fixture", name: "Studio" },
    overview: {
      host_name: "Studio",
      projects: [project("orbit", "orbit"), project("docs", "orbit-docs")],
      chats: [
        chat("a", "Ship the frame pacing fix", CLAUDE, "ready", 2, true),
        chat("b", "Tighten the hero spacing", CODEX, "running", 4),
        chat("c", "Update the changelog", PI, "ready", 180),
        chat("d", "Find what blocks a frame", CLAUDE, "failed", 1500),
        chat("docs-a", "Rewrite the queue runbook", CODEX, "stopped", 4000),
      ],
      agents: [
        { id: "claude", name: "Claude Code", available: true },
        { id: "codex", name: "Codex", available: true },
        { id: "pi", name: "Pi", available: false },
      ],
      default_agent: "claude",
    },
    opening: null,
    chat: null,
    features: [],
    look: {
      theme: search.get("theme") ?? "aura",
      appearance: appearance === "light" || appearance === "dark" ? appearance : "system",
    },
  };
  const refuse = async (): Promise<never> => {
    throw new Error("This is a fixture: no computer is connected.");
  };
  const nothing = () => {};
  const stub = {
    getState: () => state,
    subscribe: () => nothing,
    start: async () => {},
    pair: async () => {},
    stop: nothing,
    wake: nothing,
    use: nothing,
    forget: refuse,
    discard: refuse,
    open: refuse,
    close: nothing,
    create: refuse,
    send: refuse,
    options: refuse,
    interrupt: refuse,
    permission: refuse,
    answer: refuse,
    loadEarlier: refuse,
  };
  // The page only calls the methods above; the client's sealed channel is never built.
  return stub as unknown as RemoteClient;
}
