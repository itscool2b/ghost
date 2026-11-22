import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { WorkspaceSnapshot } from "../../bindings";
import { chatSummary } from "../../lib/chat/testing";
import { ChatList } from "./ChatList";

const noop = () => {};
const workspace = {
  projects: [{ id: "p1", name: "o7", color: null, folders: [], created_at: 1, updated_at: 1 }],
  chats: [
    {
      chat: chatSummary("a", { title: "Fix login" }),
      project_id: "p1",
      folder_id: null,
      pinned: false,
      archived: false,
    },
  ],
} as unknown as WorkspaceSnapshot;

describe("the sidebar while the app opens", () => {
  it("shows the lists kept from the last launch as its rows, at once and without a note", () => {
    const html = renderToStaticMarkup(
      <ChatList
        chats={[chatSummary("a", { title: "Fix login" })]}
        organization={workspace}
        error={null}
        selected={null}
        onSelect={noop}
        onNewChat={noop}
        opening
      />,
    );
    expect(html).toContain("Fix login");
    expect(html).toContain("o7");
    expect(html).not.toContain("chats-skeleton");
    expect(html).not.toContain("chats-opening");
    expect(html).not.toContain("chats-error");
    expect(html).not.toContain("aria-busy");
  });

  it("draws no placeholder on its first frame: a list that answers at once never flashes one", () => {
    const html = renderToStaticMarkup(
      <ChatList
        chats={null}
        error={null}
        selected={null}
        onSelect={noop}
        onNewChat={noop}
        opening
      />,
    );
    expect(html).not.toContain("chats-skeleton");
    expect(html).not.toContain("chats-opening");
    expect(html).toContain('aria-busy="true"');
    // Nothing claims the profile is empty before the database has said so.
    expect(html).not.toContain("will appear here");
  });

  it("keeps a real fault as an error, with the way to read the lists again", () => {
    const html = renderToStaticMarkup(
      <ChatList
        chats={null}
        error="o7's database is not available: The disk is full."
        selected={null}
        onSelect={noop}
        onNewChat={noop}
        opening
        onRetry={noop}
      />,
    );
    expect(html).toContain('data-testid="chats-error"');
    expect(html).toContain("The disk is full.");
    expect(html).toContain(">Retry<");
    expect(html).not.toContain("aria-busy");
  });
});
