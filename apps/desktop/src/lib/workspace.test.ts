import { describe, expect, it, vi } from "vitest";
import type { WorkspaceSnapshot } from "../bindings";
import { type WorkspaceApi, WorkspaceStore } from "./workspace";

function deferred<T>() {
  let resolve: (value: T) => void = () => {};
  let reject: (reason: unknown) => void = () => {};
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

const snapshotOf = (name: string) =>
  ({ projects: [{ id: name }], chats: [] }) as unknown as WorkspaceSnapshot;

describe("workspace operations", () => {
  it("shows the workspace kept from the last launch until the first read answers", async () => {
    const read = deferred<WorkspaceSnapshot>();
    const store = new WorkspaceStore({ snapshot: () => read.promise } as unknown as WorkspaceApi);
    store.hydrate(snapshotOf("kept"));
    expect(store.getSnapshot()).toMatchObject({ data: snapshotOf("kept"), cached: true });
    const reading = store.refresh();
    expect(store.getSnapshot().cached).toBe(true);
    read.resolve(snapshotOf("read"));
    await reading;
    expect(store.getSnapshot()).toMatchObject({ data: snapshotOf("read"), cached: false });
    // What was kept never replaces what was read.
    store.hydrate(snapshotOf("stale"));
    expect(store.getSnapshot()).toMatchObject({ data: snapshotOf("read"), cached: false });
  });

  it("runs two operations at once, each with its own result or error", async () => {
    const snapshot = vi.fn(async () => snapshotOf("after"));
    const store = new WorkspaceStore({ snapshot } as unknown as WorkspaceApi);
    const first = deferred<string>();
    const second = deferred<string>();
    const started: string[] = [];
    const creating = store.perform(() => {
      started.push("create");
      return first.promise;
    });
    const stopping = store.perform(() => {
      started.push("stop");
      return second.promise;
    });
    // The second did not wait for the first, and was not dropped.
    expect(started).toEqual(["create", "stop"]);
    expect(store.getSnapshot().pending).toBe(2);
    second.reject(new Error("The folder is busy."));
    expect(await stopping).toEqual({ ok: false, error: "The folder is busy." });
    // One caller's failure is its own: the workspace itself read fine.
    expect(store.getSnapshot().error).toBeNull();
    expect(store.getSnapshot().pending).toBe(1);
    first.resolve("made");
    expect(await creating).toEqual({ ok: true, value: "made" });
    expect(store.getSnapshot().pending).toBe(0);
    expect(store.getSnapshot().data).toEqual(snapshotOf("after"));
  });

  it("answers an operation only after a read that shows its change, when a later read overtook its own", async () => {
    const reads = [deferred<WorkspaceSnapshot>(), deferred<WorkspaceSnapshot>()];
    let next = 0;
    const snapshot = vi.fn(() => reads[next++]?.promise ?? Promise.resolve(snapshotOf("late")));
    const store = new WorkspaceStore({ snapshot } as unknown as WorkspaceApi);
    let answered = false;
    const made = store
      .perform(async () => "made")
      .then((outcome) => {
        answered = true;
        return outcome;
      });
    await vi.waitFor(() => expect(snapshot).toHaveBeenCalledTimes(1));
    // Another screen reads meanwhile: the operation's own read is superseded.
    const later = store.refresh();
    reads[0]?.resolve(snapshotOf("stale"));
    await Promise.resolve();
    await Promise.resolve();
    expect(answered).toBe(false);
    reads[1]?.resolve(snapshotOf("fresh"));
    await later;
    expect(await made).toEqual({ ok: true, value: "made" });
    expect(store.getSnapshot().data).toEqual(snapshotOf("fresh"));
  });
});
