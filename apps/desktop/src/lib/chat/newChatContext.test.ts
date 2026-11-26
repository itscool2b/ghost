import { describe, expect, it } from "vitest";
import type { AgentEntry, ProjectFolder, ProjectSummary, WorkspaceSnapshot } from "../../bindings";
import {
  chosenMatch,
  newChatAgent,
  projectChatContext,
  projectSourceFolder,
} from "./newChatContext";
import { chatSummary } from "./testing";

const folder = (id: string, ok = true): ProjectFolder => ({
  id,
  project_id: "project",
  display_name: id,
  path: ok ? `/code/${id}` : null,
  folder_state: ok ? "ok" : "missing",
  unavailable_reason: null,
  kind: "local",
  git_remote_url: null,
  git_default_branch: null,
});
const project = (folders: ProjectFolder[]): ProjectSummary => ({
  id: "project",
  name: "Product",
  color: null,
  default_agent: "claude",
  default_model: null,
  folders,
  created_at: 0,
  updated_at: 0,
});
const workspace = (value: ProjectSummary): WorkspaceSnapshot => ({
  projects: [value],
  chats: [
    {
      chat: { ...chatSummary("recent"), updated_at: 10 },
      project_id: "project",
      folder_id: "second",
      pinned: false,
      archived: false,
    },
    {
      chat: { ...chatSummary("foreign"), updated_at: 20 },
      project_id: "elsewhere",
      folder_id: "first",
      pinned: false,
      archived: false,
    },
  ],
});
const agent = (id: string, checking = false): AgentEntry => ({
  id,
  name: id,
  description: "",
  launch: id,
  needs_download: false,
  last_verified: "",
  installation: null,
  latest_release: null,
  availability: checking ? { state: "checking" } : { state: "ready", path: id, version: null },
});

describe("project new-chat context", () => {
  it("prefills the sole usable folder and retains the project preference", () => {
    const value = project([folder("first"), folder("second", false)]);
    expect(projectChatContext(value, workspace(value))).toEqual({
      projectId: "project",
      projectName: "Product",
      folderId: "first",
      defaultAgent: "claude",
    });
  });
  it("uses the last valid folder within this project only", () => {
    const value = project([folder("first"), folder("second")]);
    expect(projectChatContext(value, workspace(value)).folderId).toBe("second");
    expect(projectChatContext(value, { projects: [value], chats: [] }).folderId).toBeNull();
  });
  it("leaves unavailable projects unresolved", () => {
    const value = project([folder("second", false)]);
    expect(projectChatContext(value, workspace(value)).folderId).toBeNull();
  });
  it("resolves a recent managed worktree to its source within the same project", () => {
    const value = project([folder("first"), folder("second"), folder("managed")]);
    const context = { ...projectChatContext(value, workspace(value)), folderId: "managed" };
    const retained = [{ path: "/code/managed", source: "/code/first" }];
    expect(projectSourceFolder(context, workspace(value), retained)).toBe("first");
    expect(projectSourceFolder(context, workspace(value), [])).toBe("managed");
  });
  it("falls back only to a sole usable source after excluding retained paths", () => {
    const value = project([folder("first"), folder("second", false), folder("managed")]);
    const context = { ...projectChatContext(value, workspace(value)), folderId: "managed" };
    const retained = [{ path: "/code/managed", source: "/unmapped/source" }];
    expect(projectSourceFolder(context, workspace(value), retained)).toBe("first");
    const multiple = project([folder("first"), folder("second"), folder("managed")]);
    expect(projectSourceFolder(context, workspace(multiple), retained)).toBeNull();
  });
  it("does not resolve a retained source through a foreign project or an unavailable folder", () => {
    const value = project([folder("managed"), folder("first", false)]);
    const snapshot = {
      ...workspace(value),
      projects: [value, { ...project([folder("foreign")]), id: "elsewhere" }],
    };
    const context = { ...projectChatContext(value, snapshot), folderId: "managed" };
    expect(
      projectSourceFolder(context, snapshot, [{ path: "/code/managed", source: "/code/foreign" }]),
    ).toBeNull();
    expect(projectSourceFolder(null, snapshot, [])).toBeNull();
  });
  it("never prefills a worktree: a recent chat in one prefills the folder it was made from", () => {
    const tree = { ...folder("tree"), worktree_of: "/code/second" };
    // The worktree is the only folder besides its source: the source is the sole startable one.
    const pair = project([folder("second"), tree]);
    const inTree = (value: ProjectSummary): WorkspaceSnapshot => ({
      ...workspace(value),
      chats: [
        {
          chat: { ...chatSummary("in-tree"), updated_at: 30 },
          project_id: "project",
          folder_id: "tree",
          pinned: false,
          archived: false,
        },
      ],
    });
    expect(projectChatContext(pair, inTree(pair)).folderId).toBe("second");
    // With several folders, the recent worktree chat stands for its source.
    const several = project([folder("first"), folder("second"), tree]);
    expect(projectChatContext(several, inTree(several)).folderId).toBe("second");
    // Without any retained record: Rust marked the folder, so nothing waits for the list.
    const context = { ...projectChatContext(several, inTree(several)), folderId: "tree" };
    expect(projectSourceFolder(context, inTree(several), [])).toBe("second");
    // A worktree whose source is not in this project leaves the choice to the person.
    const orphan = project([folder("first"), folder("other"), { ...tree, worktree_of: "/gone" }]);
    expect(projectChatContext(orphan, inTree(orphan)).folderId).toBeNull();
    expect(projectSourceFolder({ ...context, folderId: "tree" }, inTree(orphan), [])).toBeNull();
    // A project of nothing but a worktree prefills nothing.
    const alone = project([tree]);
    expect(projectChatContext(alone, inTree(alone)).folderId).toBeNull();
  });
  it("chooses the draft project's folder for a pick, else the one used last, never a worktree", () => {
    const matches = {
      path: "/code/first",
      worktree: null,
      matches: [
        { folder_id: "b", project_id: "recent", project_name: "Recent", last_chat_at: 9 },
        { folder_id: "a", project_id: "draft", project_name: "Draft", last_chat_at: null },
      ],
    };
    expect(chosenMatch(matches, "draft")?.folder_id).toBe("a");
    expect(chosenMatch(matches, null)?.folder_id).toBe("b");
    expect(chosenMatch(matches, "unrelated")?.folder_id).toBe("b");
    expect(chosenMatch({ ...matches, matches: [] }, "draft")).toBeNull();
    expect(
      chosenMatch({ ...matches, worktree: { branch: "ghost/x", source: "/code/src" } }, "draft"),
    ).toBeNull();
    expect(chosenMatch(null, "draft")).toBeNull();
  });
  it("uses project then profile agent defaults without replacing a deliberate selection", () => {
    const value = project([folder("first")]);
    const context = projectChatContext(value, workspace(value));
    expect(newChatAgent(context, "codex", [agent("claude"), agent("codex")], null)).toBe("claude");
    expect(newChatAgent(context, "codex", [agent("codex")], null)).toBe("codex");
    expect(newChatAgent(context, "codex", [agent("claude", true), agent("codex")], null)).toBe(
      "wait",
    );
    expect(newChatAgent(context, "codex", [agent("claude")], "hermes")).toBeNull();
  });
});
