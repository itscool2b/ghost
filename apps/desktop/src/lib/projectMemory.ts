import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import {
  type ChatSummary,
  commands,
  type DeleteMemoryNote,
  type MemoryNote,
  type ProjectMemory,
  type ProjectMemoryChanged,
  type SaveMemoryNote,
  type SaveProjectMemory,
} from "../bindings";
import { errorMessage } from "./errors";
import { inTauri, settle } from "./ipc";

/**
 * A project's memory is a short brief, which every chat of the project receives, and notes, which
 * every chat sees listed and its agent reads when one fits the task. Rust owns every rule: who may
 * write, the limits, and that a save names the revision it read.
 */
export interface ProjectMemoryApi {
  /** The brief, with the list of notes. */
  read(projectId: string): Promise<ProjectMemory>;
  /** Saves the brief. */
  save(input: SaveProjectMemory): Promise<ProjectMemory>;
  /** One note with its text. */
  readNote(projectId: string, name: string): Promise<MemoryNote>;
  /** Saves one note whole: a new one when it names no revision, otherwise over the one it read. */
  saveNote(input: SaveMemoryNote): Promise<MemoryNote>;
  /** Removes a note; answers the memory as it then stands. */
  deleteNote(input: DeleteMemoryNote): Promise<ProjectMemory>;
  /**
   * Takes back a run of edits to the brief or to one note, `first` to `last`, as your own save.
   * Refused once the brief or the note has moved on from `last`.
   */
  undo(projectId: string, first: string, last: string): Promise<ProjectMemory>;
  /**
   * Calls `changed` after every committed memory change, the editor's or an agent's, until the
   * returned function is called. An agent's direct edit is named in `agent_edit`.
   */
  subscribe(changed: (change: ProjectMemoryChanged) => void): () => void;
}

/** The window event Rust sends after each committed memory change (memory_commands.rs). */
export const PROJECT_MEMORY_CHANGED = "auracode-project-memory-changed";
/** Read every open project again: changes were missed. */
export const MEMORY_CHANGED_ANYWHERE: ProjectMemoryChanged = { project_id: null, agent_edit: null };

/** Native memory changes, the editor's and agents'. */
export function subscribeProjectMemory(
  changed: (change: ProjectMemoryChanged) => void,
): () => void {
  if (!inTauri()) return () => {};
  let disposed = false;
  let stop: UnlistenFn | null = null;
  // Without native events, focus and Refresh still read memory again.
  void listen<ProjectMemoryChanged>(PROJECT_MEMORY_CHANGED, ({ payload }) => {
    if (!disposed) changed(payload);
  })
    .then((unlisten) => {
      if (disposed) unlisten();
      else stop = unlisten;
    })
    .catch(() => {});
  return () => {
    disposed = true;
    stop?.();
    stop = null;
  };
}

export const projectMemoryApi: ProjectMemoryApi = {
  read: (id) => settle(commands.readProjectMemory(id)),
  save: (input) => settle(commands.saveProjectMemory(input)),
  readNote: (id, name) => settle(commands.readMemoryNote(id, name)),
  saveNote: (input) => settle(commands.saveMemoryNote(input)),
  deleteNote: (input) => settle(commands.deleteMemoryNote(input)),
  undo: (id, first, last) => settle(commands.undoMemoryEdit(id, first, last)),
  subscribe: subscribeProjectMemory,
};
/** The most the brief holds: every chat of the project receives all of it. */
export const MAX_MEMORY_BYTES = 4 * 1024;
/** The most one note holds. */
export const MAX_NOTE_BYTES = 16 * 1024;
/** The most notes a project has. */
export const MAX_NOTES = 32;
/** The longest description of a note, in characters. */
export const MAX_NOTE_DESCRIPTION = 120;
/** What a note may be named: lowercase letters, digits and dashes, at most 40 characters. */
export const NOTE_NAME = /^[a-z0-9][a-z0-9-]{0,39}$/;
export function memoryBytes(markdown: string): number {
  return new TextEncoder().encode(markdown).length;
}

/** What memory shows of a chat: its title and its agent's name. */
export interface MemoryChat {
  readonly title: string | null;
  readonly agent: string;
}
export type MemoryChats = ReadonlyMap<string, MemoryChat>;
export const NO_CHATS: MemoryChats = new Map();
/** Later entries win, so live chat titles can follow the saved ones. */
export function memoryChats(
  chats: readonly Pick<ChatSummary, "id" | "title" | "agent_name">[],
): MemoryChats {
  return new Map(chats.map((chat) => [chat.id, { title: chat.title, agent: chat.agent_name }]));
}

/** A revision an agent saved directly names its chat (chatstore/memory.rs). */
export const AGENT_SOURCE = "agent:";
export function agentChat(source: string): string | null {
  return source.startsWith(AGENT_SOURCE) ? source.slice(AGENT_SOURCE.length) || null : null;
}

/**
 * Names the agent of a chat: "Claude Code in Fix the login flow", "Claude Code" for an untitled
 * chat, or "An agent" (`an agent` inside a sentence) when the chat is not known here.
 */
export function agentLabel(chatId: string, chats: MemoryChats, sentenceStart = true): string {
  // A linked bot's save names no chat.
  if (chatId === "") return sentenceStart ? "A linked assistant" : "a linked assistant";
  const chat = chats.get(chatId);
  if (!chat) return sentenceStart ? "An agent" : "an agent";
  const title = chat.title?.trim();
  return title ? `${chat.agent} in ${title}` : chat.agent;
}

/** A newer saved version than the one a draft started from. It is shown, never applied over it. */
export function newerVersion(
  base: ProjectMemory | null,
  saved: ProjectMemory | null,
  draft: string,
): ProjectMemory | null {
  if (!base || !saved || saved.project_id !== base.project_id) return null;
  return saved.revision !== base.revision && draft !== base.markdown ? saved : null;
}

/** Whether an editor without a draft takes a newer saved version at once. */
export function adoptsSaved(
  base: ProjectMemory | null,
  saved: ProjectMemory | null,
  draft: string,
): boolean {
  return (
    !!base &&
    !!saved &&
    saved.project_id === base.project_id &&
    saved.revision !== base.revision &&
    draft === base.markdown
  );
}

export type AgentEditState =
  | { readonly status: "shown" }
  | { readonly status: "undoing" }
  | { readonly status: "undone" }
  | { readonly status: "failed"; readonly message: string };

/** A run of edits to the brief (`note` is `null`) or to one note: what Undo takes back as one. */
export interface EditedSubject {
  readonly note: string | null;
  /** The earliest edit of the run and the latest. */
  readonly first: string;
  readonly last: string;
  /** The latest edit removed the note. */
  readonly deleted: boolean;
}

/** The window's notice of agents' direct edits to one project, with what Undo takes back. */
export interface AgentEditNotice {
  readonly projectId: string;
  /** The latest of these edits: another one folds in only when it follows this. */
  readonly revision: string;
  /** What these edits changed, in the order each was first touched. */
  readonly subjects: readonly EditedSubject[];
  /** The chats whose agents made them; an empty id is a linked bot. */
  readonly chats: readonly string[];
  readonly edits: number;
  readonly state: AgentEditState;
}
const MAX_NOTICES = 3;

/**
 * Adds an agent's direct edit. Edits that follow each other in one project fold into one notice,
 * so Undo returns to the revision before the first; anything in between starts a new notice.
 */
export function noticeAgentEdit(
  notices: readonly AgentEditNotice[],
  change: ProjectMemoryChanged,
): readonly AgentEditNotice[] {
  const edit = change.agent_edit;
  const projectId = change.project_id;
  if (!edit || !projectId) return notices;
  const current = notices.find((notice) => notice.projectId === projectId);
  if (current?.revision === edit.revision) return notices;
  const note = edit.note ?? null;
  const deleted = edit.deleted ?? false;
  const folds = current?.state.status === "shown" && current.revision === edit.parent_revision;
  const next: AgentEditNotice = folds
    ? {
        ...current,
        revision: edit.revision,
        edits: current.edits + 1,
        chats: current.chats.includes(edit.chat_id)
          ? current.chats
          : [...current.chats, edit.chat_id],
        // A second edit to the same thing lengthens its run; anything else starts one.
        subjects: current.subjects.some((subject) => subject.note === note)
          ? current.subjects.map((subject) =>
              subject.note === note ? { ...subject, last: edit.revision, deleted } : subject,
            )
          : [...current.subjects, { note, first: edit.revision, last: edit.revision, deleted }],
      }
    : {
        projectId,
        revision: edit.revision,
        subjects: [{ note, first: edit.revision, last: edit.revision, deleted }],
        chats: [edit.chat_id],
        edits: 1,
        state: { status: "shown" },
      };
  return [...notices.filter((notice) => notice.projectId !== projectId), next].slice(-MAX_NOTICES);
}

/** Who made a notice's edits, at the start of a sentence or inside one. */
export function agentEditAuthor(
  notice: AgentEditNotice,
  chats: MemoryChats,
  sentenceStart = true,
): string {
  const [only] = notice.chats;
  if (notice.chats.length === 1 && only !== undefined)
    return agentLabel(only, chats, sentenceStart);
  return `${sentenceStart ? "Agents" : "agents"} in ${notice.chats.length} chats`;
}

/**
 * A memory edit as its toast says it: a short title, then who and where on one line ("Claude
 * Code · Fix the login flow · Ghost"), so a long chat title shortens instead of swallowing the
 * sentence around it. `agentEditText` stays the full sentence, for the toast's tooltip.
 */
export function agentEditToast(
  notice: AgentEditNotice,
  chats: MemoryChats,
  project: string | null,
): { readonly title: string; readonly detail: string } {
  if (notice.state.status === "undone") {
    return {
      title: "Edit undone",
      detail: `Project memory${project ? ` for ${project}` : ""} is back to its revision before ${
        notice.edits > 1 ? "these edits" : "this edit"
      }.`,
    };
  }
  const [only] = notice.chats;
  const chat = notice.chats.length === 1 && only !== undefined ? chats.get(only) : undefined;
  const who =
    notice.chats.length > 1
      ? [`Agents in ${notice.chats.length} chats`]
      : chat
        ? [chat.agent, chat.title?.trim() ?? ""]
        : ["An agent"];
  const [subject] = notice.subjects;
  const one = notice.edits === 1 && subject !== undefined ? subject : null;
  return {
    title:
      one === null
        ? `${notice.edits} edits to project memory`
        : one.note === null
          ? "Project memory updated"
          : one.deleted
            ? "Memory note deleted"
            : "Memory note saved",
    // Which notes, between who and where.
    detail: [...who, editedNotes(notice), project ?? ""].filter((part) => part !== "").join(" · "),
  };
}

/** The notes a notice's edits touched, by name: "windows-build, release". */
export function editedNotes(notice: AgentEditNotice): string {
  return notice.subjects
    .map((subject) => subject.note)
    .filter((note): note is string => note !== null)
    .join(", ");
}

/** "Claude Code in Fix the login flow updated project memory for Ghost." */
export function agentEditText(
  notice: AgentEditNotice,
  chats: MemoryChats,
  project: string | null,
): string {
  const target = `project memory${project ? ` for ${project}` : ""}`;
  if (notice.state.status === "undone") {
    return `Undone. The ${target} is back to its revision before ${
      notice.edits > 1 ? "these edits" : "this edit"
    }.`;
  }
  const who = agentEditAuthor(notice, chats);
  if (notice.edits > 1) return `${who} made ${notice.edits} edits to ${target}.`;
  const [subject] = notice.subjects;
  if (subject === undefined || subject.note === null) return `${who} updated ${target}.`;
  return subject.deleted
    ? `${who} deleted the note ${subject.note} from ${target}.`
    : `${who} saved the note ${subject.note} in ${target}.`;
}

/**
 * Takes back what a notice's edits changed, as saves of your own: each run of edits to the brief
 * or to a note goes back to what was there before it, newest first. Rust refuses a run whose
 * brief or note has since moved on, so a later edit by anyone is never overwritten; what was
 * already taken back stays taken back, and the error says the rest was not.
 */
export async function undoAgentEdit(
  api: Pick<ProjectMemoryApi, "undo">,
  notice: AgentEditNotice,
): Promise<ProjectMemory> {
  let memory: ProjectMemory | null = null;
  let done = 0;
  for (const subject of [...notice.subjects].reverse()) {
    try {
      memory = await api.undo(notice.projectId, subject.first, subject.last);
      done += 1;
    } catch (failure) {
      if (done === 0) throw failure;
      throw new Error(`${done} of ${notice.subjects.length} was undone. ${errorMessage(failure)}`);
    }
  }
  if (memory === null) throw new Error("There was nothing to undo.");
  return memory;
}

/** The notices of agents' direct edits, from the memory events, with Undo and Dismiss. */
export function useAgentMemoryEdits(api: ProjectMemoryApi) {
  const [notices, setNotices] = useState<readonly AgentEditNotice[]>([]);
  useEffect(
    () => api.subscribe((change) => setNotices((current) => noticeAgentEdit(current, change))),
    [api],
  );
  const update = (notice: AgentEditNotice, state: AgentEditState) =>
    setNotices((current) =>
      current.map((entry) =>
        entry.projectId === notice.projectId && entry.revision === notice.revision
          ? { ...entry, state }
          : entry,
      ),
    );
  return {
    notices,
    dismiss: (notice: AgentEditNotice) =>
      setNotices((current) =>
        current.filter(
          (entry) => entry.projectId !== notice.projectId || entry.revision !== notice.revision,
        ),
      ),
    undo: async (notice: AgentEditNotice) => {
      update(notice, { status: "undoing" });
      try {
        await undoAgentEdit(api, notice);
        update(notice, { status: "undone" });
      } catch (failure) {
        update(notice, {
          status: "failed",
          message: `Nothing was undone. ${errorMessage(failure)}`,
        });
      }
    },
  };
}
