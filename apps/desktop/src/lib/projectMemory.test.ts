import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProjectMemory, ProjectMemoryChanged } from "../bindings";

const native = vi.hoisted(() => ({ available: true, listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: native.listen }));
vi.mock("./ipc", () => ({ inTauri: () => native.available, settle: vi.fn() }));

import {
  type AgentEditNotice,
  adoptsSaved,
  agentEditText,
  agentEditToast,
  agentLabel,
  editedNotes,
  MAX_MEMORY_BYTES,
  MAX_NOTE_BYTES,
  MEMORY_CHANGED_ANYWHERE,
  memoryBytes,
  memoryChats,
  NOTE_NAME,
  newerVersion,
  noticeAgentEdit,
  PROJECT_MEMORY_CHANGED,
  subscribeProjectMemory,
  undoAgentEdit,
} from "./projectMemory";

const memory: ProjectMemory = {
  project_id: "one",
  revision: "r2",
  markdown: "Current",
  updated_at: 1,
};
const chats = memoryChats([
  { id: "login", title: "Fix the login flow", agent_name: "Claude Code" },
  { id: "untitled", title: null, agent_name: "Codex" },
]);
const edit = (
  id: string,
  parent: string | null,
  chat = "login",
  note: string | null = null,
  deleted = false,
): ProjectMemoryChanged => ({
  project_id: "one",
  agent_edit: { revision: id, parent_revision: parent, chat_id: chat, note, deleted },
});
/** One run of edits to the brief, or to a note. */
const run = (first: string, last = first, note: string | null = null, deleted = false) => ({
  note,
  first,
  last,
  deleted,
});

describe("project memory", () => {
  it("counts UTF-8 bytes without silently truncating Markdown", () => {
    expect(memoryBytes("é")).toBe(2);
    expect(memoryBytes("é".repeat(MAX_MEMORY_BYTES / 2))).toBe(MAX_MEMORY_BYTES);
    expect(memoryBytes("é".repeat(MAX_MEMORY_BYTES / 2 + 1))).toBeGreaterThan(MAX_MEMORY_BYTES);
  });
  it("keeps the brief short and gives a note more room, as Rust does", () => {
    expect(MAX_MEMORY_BYTES).toBe(4096);
    expect(MAX_NOTE_BYTES).toBe(16384);
  });
  it("names a note in lowercase letters, digits and dashes", () => {
    for (const name of ["windows-build", "a", "2026-plan", "n".repeat(40)])
      expect(NOTE_NAME.test(name), name).toBe(true);
    for (const name of ["", "Windows", "-lead", "has space", "under_score", "../x", "n".repeat(41)])
      expect(NOTE_NAME.test(name), name).toBe(false);
  });
});

describe("agent names", () => {
  it("lets live chat titles follow saved ones", () => {
    const named = memoryChats([
      { id: "login", title: null, agent_name: "Claude Code" },
      { id: "login", title: "Renamed chat", agent_name: "Claude Code" },
    ]);
    expect(agentLabel("login", named)).toBe("Claude Code in Renamed chat");
  });
});

describe("live refresh", () => {
  const newer: ProjectMemory = { ...memory, revision: "r3", markdown: "Agent edit" };
  it("takes a newer version into an editor without a draft", () => {
    expect(adoptsSaved(memory, newer, memory.markdown)).toBe(true);
    expect(newerVersion(memory, newer, memory.markdown)).toBeNull();
  });
  it("never applies a newer version over a draft, and offers it instead", () => {
    expect(adoptsSaved(memory, newer, "My draft")).toBe(false);
    expect(newerVersion(memory, newer, "My draft")).toEqual(newer);
  });
  it("ignores the same revision and another project's memory", () => {
    expect(newerVersion(memory, memory, "My draft")).toBeNull();
    expect(adoptsSaved(memory, memory, memory.markdown)).toBe(false);
    const other = { ...newer, project_id: "two" };
    expect(newerVersion(memory, other, "My draft")).toBeNull();
    expect(adoptsSaved(memory, other, memory.markdown)).toBe(false);
    expect(newerVersion(null, newer, "")).toBeNull();
  });
});

describe("agent edit notices", () => {
  it("shows only agents' new revisions", () => {
    expect(noticeAgentEdit([], MEMORY_CHANGED_ANYWHERE)).toEqual([]);
    expect(noticeAgentEdit([], { project_id: "one", agent_edit: null })).toEqual([]);
    const [notice] = noticeAgentEdit([], edit("r2", "r1"));
    expect(notice).toEqual({
      projectId: "one",
      revision: "r2",
      subjects: [run("r2")],
      chats: ["login"],
      edits: 1,
      state: { status: "shown" },
    });
  });
  it("folds edits that follow each other, so Undo returns to before the first", () => {
    let notices = noticeAgentEdit([], edit("r2", "r1"));
    notices = noticeAgentEdit(notices, edit("r3", "r2"));
    notices = noticeAgentEdit(notices, edit("r4", "r3", "untitled"));
    expect(notices).toEqual([
      {
        projectId: "one",
        revision: "r4",
        subjects: [run("r2", "r4")],
        chats: ["login", "untitled"],
        edits: 3,
        state: { status: "shown" },
      },
    ]);
    // The same event twice changes nothing.
    expect(noticeAgentEdit(notices, edit("r4", "r3"))).toBe(notices);
  });
  it("starts again after anything in between, and after an undo", () => {
    const first = noticeAgentEdit([], edit("r2", "r1"));
    // The person saved r3 in between: Undo must never discard it.
    expect(noticeAgentEdit(first, edit("r4", "r3"))).toEqual([
      expect.objectContaining({ revision: "r4", subjects: [run("r4")], edits: 1 }),
    ]);
    const undone = first.map((notice) => ({ ...notice, state: { status: "undone" } as const }));
    expect(noticeAgentEdit(undone, edit("r3", "r2"))).toEqual([
      expect.objectContaining({ revision: "r3", subjects: [run("r3")], edits: 1 }),
    ]);
  });
  it("keeps what each edit touched apart: the brief, and each note by name", () => {
    let notices = noticeAgentEdit([], edit("r2", "r1", "login", "windows-build"));
    notices = noticeAgentEdit(notices, edit("r3", "r2"));
    notices = noticeAgentEdit(notices, edit("r4", "r3", "login", "windows-build"));
    notices = noticeAgentEdit(notices, edit("r5", "r4", "untitled", "release", true));
    expect(notices).toEqual([
      {
        projectId: "one",
        revision: "r5",
        // A second edit to a note lengthens its run; the brief and another note have their own.
        subjects: [run("r2", "r4", "windows-build"), run("r3"), run("r5", "r5", "release", true)],
        chats: ["login", "untitled"],
        edits: 4,
        state: { status: "shown" },
      },
    ]);
    const [notice] = notices;
    if (!notice) throw new Error("notice");
    expect(editedNotes(notice)).toBe("windows-build, release");
    expect(agentEditToast(notice, chats, "o7")).toEqual({
      title: "4 edits to project memory",
      detail: "Agents in 2 chats · windows-build, release · o7",
    });
  });
  it("says which note an agent saved or deleted, and names a linked assistant", () => {
    const [saved] = noticeAgentEdit([], edit("r2", "r1", "login", "windows-build"));
    const [deleted] = noticeAgentEdit([], edit("r2", "r1", "login", "release", true));
    const [bot] = noticeAgentEdit([], edit("r2", "r1", ""));
    if (!saved || !deleted || !bot) throw new Error("notices");
    expect(agentEditToast(saved, chats, "o7")).toEqual({
      title: "Memory note saved",
      detail: "Claude Code · Fix the login flow · windows-build · o7",
    });
    expect(agentEditText(saved, chats, "o7")).toBe(
      "Claude Code in Fix the login flow saved the note windows-build in project memory for o7.",
    );
    expect(agentEditToast(deleted, chats, null).title).toBe("Memory note deleted");
    expect(agentEditText(deleted, chats, null)).toBe(
      "Claude Code in Fix the login flow deleted the note release from project memory.",
    );
    expect(agentLabel("", chats)).toBe("A linked assistant");
    expect(agentEditText(bot, chats, "o7")).toBe(
      "A linked assistant updated project memory for o7.",
    );
  });
  it("keeps the newest three projects", () => {
    let notices: readonly AgentEditNotice[] = [];
    for (const project of ["a", "b", "c", "d"])
      notices = noticeAgentEdit(notices, { ...edit(`${project}2`, null), project_id: project });
    expect(notices.map((notice) => notice.projectId)).toEqual(["b", "c", "d"]);
  });
  it("says who edited, how often, and what Undo did", () => {
    const [one] = noticeAgentEdit([], edit("r2", "r1"));
    if (!one) throw new Error("notice");
    expect(agentEditText(one, chats, "auracode")).toBe(
      "Claude Code in Fix the login flow updated project memory for auracode.",
    );
    expect(agentEditText({ ...one, chats: ["gone"] }, chats, null)).toBe(
      "An agent updated project memory.",
    );
    expect(agentEditText({ ...one, edits: 2, chats: ["login", "untitled"] }, chats, "x")).toBe(
      "Agents in 2 chats made 2 edits to project memory for x.",
    );
    expect(agentEditText({ ...one, state: { status: "undone" } }, chats, "auracode")).toBe(
      "Undone. The project memory for auracode is back to its revision before this edit.",
    );
  });
  it("puts the edit on a toast: a short title, then who and where on one line", () => {
    const [one] = noticeAgentEdit([], edit("r2", "r1"));
    if (!one) throw new Error("notice");
    expect(agentEditToast(one, chats, "auracode")).toEqual({
      title: "Project memory updated",
      detail: "Claude Code · Fix the login flow · auracode",
    });
    expect(agentEditToast({ ...one, chats: ["gone"] }, chats, null)).toEqual({
      title: "Project memory updated",
      detail: "An agent",
    });
    expect(agentEditToast({ ...one, edits: 2, chats: ["login", "untitled"] }, chats, "x")).toEqual({
      title: "2 edits to project memory",
      detail: "Agents in 2 chats · x",
    });
    expect(agentEditToast({ ...one, state: { status: "undone" } }, chats, "auracode")).toEqual({
      title: "Edit undone",
      detail: "Project memory for auracode is back to its revision before this edit.",
    });
  });
});

describe("undoing an agent's edit", () => {
  const notice: AgentEditNotice = {
    projectId: "one",
    revision: "r5",
    subjects: [run("r2", "r4", "windows-build"), run("r3"), run("r5", "r5", "release", true)],
    chats: ["login"],
    edits: 4,
    state: { status: "shown" },
  };
  it("takes back each run through Rust, newest first, and saves nothing itself", async () => {
    const undo = vi.fn(async () => memory);
    expect(await undoAgentEdit({ undo }, notice)).toBe(memory);
    expect(undo.mock.calls).toEqual([
      ["one", "r5", "r5"],
      ["one", "r3", "r3"],
      ["one", "r2", "r4"],
    ]);
  });
  it("passes Rust's refusal on when nothing could be undone", async () => {
    const undo = vi.fn(async () => {
      throw new Error("Memory changed after that edit, so it cannot be undone.");
    });
    await expect(undoAgentEdit({ undo }, notice)).rejects.toThrow("Memory changed after that edit");
    expect(undo).toHaveBeenCalledTimes(1);
  });
  it("says how far it got when a later run could not be undone", async () => {
    let calls = 0;
    const undo = vi.fn(async () => {
      calls += 1;
      if (calls === 2) throw new Error("Memory changed after that edit, so it cannot be undone.");
      return memory;
    });
    await expect(undoAgentEdit({ undo }, notice)).rejects.toThrow(
      "1 of 3 was undone. Memory changed after that edit",
    );
    expect(undo).toHaveBeenCalledTimes(2);
  });
});

describe("memory change events", () => {
  beforeEach(() => {
    native.available = true;
    native.listen.mockReset();
  });
  it("forwards memory changes, missed ones included, and stops on disposal", async () => {
    const handlers = new Map<string, (event: { payload: unknown }) => void>();
    const unlisten = vi.fn();
    native.listen.mockImplementation(async (name: string, handler) => {
      handlers.set(name, handler);
      return unlisten;
    });
    const changed = vi.fn();
    const stop = subscribeProjectMemory(changed);
    await Promise.resolve();
    await Promise.resolve();
    expect([...handlers.keys()]).toEqual([PROJECT_MEMORY_CHANGED]);
    const change = edit("r2", "r1");
    handlers.get(PROJECT_MEMORY_CHANGED)?.({ payload: change });
    // What Rust sends when its own listener fell behind: every open project reads again.
    handlers.get(PROJECT_MEMORY_CHANGED)?.({ payload: MEMORY_CHANGED_ANYWHERE });
    expect(changed.mock.calls).toEqual([[change], [MEMORY_CHANGED_ANYWHERE]]);
    stop();
    handlers.get(PROJECT_MEMORY_CHANGED)?.({ payload: change });
    expect(changed).toHaveBeenCalledTimes(2);
    expect(unlisten).toHaveBeenCalledOnce();
  });
  it("releases a listener that registers after the page left, and needs the native app", async () => {
    let finish: (stop: () => void) => void = () => {};
    native.listen.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    );
    const stop = subscribeProjectMemory(vi.fn());
    stop();
    const late = vi.fn();
    finish(late);
    await Promise.resolve();
    await Promise.resolve();
    expect(late).toHaveBeenCalledOnce();
    native.available = false;
    native.listen.mockReset();
    subscribeProjectMemory(vi.fn())();
    expect(native.listen).not.toHaveBeenCalled();
  });
});
