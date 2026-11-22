import { type ChatSummary, commands, type WorkspaceSnapshot } from "../bindings";
import { settle } from "./ipc";

/**
 * The sidebar's lists as this profile last showed them, read from two small files before the
 * first render (`launch_cache`, which never touches the database). The window's first frame shows
 * them, and the database's own answer replaces them a moment later.
 *
 * Rust hands back only what this build wrote for the open profile, so the shapes are this page's
 * own. They are still checked here, because a file on disk is not a promise: whatever does not
 * look right is dropped, and the sidebar starts empty as it always did.
 */
export interface LaunchLists {
  readonly chats: ChatSummary[] | null;
  readonly workspace: WorkspaceSnapshot | null;
}

export const NO_LAUNCH_LISTS: LaunchLists = { chats: null, workspace: null };

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

function isChat(value: unknown): value is ChatSummary {
  return (
    isRecord(value) &&
    typeof value.id === "string" &&
    (value.title === null || typeof value.title === "string") &&
    typeof value.agent_id === "string" &&
    typeof value.agent_name === "string" &&
    typeof value.folder === "string" &&
    typeof value.folder_state === "string" &&
    typeof value.status === "string" &&
    typeof value.created_at === "number" &&
    typeof value.updated_at === "number"
  );
}

/**
 * A chat as it stands at launch. What it was doing when the list was kept is over: no agent runs
 * before the app has started one, and nothing can be waiting on the person yet.
 */
function atRest(chat: ChatSummary): ChatSummary {
  return { ...chat, status: "stopped", needs_attention: false };
}

function parse(text: string | null): unknown {
  if (text === null) return null;
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return null;
  }
}

/** The chat list kept as `text`, or `null` when it is not one. */
export function launchChats(text: string | null): ChatSummary[] | null {
  const value = parse(text);
  if (!Array.isArray(value) || !value.every(isChat)) return null;
  return value.map(atRest);
}

/** The workspace kept as `text`, or `null` when it is not one. */
export function launchWorkspace(text: string | null): WorkspaceSnapshot | null {
  const value = parse(text);
  if (!isRecord(value) || !Array.isArray(value.projects) || !Array.isArray(value.chats)) {
    return null;
  }
  const projects = value.projects.every(
    (project) =>
      isRecord(project) &&
      typeof project.id === "string" &&
      typeof project.name === "string" &&
      (project.color === null || typeof project.color === "string") &&
      Array.isArray(project.folders),
  );
  const chats = value.chats.every(
    (entry) =>
      isRecord(entry) &&
      isChat(entry.chat) &&
      typeof entry.project_id === "string" &&
      (entry.folder_id === null || typeof entry.folder_id === "string") &&
      typeof entry.pinned === "boolean" &&
      typeof entry.archived === "boolean",
  );
  if (!projects || !chats) return null;
  const snapshot = value as unknown as WorkspaceSnapshot;
  return {
    ...snapshot,
    chats: snapshot.chats.map((entry) => ({ ...entry, chat: atRest(entry.chat) })),
  };
}

/** Reads what the profile kept. Never fails: a launch without it is an ordinary launch. */
export async function readLaunchLists(
  read: () => Promise<{ chats: string | null; workspace: string | null }> = () =>
    settle(commands.launchCache()),
): Promise<LaunchLists> {
  try {
    const kept = await read();
    return { chats: launchChats(kept.chats), workspace: launchWorkspace(kept.workspace) };
  } catch {
    return NO_LAUNCH_LISTS;
  }
}
