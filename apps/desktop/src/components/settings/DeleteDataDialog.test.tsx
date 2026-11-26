import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { DataCleanupReview } from "../../bindings";
import {
  cleanupChoice,
  confirmed,
  deleteLabel,
  formatBytes,
  itemLabel,
  worktreeName,
} from "../../lib/dataCleanup";
import { DeleteDataBody, type DeleteDataPhase } from "./DeleteDataDialog";

const REVIEW: DataCleanupReview = {
  token: "one-use",
  scope: "machine",
  items: [
    {
      kind: "data",
      path: "C:\\Users\\you\\AppData\\Local\\si.auracode.desktop",
      bytes: 1_288_490_189,
    },
    { kind: "app", path: "C:\\Users\\you\\AppData\\Roaming\\si.auracode.desktop", bytes: null },
  ],
  worktrees: [
    {
      path: "C:\\data\\profiles\\local\\worktrees\\a",
      repository: "C:\\code\\shop",
      branch: "ghost/fix-login",
      summary: "3 uncommitted files",
      unsaved: true,
    },
    {
      path: "C:\\data\\profiles\\local\\worktrees\\b",
      repository: "C:\\code\\shop",
      branch: "ghost/clean",
      summary: "Nothing unsaved",
      unsaved: false,
    },
  ],
  secure_store: "Windows Credential Manager",
  app_bundle: null,
};

function render(
  review: DataCleanupReview | null,
  options: {
    phase?: DeleteDataPhase;
    typed?: string;
    deleteUnsaved?: boolean;
    trash?: boolean;
    error?: string | null;
  } = {},
): string {
  return renderToStaticMarkup(
    <DeleteDataBody
      review={review}
      phase={options.phase ?? "ready"}
      error={options.error ?? null}
      deleteUnsaved={options.deleteUnsaved ?? false}
      moveToTrash={options.trash ?? false}
      typed={options.typed ?? ""}
      onDeleteUnsaved={() => {}}
      onMoveToTrash={() => {}}
      onTyped={() => {}}
      onRetry={() => {}}
      onCancel={() => {}}
      onDelete={() => {}}
    />,
  );
}

describe("Delete all Ghost data", () => {
  it("lists exactly what Rust reviewed, with sizes and the agent tokens", () => {
    const html = render(REVIEW);
    expect(html).toContain("<dt>Chats and settings</dt>");
    expect(html).toContain("C:\\Users\\you\\AppData\\Local\\si.auracode.desktop");
    expect(html).toContain("1.2 GB");
    expect(html).toContain("<dt>Window and web data</dt>");
    expect(html).toContain(
      "Sign-in tokens of agent accounts you added in Ghost, kept in Windows Credential Manager.",
    );
    expect(html).toContain("Your agents, their settings and your repositories are not touched.");
  });

  it("warns about worktrees with unsaved work and keeps them by default", () => {
    const html = render(REVIEW);
    expect(html).toContain("1 worktree has work saved nowhere else");
    expect(html).toContain("ghost/fix-login");
    expect(html).toContain("3 uncommitted files");
    expect(html).not.toContain("ghost/clean</span>");
    expect(html).toMatch(/data-testid="delete-data-unsaved"(?![^>]*checked)/);
    expect(html).toContain("1 worktree with nothing unsaved is deleted too");
    expect(cleanupChoice(REVIEW, false, false)).toEqual({
      remove_worktrees: [],
      move_app_to_trash: false,
    });
    expect(cleanupChoice(REVIEW, true, false).remove_worktrees).toEqual([
      "C:\\data\\profiles\\local\\worktrees\\a",
    ]);
  });

  it("enables the red pill only once DELETE is typed", () => {
    expect(render(REVIEW)).toMatch(/data-testid="delete-data-apply" disabled=""/);
    expect(render(REVIEW, { typed: "delete" })).toMatch(
      /data-testid="delete-data-apply" disabled=""/,
    );
    const ready = render(REVIEW, { typed: " DELETE " });
    expect(ready).toMatch(
      /class="g-button g-button-danger" data-testid="delete-data-apply">Delete data and quit</,
    );
    expect(confirmed("DELETE")).toBe(true);
    expect(confirmed("DELET")).toBe(false);
  });

  it("offers the Trash only where the app bundle can move", () => {
    expect(render(REVIEW)).not.toContain("Also move Ghost to the Trash");
    const mac = {
      ...REVIEW,
      app_bundle: "/Applications/Ghost.app",
      keys: [],
      warnings: [],
      secure_store: "the macOS keychain",
    };
    const html = render(mac, { trash: true, typed: "DELETE" });
    expect(html).toContain("Also move Ghost to the Trash");
    expect(html).toContain("Delete data and move to Trash");
    expect(cleanupChoice(mac, false, true).move_app_to_trash).toBe(true);
    expect(cleanupChoice(REVIEW, false, true).move_app_to_trash).toBe(false);
  });

  it("says a data folder of its own leaves the installed app's data alone", () => {
    const html = render({ ...REVIEW, scope: "data_folder", worktrees: [] });
    expect(html).toContain("the installed app&#x27;s data");
    expect(html).not.toContain("worktree has work");
  });

  it("says what stays, and names no account in the cloud", () => {
    const html = render(REVIEW);
    expect(html).toContain("Stays: your agents and their folders");
    expect(html).toContain("your repositories and branches.");
    expect(html).not.toContain("cloud");
    expect(html).not.toContain("Devices");
  });

  it("shows loading, a failed review with a retry, and quitting", () => {
    expect(render(null, { phase: "loading" })).toContain("Reading what Ghost keeps on this device");
    const failed = render(null, { error: "The review could not finish." });
    expect(failed).toContain('role="alert"');
    expect(failed).toContain("Review again");
    const quitting = render(REVIEW, { phase: "quitting", typed: "DELETE" });
    expect(quitting).toContain("Ghost is quitting.");
    expect(quitting).toMatch(/data-testid="delete-data-apply" disabled=""/);
  });

  it("words sizes, labels and names plainly, without em dashes", () => {
    expect(formatBytes(0)).toBe("1 KB");
    expect(formatBytes(12_000)).toBe("12 KB");
    expect(formatBytes(356_515_840)).toBe("340 MB");
    expect(formatBytes(1_288_490_189)).toBe("1.2 GB");
    expect(itemLabel({ kind: "temporary", path: "x", bytes: null }, "machine")).toBe(
      "Update downloads",
    );
    expect(itemLabel({ kind: "entry", path: "x", bytes: null }, "data_folder")).toBe(
      "In this folder",
    );
    expect(deleteLabel(false)).toBe("Delete data and quit");
    const [first] = REVIEW.worktrees;
    if (first) expect(worktreeName({ ...first, branch: null })).toBe("a");
    expect(render(REVIEW)).not.toContain("\u2014");
  });
});
