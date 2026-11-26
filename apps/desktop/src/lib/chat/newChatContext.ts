import type {
  AgentEntry,
  FolderMatch,
  FolderMatches,
  ManagedWorktree,
  ProjectFolder,
  ProjectSummary,
  WorkspaceSnapshot,
} from "../../bindings";
import { sameFolder } from "../gitWorkspace";
import { preselectAgent } from "../setup";

export interface NewChatContext {
  readonly projectId: string;
  readonly projectName: string;
  readonly folderId: string | null;
  readonly defaultAgent: string | null;
}

type Retained = readonly Pick<ManagedWorktree, "path" | "source">[];

/** Where a worktree Ghost made came from: Rust says so on the folder; a retained record also
 * does, for a worktree the registry still lists. */
function worktreeSource(folder: ProjectFolder, retained: Retained): string | null {
  if (folder.worktree_of) return folder.worktree_of;
  const path = folder.path;
  return path ? (retained.find((record) => sameFolder(record.path, path))?.source ?? null) : null;
}

/** A folder a new chat may start in by itself: on this device, available, and not a worktree. */
function startable(folder: ProjectFolder, retained: Retained): boolean {
  return folder.path !== null && folder.folder_state === "ok" && !worktreeSource(folder, retained);
}

/**
 * The folder of this project a chat's folder stands for: itself, or for a worktree, the folder
 * it was made from, within the same project. A worktree is never the answer.
 */
function standingFor(
  project: ProjectSummary,
  folderId: string | null,
  retained: Retained,
): string | null {
  const folder = project.folders.find((item) => item.id === folderId);
  if (!folder) return null;
  if (startable(folder, retained)) return folder.id;
  const source = worktreeSource(folder, retained);
  if (!source) return null;
  return (
    project.folders.find(
      (item) => startable(item, retained) && item.path !== null && sameFolder(item.path, source),
    )?.id ?? null
  );
}

/**
 * Resolve only this project's usable mappings; never borrow a folder from another project, and
 * never prefill a worktree: a recent chat in one prefills the folder it was made from.
 */
export function projectChatContext(
  project: ProjectSummary,
  workspace: WorkspaceSnapshot,
): NewChatContext {
  const usable = project.folders.filter((folder) => startable(folder, []));
  const recent = workspace.chats
    .filter((entry) => entry.project_id === project.id && !entry.archived)
    .toSorted((a, b) => b.chat.updated_at - a.chat.updated_at)
    .map((entry) => standingFor(project, entry.folder_id, []))
    .find((id) => id !== null);
  return {
    projectId: project.id,
    projectName: project.name,
    folderId: usable.length === 1 ? (usable[0]?.id ?? null) : (recent ?? null),
    defaultAgent: project.default_agent,
  };
}

/**
 * The automatic prefill as the workspace stands: the context's folder, or the source of the
 * worktree it names, else the sole folder a chat may start in. Retained records add what the
 * registry knows; nothing waits for them, since Rust marks worktree folders itself.
 */
export function projectSourceFolder(
  context: NewChatContext | null,
  workspace: WorkspaceSnapshot | null,
  retained: Retained,
): string | null {
  const project = workspace?.projects.find((project) => project.id === context?.projectId);
  if (!context || !project) return null;
  const eligible = project.folders.filter((folder) => startable(folder, retained));
  const resolved = standingFor(project, context.folderId, retained);
  if (resolved !== null && eligible.some((folder) => folder.id === resolved)) return resolved;
  return eligible.length === 1 ? (eligible[0]?.id ?? null) : null;
}

/**
 * The project folder a picked folder is: the draft's project's when it has one, else the one whose
 * chat was active last (Rust orders them). None for a managed worktree: a chat starts from its
 * source instead.
 */
export function chosenMatch(
  found: FolderMatches | null,
  projectId: string | null,
): FolderMatch | null {
  if (!found || found.worktree) return null;
  return found.matches.find((match) => match.project_id === projectId) ?? found.matches[0] ?? null;
}

/** What the chats screen shows: a chat, or New chat with its draft. */
export interface ChatsView<Draft> {
  readonly selected: string | null;
  readonly composing: boolean;
  readonly draft: Draft;
}

/**
 * Whether a chat New chat finished creating opens: only while the draft that made it still shows.
 * Creating takes a while (a worktree, its setup), and the person may have moved on meanwhile.
 */
export function createdChatOpens<Draft>(view: ChatsView<Draft>, creator: Draft): boolean {
  return view.selected === null && view.composing && view.draft === creator;
}

/** Project preference leads; an unavailable default falls back to the profile preference. */
export function newChatAgent(
  context: NewChatContext | null,
  profileDefault: string | null,
  agents: readonly AgentEntry[] | null,
  chosen: string | null,
): string | "wait" | null {
  if (chosen !== null) return null;
  const preferred = preselectAgent(context?.defaultAgent ?? null, agents, chosen);
  return preferred ?? preselectAgent(profileDefault, agents, chosen);
}
