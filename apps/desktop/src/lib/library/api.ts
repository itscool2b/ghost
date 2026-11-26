import { invoke } from "@tauri-apps/api/core";
import type { McpSnapshot } from "../../bindings";
import { toCommandError } from "../ipc";
import type {
  LibraryCopyPreview,
  LibraryCopyReceipt,
  LibraryCopyResult,
  LibraryInventory,
  LibraryItemPreview,
  LibraryItemRef,
} from "./bindings";

/** Thin native transport. DTOs are generated from ghost-config; all paths and mutations are Rust-owned. */
async function call<T>(command: string, args: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw toCommandError(error);
  }
}

export const libraryApi = {
  snapshot: (folderId: string | null) => call<LibraryInventory>("library_snapshot", { folderId }),
  preview: (folderId: string | null, item: LibraryItemRef, revision: string) =>
    call<LibraryItemPreview>("library_item_preview", { folderId, item, revision }),
  prepare: (folderId: string | null, item: LibraryItemRef, revision: string, toSource: string) =>
    call<LibraryCopyPreview>("library_copy_preview", { folderId, item, revision, toSource }),
  cancel: (id: string) => call<void>("library_copy_cancel", { id }),
  apply: (id: string) => call<LibraryCopyResult>("library_copy_apply", { id }),
  history: (folderId: string | null) =>
    call<LibraryCopyReceipt[]>("library_copy_history", { folderId }),
  rollback: (folderId: string | null, sourceId: string, id: string) =>
    call<LibraryCopyResult>("library_copy_rollback", { folderId, sourceId, id }),
  /**
   * Shares an agent's MCP server with every agent, as a shared definition for all projects. Its
   * env and header values stay on this device as local credentials; Rust checks the revision.
   */
  share: async (folderId: string | null, item: LibraryItemRef, revision: string) => {
    const shared = await call<McpSnapshot>("shared_mcp_snapshot", {});
    return call<McpSnapshot>("library_mcp_share", {
      folderId,
      item,
      revision,
      expected: shared.revision,
    });
  },
};

export type LibraryApi = typeof libraryApi;
