/** Browser fixture only. All values are synthetic and stay in this page's memory. */
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import type {
  MemoryNote,
  MemoryNoteSummary,
  ProjectMemory,
  ProjectMemoryChanged,
} from "../bindings";
import { Banners } from "../components/chat/Banners";
import { ProjectMemoryPanel } from "../components/settings/ProjectMemoryPanel";
import {
  agentEditText,
  agentEditToast,
  type MemoryChats,
  memoryBytes,
  type ProjectMemoryApi,
  useAgentMemoryEdits,
} from "../lib/projectMemory";
import { createSetupFixture } from "./setupFixture";

/** One saved revision, of the brief (`note` is `null`) or of a note. */
interface Saved {
  readonly revision: string;
  readonly project: string;
  readonly note: string | null;
  readonly description: string | null;
  readonly markdown: string;
  readonly deleted: boolean;
  readonly reason: string;
  readonly source: string;
  readonly at: number;
}

export interface ProjectMemoryFixture {
  readonly api: ProjectMemoryApi;
  /**
   * An agent's write of the brief through its chat's grant. It is saved while agents may edit
   * memory and refused otherwise; the answer says which. `silent` skips the change event, as a
   * missed event would.
   */
  agentWrite(
    markdown: string,
    options?: { readonly silent?: boolean; readonly chat?: string },
  ): boolean;
  /** An agent's save of one note, new or over the one that is there. */
  agentNote(
    name: string,
    markdown: string,
    options?: { readonly chat?: string; readonly description?: string },
  ): boolean;
  /** An agent's removal of one note. */
  agentDeleteNote(name: string, options?: { readonly chat?: string }): boolean;
  /** What was saved, newest first, with who saved it and why. */
  saves(): readonly Pick<Saved, "note" | "markdown" | "reason" | "source" | "deleted">[];
}

const FIRST = "fixture-project";
const START = 1793505600000;
const DAY = 86_400_000;

export function createProjectMemoryFixture(
  agentsEdit: () => boolean = () => true,
): ProjectMemoryFixture {
  // Newest first. The fixture follows the app's rules where a check can see them: a save names
  // the revision it read, and Undo goes back only while nothing newer was saved.
  const history: Saved[] = [
    {
      revision: "revision-2",
      project: FIRST,
      note: "windows-build",
      description: "Read before building the app on Windows",
      markdown: "Build with the helper script. Never run cargo on the warm target.\n",
      deleted: false,
      reason: "Learned while working",
      source: "agent:chat-fixture",
      at: Date.now() - 3 * DAY,
    },
    {
      revision: "revision-1",
      project: FIRST,
      note: null,
      description: null,
      markdown: "# Project notes\n\nUse pnpm for workspace commands.\n",
      deleted: false,
      reason: "Initial project notes",
      source: "user",
      at: START,
    },
  ];
  const projects = new Set([FIRST, "second-project"]);
  const listeners = new Set<(change: ProjectMemoryChanged) => void>();
  const notify = (change: ProjectMemoryChanged) => {
    for (const listener of [...listeners]) listener(change);
  };
  const by = (source: string) =>
    source.startsWith("agent:") ? "agent" : source === "bot" ? "bot" : "user";
  const of = (id: string) => {
    if (!projects.has(id)) throw new Error("That project is no longer here.");
    return history.filter((saved) => saved.project === id);
  };
  const latest = (id: string, note: string | null) => of(id).find((saved) => saved.note === note);
  const notes = (id: string): MemoryNoteSummary[] =>
    [...new Set(of(id).map((saved) => saved.note))]
      .filter((name): name is string => name !== null)
      .map((name) => latest(id, name))
      .filter((saved): saved is Saved => saved !== undefined && !saved.deleted)
      .map((saved) => ({
        name: saved.note ?? "",
        description: saved.description ?? "",
        revision: saved.revision,
        updated_at: saved.at,
        by: by(saved.source),
        bytes: memoryBytes(saved.markdown),
      }))
      .sort((a, b) => a.name.localeCompare(b.name));
  const memory = (id: string): ProjectMemory => {
    const brief = latest(id, null);
    return {
      project_id: id,
      revision: brief?.revision ?? null,
      markdown: brief?.markdown ?? "",
      updated_at: brief?.at ?? START,
      head: of(id)[0]?.revision ?? null,
      notes: notes(id),
    };
  };
  const note = (id: string, name: string): MemoryNote => {
    const saved = latest(id, name);
    if (!saved || saved.deleted)
      throw new Error(`There is no note ${name}. o7_memory_read lists the notes there are.`);
    return {
      project_id: id,
      name,
      description: saved.description ?? "",
      revision: saved.revision,
      markdown: saved.markdown,
      updated_at: saved.at,
      by: by(saved.source),
    };
  };
  let count = history.length;
  const commit = (entry: Omit<Saved, "revision" | "at">): Saved => {
    count += 1;
    const next = { ...entry, revision: `revision-${count}`, at: Date.now() };
    history.unshift(next);
    return next;
  };
  // `?case=stale`: the first save meets a version saved on another device a moment earlier.
  let raced = new URLSearchParams(location.search).get("case") === "stale";
  const api: ProjectMemoryApi = {
    read: async (id) => structuredClone(memory(id)),
    save: async (input) => {
      if (raced) {
        raced = false;
        commit({
          project: input.project_id,
          note: null,
          description: null,
          markdown: "Saved on another device.\n",
          deleted: false,
          reason: "Edited elsewhere",
          source: "user",
        });
      }
      if (input.expected_revision !== memory(input.project_id).revision)
        throw new Error("Project memory changed. Review the current revision before saving.");
      commit({
        project: input.project_id,
        note: null,
        description: null,
        markdown: input.markdown,
        deleted: false,
        reason: input.reason,
        source: "user",
      });
      notify({ project_id: input.project_id, agent_edit: null });
      return structuredClone(memory(input.project_id));
    },
    readNote: async (id, name) => structuredClone(note(id, name)),
    saveNote: async (input) => {
      const current = latest(input.project_id, input.name);
      const there = current && !current.deleted ? current : null;
      if (there === null && input.expected_revision !== null)
        throw new Error(
          `There is no note ${input.name}. o7_memory_read lists the notes there are.`,
        );
      if (there !== null && input.expected_revision !== there.revision)
        throw new Error(
          `The note ${input.name} changed since revision ${input.expected_revision}: it is now revision ${there.revision}. Read it again, apply your edit to the current text, and write with the new base revision.`,
        );
      commit({
        project: input.project_id,
        note: input.name,
        description: input.description,
        markdown: input.markdown,
        deleted: false,
        reason: input.reason,
        source: "user",
      });
      notify({ project_id: input.project_id, agent_edit: null });
      return structuredClone(note(input.project_id, input.name));
    },
    deleteNote: async (input) => {
      const there = note(input.project_id, input.name);
      if (input.expected_revision !== there.revision)
        throw new Error(`The note ${input.name} changed. Read it again before you delete it.`);
      commit({
        project: input.project_id,
        note: input.name,
        description: there.description,
        markdown: "",
        deleted: true,
        reason: input.reason,
        source: "user",
      });
      notify({ project_id: input.project_id, agent_edit: null });
      return structuredClone(memory(input.project_id));
    },
    undo: async (id, first, last) => {
      const all = of(id);
      const start = all.find((saved) => saved.revision === first);
      const end = all.find((saved) => saved.revision === last);
      const moved = new Error(
        "Memory changed after that edit, so it cannot be undone. Open Project memory to change it.",
      );
      if (!start || !end || start.note !== end.note) throw moved;
      if (latest(id, end.note)?.revision !== last) throw moved;
      const before = all.slice(all.indexOf(start) + 1).find((saved) => saved.note === start.note);
      const restored = before && !before.deleted ? before : null;
      commit({
        project: id,
        note: start.note,
        description: restored?.description ?? end.description,
        markdown: restored?.markdown ?? "",
        deleted: start.note !== null && restored === null,
        reason: "Undid an agent's edit",
        source: "user",
      });
      notify({ project_id: id, agent_edit: null });
      return structuredClone(memory(id));
    },
    subscribe: (changed) => {
      listeners.add(changed);
      return () => listeners.delete(changed);
    },
  };
  const agent = (
    chat: string,
    entry: Pick<Saved, "note" | "description" | "markdown" | "deleted">,
    silent = false,
  ) => {
    if (!agentsEdit()) return false;
    const parent = memory(FIRST).head ?? null;
    const next = commit({
      project: FIRST,
      ...entry,
      reason: "Learned while working",
      source: `agent:${chat}`,
    });
    if (!silent)
      notify({
        project_id: FIRST,
        agent_edit: {
          revision: next.revision,
          parent_revision: parent,
          chat_id: chat,
          note: entry.note,
          deleted: entry.deleted,
        },
      });
    return true;
  };
  return {
    api,
    saves: () =>
      history.map(({ note, markdown, reason, source, deleted }) => ({
        note,
        markdown,
        reason,
        source,
        deleted,
      })),
    agentWrite: (markdown, { silent = false, chat = "chat-fixture" } = {}) =>
      agent(chat, { note: null, description: null, markdown, deleted: false }, silent),
    agentNote: (name, markdown, { chat = "chat-fixture", description = `Read for ${name}` } = {}) =>
      agent(chat, { note: name, description, markdown, deleted: false }),
    agentDeleteNote: (name, { chat = "chat-fixture" } = {}) =>
      agent(chat, {
        note: name,
        description: latest(FIRST, name)?.description ?? null,
        markdown: "",
        deleted: true,
      }),
  };
}

const ONE = [{ id: FIRST, name: "o7 fixture" }];
const TWO = [...ONE, { id: "second-project", name: "website fixture" }];
const CHATS: MemoryChats = new Map([
  ["chat-fixture", { title: "Fix the login flow", agent: "Claude Code" }],
  ["chat-untitled", { title: null, agent: "Codex" }],
]);
const setup = createSetupFixture();
const fixture = createProjectMemoryFixture(() => setup.general().agent_memory_writes);

export default function ProjectMemoryPreview() {
  const edits = useAgentMemoryEdits(fixture.api);
  // Open shows the panel afresh, as Settings does in the app.
  const [opened, setOpened] = useState(0);
  const params = new URLSearchParams(location.search);
  const projects = params.get("projects") === "2" ? TWO : params.get("projects") === "0" ? [] : ONE;
  return (
    <main
      className="g-root settings-content"
      style={{ maxWidth: 960, margin: "0 auto", padding: 32 }}
    >
      <h1>Customize</h1>
      <Banners
        storage={null}
        memoryEdits={edits.notices.map((notice) => ({
          key: `${notice.projectId}:${notice.revision}`,
          ...agentEditToast(notice, CHATS, "o7 fixture"),
          text: agentEditText(notice, CHATS, "o7 fixture"),
          state: notice.state,
          onReview: () => {
            edits.dismiss(notice);
            setOpened(Date.now());
          },
          onUndo: () => void edits.undo(notice),
          onDismiss: () => edits.dismiss(notice),
        }))}
      />
      <ProjectMemoryPanel key={opened} projects={projects} api={fixture.api} setup={setup} />
    </main>
  );
}
export function mountProjectMemoryPreview(container: HTMLElement) {
  Object.assign(window, {
    projectMemoryFixture: {
      agentWrite: fixture.agentWrite,
      agentNote: fixture.agentNote,
      agentDeleteNote: fixture.agentDeleteNote,
      saves: fixture.saves,
    },
  });
  createRoot(container).render(
    <StrictMode>
      <ProjectMemoryPreview />
    </StrictMode>,
  );
}
