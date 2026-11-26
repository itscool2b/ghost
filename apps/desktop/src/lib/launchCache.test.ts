import { describe, expect, it } from "vitest";
import type { WorkspaceSnapshot } from "../bindings";
import { chatSummary } from "./chat/testing";
import { launchChats, launchWorkspace, NO_LAUNCH_LISTS, readLaunchLists } from "./launchCache";

const project = { id: "p1", name: "Ghost", color: null, folders: [], created_at: 1, updated_at: 1 };
const entry = (id: string) => ({
  chat: chatSummary(id, { status: "running", needs_attention: true }),
  project_id: "p1",
  folder_id: null,
  pinned: false,
  archived: false,
});

describe("the sidebar kept from the last launch", () => {
  it("reads a chat list back at rest: nothing runs and nothing waits before the app started it", () => {
    const kept = JSON.stringify([
      chatSummary("a", { title: "Fix login", status: "running", needs_attention: true }),
      chatSummary("b", { status: "failed" }),
    ]);
    const chats = launchChats(kept);
    expect(chats?.map((chat) => [chat.id, chat.title, chat.status, chat.needs_attention])).toEqual([
      ["a", "Fix login", "stopped", false],
      ["b", null, "stopped", false],
    ]);
  });

  it("drops whatever is not a chat list instead of rendering it", () => {
    for (const text of [
      null,
      "",
      "{",
      "null",
      '{"id":"a"}',
      '[{"id":"a"}]',
      JSON.stringify([{ ...chatSummary("a"), title: 7 }]),
      JSON.stringify([chatSummary("a"), "b"]),
      JSON.stringify([{ ...chatSummary("a"), updated_at: "now" }]),
    ]) {
      expect(launchChats(text), String(text)).toBeNull();
    }
    expect(launchChats("[]")).toEqual([]);
  });

  it("reads a workspace back with its chats at rest", () => {
    const kept = JSON.stringify({ projects: [project], chats: [entry("a")] });
    const workspace = launchWorkspace(kept);
    expect(workspace?.projects.map((item) => item.name)).toEqual(["Ghost"]);
    expect(workspace?.chats.map((item) => [item.chat.id, item.chat.status])).toEqual([
      ["a", "stopped"],
    ]);
    expect(workspace?.chats[0]?.chat.needs_attention).toBe(false);
  });

  it("drops whatever is not a workspace", () => {
    const whole = { projects: [project], chats: [entry("a")] } as unknown as WorkspaceSnapshot;
    for (const value of [
      null,
      [],
      { projects: [project] },
      { ...whole, projects: [{ ...project, folders: null }] },
      { ...whole, projects: [{ ...project, name: 1 }] },
      { ...whole, chats: [{ ...entry("a"), project_id: null }] },
      { ...whole, chats: [{ ...entry("a"), chat: { id: "a" } }] },
      { ...whole, chats: [{ ...entry("a"), pinned: "yes" }] },
    ]) {
      expect(launchWorkspace(JSON.stringify(value)), JSON.stringify(value)).toBeNull();
    }
    expect(launchWorkspace("not json")).toBeNull();
  });

  it("never fails a launch: an unreadable or refused cache is an ordinary first frame", async () => {
    expect(await readLaunchLists(() => Promise.reject(new Error("locked")))).toBe(NO_LAUNCH_LISTS);
    expect(await readLaunchLists(async () => ({ chats: "[", workspace: null }))).toEqual({
      chats: null,
      workspace: null,
    });
    const lists = await readLaunchLists(async () => ({
      chats: JSON.stringify([chatSummary("a")]),
      workspace: JSON.stringify({ projects: [project], chats: [] }),
    }));
    expect(lists.chats?.map((chat) => chat.id)).toEqual(["a"]);
    expect(lists.workspace?.projects).toHaveLength(1);
  });
});
