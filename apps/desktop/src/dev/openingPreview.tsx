/**
 * Development-only fixture of the sidebar while the app opens (`?preview=opening`). No native
 * command and no database: the two lists are invented and answer after `&late=<ms>` (600 when
 * absent). `&kept` starts from lists kept from the last launch, as lib/launchCache.ts hands them
 * over; one of their chats is gone from the database's own answer and one is new, so the checks
 * can watch the kept rows give way. `&fail` makes every read fail until Retry is pressed.
 */
import { StrictMode, useEffect, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import type { ChatSummary, ProjectSummary, WorkspaceChat, WorkspaceSnapshot } from "../bindings";
import { ChatList } from "../components/chat/ChatList";
import { SidebarBand, sidebarOpening } from "../components/chat/ChatsScreen";
import { LaunchArt } from "../components/chat/LaunchArt";
import { AppStore } from "../lib/chat/appStore";
import { chatSummary, FakeTransport } from "../lib/chat/testing";
import { CommandError } from "../lib/ipc";
import { launchChats, launchWorkspace } from "../lib/launchCache";
import { type WorkspaceApi, WorkspaceStore } from "../lib/workspace";

const project = (id: string, name: string): ProjectSummary => ({
  id,
  name,
  color: null,
  default_agent: null,
  default_model: null,
  folders: [],
  created_at: 1,
  updated_at: 1,
});
const chat = (id: string, title: string, updated: number, extra: Partial<ChatSummary> = {}) =>
  chatSummary(id, { title, updated_at: updated, status: "stopped", ...extra });
const entry = (item: ChatSummary, projectId: string): WorkspaceChat => ({
  chat: item,
  project_id: projectId,
  folder_id: null,
  pinned: false,
  archived: false,
});
const projects = [project("project-app", "o7"), project("project-site", "o7.codes")];

// What the last launch kept: one chat was working then, and one has been deleted since.
const kept = [
  chat("chat-login", "Fix the login flow", 9, { status: "running", needs_attention: true }),
  chat("chat-notes", "Ship the release notes", 7),
  chat("chat-gone", "A chat deleted since", 5),
  chat("chat-pricing", "Rebuild the pricing page", 3),
];
const keptWorkspace: WorkspaceSnapshot = {
  projects,
  chats: [
    entry(kept[0] as ChatSummary, "project-app"),
    entry(kept[1] as ChatSummary, "project-app"),
    entry(kept[2] as ChatSummary, "project-app"),
    entry(kept[3] as ChatSummary, "project-site"),
  ],
};
// What the database answers.
const live = [
  chat("chat-new", "Tidy the sidebar", 11),
  chat("chat-login", "Fix the login flow", 9),
  chat("chat-notes", "Ship the release notes", 7),
  chat("chat-pricing", "Rebuild the pricing page", 3),
];
const liveWorkspace: WorkspaceSnapshot = {
  projects,
  chats: [
    entry(live[0] as ChatSummary, "project-site"),
    entry(live[1] as ChatSummary, "project-app"),
    entry(live[2] as ChatSummary, "project-app"),
    entry(live[3] as ChatSummary, "project-site"),
  ],
};

function Opening({
  app,
  workspace,
  onRetry,
}: {
  readonly app: AppStore;
  readonly workspace: WorkspaceStore;
  readonly onRetry: () => void;
}) {
  const snapshot = useSyncExternalStore(app.subscribe, app.getSnapshot, app.getSnapshot);
  const organization = useSyncExternalStore(
    workspace.subscribe,
    workspace.getSnapshot,
    workspace.getSnapshot,
  );
  useEffect(() => {
    app.start();
    void workspace.refresh();
    return () => app.stop();
  }, [app, workspace]);
  const displayed = organization.data
    ? organization.data.chats.map((item) => item.chat)
    : snapshot.chats;
  return (
    <div className="chat-app" data-glass="none" data-os="windows" data-sidebar-collapsed={false}>
      <aside className="chat-sidebar" aria-label="Sidebar">
        <SidebarBand collapsed={false} onToggle={() => {}} />
        <ChatList
          chats={displayed}
          organization={organization.data}
          error={organization.error ?? snapshot.chatsError}
          opening={sidebarOpening(snapshot, organization, true)}
          onRetry={onRetry}
          selected={null}
          onSelect={(id) => {
            // The checks read which chat a row asked to open.
            document.body.dataset.openedChat = id;
          }}
          onNewChat={() => {}}
          onNewProject={() => {}}
          onShowArchived={() => {}}
        />
      </aside>
      <main className="chat-main">
        <LaunchArt />
      </main>
    </div>
  );
}

export function mountOpeningPreview(container: HTMLElement) {
  const params = new URLSearchParams(location.search);
  const late = Number(params.get("late") ?? 600);
  let failing = params.has("fail");
  const after = <T,>(value: T): Promise<T> =>
    new Promise((resolve, reject) => {
      setTimeout(() => {
        if (failing) {
          reject(
            new CommandError(
              "storage_unavailable",
              "o7's database is not available: The disk is full.",
            ),
          );
        } else resolve(value);
      }, late);
    });
  // The reads the stores repeat by themselves keep failing too: only Retry lets one through.
  const app = new AppStore(new FakeTransport({ chatsList: () => after(live) }));
  const workspace = new WorkspaceStore({
    snapshot: () => after(liveWorkspace),
  } as unknown as WorkspaceApi);
  if (params.has("kept")) {
    // Through the same reader the app uses, so the fixture's rows are at rest as the app's are.
    const chats = launchChats(JSON.stringify(kept));
    const lists = launchWorkspace(JSON.stringify(keptWorkspace));
    if (chats !== null) app.hydrate(chats);
    if (lists !== null) workspace.hydrate(lists);
  }
  container.style.height = "100vh";
  createRoot(container).render(
    <StrictMode>
      <Opening
        app={app}
        workspace={workspace}
        onRetry={() => {
          failing = false;
          void app.reloadChats();
          void workspace.refresh();
        }}
      />
    </StrictMode>,
  );
}
