import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceSnapshot } from "../bindings";
import { type WorkspaceApi, WorkspaceStore } from "./workspace";

describe("the workspace after a restart", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("reads again until the database answers, then stops", async () => {
    let unavailable = 3;
    const snapshot = vi.fn(() =>
      unavailable-- > 0
        ? Promise.reject(new Error("Ghost's database is not available."))
        : Promise.resolve({ projects: [], chats: [] } as unknown as WorkspaceSnapshot),
    );
    const store = new WorkspaceStore({ snapshot } as unknown as WorkspaceApi, [10, 20]);
    await store.refresh();
    expect(store.getSnapshot().error).toMatch(/not available/);
    await vi.advanceTimersByTimeAsync(10 + 20 + 20);
    expect(store.getSnapshot().data).not.toBeNull();
    expect(store.getSnapshot().error).toBeNull();
    const reads = snapshot.mock.calls.length;
    await vi.advanceTimersByTimeAsync(1000);
    expect(snapshot.mock.calls.length).toBe(reads);
  });

  it("stops waiting once the profile closes", async () => {
    const snapshot = vi.fn(() => Promise.reject(new Error("closed")));
    const store = new WorkspaceStore({ snapshot } as unknown as WorkspaceApi, [10]);
    await store.refresh();
    store.dispose();
    await vi.advanceTimersByTimeAsync(1000);
    expect(snapshot).toHaveBeenCalledTimes(1);
  });
});
