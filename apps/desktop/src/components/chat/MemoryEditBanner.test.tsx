import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { Banners, type MemoryEditBanner, MemoryEditBannerView, noticeItems } from "./Banners";

const banner = (state: MemoryEditBanner["state"]): MemoryEditBanner => ({
  key: "one:r2",
  title: "Project memory updated",
  detail: "Claude Code · Fix the login flow · Ghost",
  text: "Claude Code in Fix the login flow updated project memory for Ghost.",
  state,
  onReview: vi.fn(),
  onUndo: vi.fn(),
  onDismiss: vi.fn(),
});
const view = (edit: MemoryEditBanner) =>
  renderToStaticMarkup(
    <ul>
      <MemoryEditBannerView edit={edit} />
    </ul>,
  );

describe("an agent's memory edit banner", () => {
  it("says who edited memory and offers Open, Undo and Dismiss without acting", () => {
    const edit = banner({ status: "shown" });
    const html = view(edit);
    expect(html).toContain(">Project memory updated<");
    expect(html).toContain(">Claude Code · Fix the login flow · Ghost<");
    // The whole sentence stays in the tooltip, for a line shortened to fit.
    expect(html).toContain(
      'title="Claude Code in Fix the login flow updated project memory for Ghost."',
    );
    expect(html).toContain('role="status"');
    for (const label of [">Open<", ">Undo<", 'aria-label="Dismiss"']) expect(html).toContain(label);
    expect(html).not.toContain('data-level="warning"');
    expect(edit.onUndo).not.toHaveBeenCalled();
    expect(edit.onReview).not.toHaveBeenCalled();
  });
  it("holds every action while undoing, and keeps only Dismiss once undone", () => {
    const undoing = view(banner({ status: "undoing" }));
    expect(undoing).toContain("Undoing…");
    expect(undoing.match(/disabled=""/g)).toHaveLength(3);
    const undone = view(banner({ status: "undone" }));
    expect(undone).not.toContain(">Undo<");
    expect(undone).not.toContain(">Open<");
    expect(undone).toContain('aria-label="Dismiss"');
  });
  it("announces a failed undo and keeps Open to look at it", () => {
    const html = view(
      banner({ status: "failed", message: "Nothing was undone. Project memory changed." }),
    );
    expect(html).toContain('role="alert"');
    expect(html).toContain("Nothing was undone. Project memory changed.");
    expect(html).toContain(">Open<");
    expect(html).not.toContain(">Undo<");
  });
  it("hides Review where nothing can open Project memory", () => {
    expect(view({ ...banner({ status: "shown" }), onReview: undefined })).not.toContain(">Open<");
  });
  it("floats while fresh, and stays in the panel without leaving by itself", () => {
    expect(renderToStaticMarkup(<Banners storage={null} />)).toBe("");
    expect(
      renderToStaticMarkup(<Banners storage={null} memoryEdits={[banner({ status: "shown" })]} />),
    ).toContain('data-testid="memory-edit-banner"');
    const items = noticeItems({
      storage: null,
      env: null,
      onRetryEnv: () => {},
      recoveryNotice: null,
      onRecoveryDismissed: () => {},
      notices: [],
      onDismissNotice: () => {},
      memoryEdits: [banner({ status: "shown" })],
    });
    expect(items.map((item) => [item.key, item.level])).toEqual([["memory:one:r2", undefined]]);
    expect(renderToStaticMarkup(<ul>{items.map((item) => item.node)}</ul>)).toContain(">Undo<");
  });
});
