import { PanelLeft } from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import { flushSync } from "react-dom";
import type {
  AppInfo,
  ChatRole,
  InstructionsTarget,
  ProjectSummary,
  RoleModel,
} from "../../bindings";
import {
  availableUpdates,
  chatsWorking,
  updateNotices,
  workingChats,
} from "../../lib/agentUpdates";
import { type AppSnapshot, type AppStore, NewChatDraft } from "../../lib/chat/appStore";
import {
  type ChatsView,
  createdChatOpens,
  newChatAgent,
  projectChatContext,
} from "../../lib/chat/newChatContext";
import type { Transport } from "../../lib/chat/transport";
import { type CoordinatorApi, coordinatorApi, useCoordinationMembers } from "../../lib/coordinator";
import { restoreFocusSoon } from "../../lib/focusReturn";
import { acceptLegal, legalNotice } from "../../lib/legal";
import { navigationTarget } from "../../lib/nativeNavigation";
import {
  agentEditText,
  agentEditToast,
  memoryChats,
  type ProjectMemoryApi,
  projectMemoryApi,
  useAgentMemoryEdits,
} from "../../lib/projectMemory";
import { type SetupApi, setupApi } from "../../lib/setup";
import { speechPlayer } from "../../lib/speech";
import { TerminalPanels } from "../../lib/terminalPanel";
import { markTourSeen } from "../../lib/tour/seen";
import { reachable, seenAfter, TOUR_STOPS, type TourStop, tourOffer } from "../../lib/tour/stops";
import type { UpdatesApi } from "../../lib/updates";
import { EMPTY_WORKSPACE, type WorkspaceStore, type WorkspaceView } from "../../lib/workspace";
import { ProjectPage } from "../project/ProjectPage";
import { Tour, type TourState } from "../tour/Tour";
import { type AgentUpdateActions, AgentUpdateBanner } from "./AgentUpdateBanner";
import { AppUpdate } from "./AppUpdate";
import { Banners, noticeItems } from "./Banners";
import { ChatActionsDialog } from "./ChatActionsDialog";
import { ChatList, projectName } from "./ChatList";
import { ChatView } from "./ChatView";
import { CommandPalette, type PaletteCommand } from "./CommandPalette";
import { LaunchArt } from "./LaunchArt";
import { NewChat } from "./NewChat";
import { Notifications } from "./Notifications";
import { ProjectDialog } from "./ProjectDialog";
import { SidebarFooter } from "./SidebarFooter";
import { createXtermRuntime } from "./terminalRuntime";

const noSubscribe = () => () => {};

/** How long the window is left alone before the tour is offered. */
const TOUR_OFFER_AFTER_MS = 700;

/** `set` with `key` in or out, the same set when nothing changes. */
/**
 * Leaving the start screen for New chat, the start screen's wing flies up into New chat's own: the
 * two share the view-transition name `ghost-wing` (launch-art.css, new-chat.css). Only where the
 * webview has view transitions and motion is welcome; anywhere else New chat simply opens.
 */
export function flyWing(update: () => void): void {
  const root = document.documentElement;
  const still =
    matchMedia("(prefers-reduced-motion: reduce)").matches ||
    root.matches('[data-motion="reduced"], [data-motion="off"]');
  if (
    still ||
    typeof document.startViewTransition !== "function" ||
    !document.querySelector(".launch-art")
  ) {
    update();
    return;
  }
  root.setAttribute("data-wing-flight", "");
  const flight = document.startViewTransition(() => flushSync(update));
  void flight.finished.finally(() => root.removeAttribute("data-wing-flight"));
}

function toggled(set: ReadonlySet<string>, key: string, present: boolean): ReadonlySet<string> {
  if (set.has(key) === present) return set;
  const next = new Set(set);
  if (present) next.add(key);
  else next.delete(key);
  return next;
}
const emptyWorkspace = () => EMPTY_WORKSPACE;

/**
 * Whether the sidebar still waits for the database's own lists: nothing was read yet, or what
 * shows is what the last launch kept. `workspace` says whether this screen reads the workspace
 * at all.
 */
export function sidebarOpening(
  snapshot: Pick<AppSnapshot, "chats" | "chatsCached">,
  organization: Pick<WorkspaceView, "data" | "cached">,
  workspace: boolean,
): boolean {
  return (
    snapshot.chats === null ||
    snapshot.chatsCached ||
    organization.cached ||
    (workspace && organization.data === null)
  );
}

/**
 * The sidebar's top band: the collapse toggle at the right (Grok's). The band drags the window and
 * leaves the left to the macOS traffic lights; the logo sits in the footer beside Settings.
 */
export function SidebarBand({
  collapsed,
  onToggle,
}: {
  readonly collapsed: boolean;
  readonly onToggle: () => void;
}) {
  return (
    <>
      <div className="chat-drag" data-tauri-drag-region="" />
      <button
        type="button"
        className="g-button g-button-ghost g-button-icon g-button-28 chat-sidebar-toggle"
        aria-label={collapsed ? "Expand sidebar" : "Collapse sidebar"}
        aria-keyshortcuts="Control+\ Meta+\"
        aria-expanded={!collapsed}
        onClick={onToggle}
      >
        <PanelLeft size={16} />
      </button>
    </>
  );
}

/**
 * The chats: the sidebar with New chat and the projects' chats, the banners, and the open chat
 * (or New chat). Diagnostics (the M0 info card and stress view) is one button away.
 */
export function ChatsScreen({
  app,
  transport,
  info,
  onRecoveryDismissed,
  onDiagnostics,
  onAgents,
  dashboardOpen = false,
  onDashboard,
  onSettings,
  onCustomize,
  onInstructions,
  active = true,
  workspace,
  openChatRequest,
  newChatRequest,
  setup = setupApi,
  memory = projectMemoryApi,
  onOpenMemory,
  updates,
  coordinator = coordinatorApi,
  offersTour = false,
  tourRequest,
  tourArrive = false,
}: {
  readonly app: AppStore;
  readonly transport: Transport;
  readonly info: AppInfo;
  /** The shell acknowledged the database repair, so its notice goes for good. */
  readonly onRecoveryDismissed: () => void;
  readonly onDiagnostics: () => void;
  readonly onAgents?: () => void;
  /** Opens the Dashboard: every chat by what it needs, and what its agent runs. */
  /** The Dashboard is open over the chats: its button in the sidebar shows it. */
  readonly dashboardOpen?: boolean;
  readonly onDashboard?: () => void;
  readonly onSettings?: () => void;
  readonly onCustomize?: (projectId: string | null, activeChatId?: string) => void;
  /** Opens Settings' Instructions on a scope: global, a project folder, or a chat's folder. */
  readonly onInstructions?: (target: InstructionsTarget, chatLabel?: string) => void;
  readonly active?: boolean;
  readonly workspace?: WorkspaceStore;
  readonly openChatRequest?: { id: string };
  /** Opens New chat with this agent chosen (setup's "Explore the local demo" asks for the Demo agent). */
  readonly newChatRequest?: { id: string; agent: string };
  /** Settings' saved preferences: New chat starts with the default agent selected. */
  readonly setup?: SetupApi;
  /** Project memory: an agent's direct edit shows a banner with Review and Undo. */
  readonly memory?: ProjectMemoryApi;
  /** Opens Customize > Project memory on a revision, with its changes. */
  /** Opens Settings' Project memory on a project, from a notice of an agent's edit. */
  readonly onOpenMemory?: (projectId: string) => void;
  /** The app's updater: Update shows at the sidebar's foot while a newer version is on offer. */
  readonly updates?: UpdatesApi;
  /** Coordinated projects (an experiment): shown only while Settings > Experiments has it on. */
  readonly coordinator?: CoordinatorApi;
  /**
   * Offer the tour here by itself: to a profile that never met it, and the stops added since to
   * one that has. Off in previews and checks, which ask for the tour when they want it.
   */
  readonly offersTour?: boolean;
  /** Take the whole tour now (Settings > General). */
  readonly tourRequest?: { id: string };
  /** Setup just ended: the wing flies in to the sidebar's foot before it offers the tour. */
  readonly tourArrive?: boolean;
}) {
  const snapshot = useSyncExternalStore(app.subscribe, app.getSnapshot, app.getSnapshot);
  const dismissedUpdates = useSyncExternalStore(
    updateNotices.subscribe,
    updateNotices.getSnapshot,
    updateNotices.getSnapshot,
  );
  // One object for the screen's life, so each notice asks its agent's maintenance once.
  const [updateActions] = useState<AgentUpdateActions>(() => ({
    maintenance: (agentId) => transport.agentMaintenance(agentId),
    update: (agentId) => transport.agentUpdate(agentId),
    refresh: () => app.refreshAgents(),
    openAgents: onAgents,
    terminal: transport.updateTerminal,
  }));
  const memoryEdits = useAgentMemoryEdits(memory);
  // A memory edit's toast moves on by itself; the edit stays in the notifications panel.
  const [toastedEdits, setToastedEdits] = useState<ReadonlySet<string>>(new Set());
  // The Terms of Service and Privacy Policy, once, until this copy accepts them (lib/legal.ts).
  const [legal, setLegal] = useState(() => legalNotice());
  // Startup notices put away for this run, and agents whose last update failed (a red bell).
  const [dismissedNotices, setDismissedNotices] = useState<ReadonlySet<string>>(new Set());
  const [failedUpdates, setFailedUpdates] = useState<ReadonlySet<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);
  // A chat switch by keyboard (Ctrl+1 to 9) takes the focused terminal or composer away with the
  // old chat: focus returns to what held it last and is on screen (lib/focusReturn.ts).
  useEffect(() => {
    if (selected !== null) restoreFocusSoon();
  }, [selected]);
  // Launch shows the start screen's aircraft; New chat, a project's New chat and Ctrl+N open the
  // composer, which then stays whenever no chat is open.
  const [composing, setComposing] = useState(false);
  const [draft, setDraft] = useState(() => new NewChatDraft(transport));
  // What shows now, for a create that finishes later: its chat opens only while its draft shows.
  const showing = useRef<ChatsView<NewChatDraft>>({ selected, composing, draft });
  useLayoutEffect(() => {
    showing.current = { selected, composing, draft };
  });
  // Settings' default agent, read again whenever the chats come back into view (from Settings).
  const [defaultAgent, setDefaultAgent] = useState<string | null | undefined>(undefined);
  // Coordinated projects, an experiment: read with the default agent, so turning it on or off in
  // Settings shows here when the chats come back into view.
  const [coordination, setCoordination] = useState(false);
  // The project whose page is open in the chat pane's place.
  const [boardProject, setBoardProject] = useState<string | null>(null);
  const members = useCoordinationMembers(coordinator, coordination);
  // Leaving the chats for Settings ends what was being read aloud, as closing its chat does
  // (ChatView): no voice is left talking with its stop button out of reach.
  // biome-ignore lint/correctness/useExhaustiveDependencies: leaving the view is what ends it.
  useEffect(() => () => speechPlayer.stop(), [active]);
  useEffect(() => {
    if (!active) return;
    let live = true;
    setup.snapshot().then(
      (saved) => {
        if (!live) return;
        setDefaultAgent(saved.preferences.general.default_agent);
        // The voice answers are read in, chosen in Settings > Voice.
        speechPlayer.setVoice(saved.preferences.general.speech_voice ?? null);
        // A profile still in setup is not offered the tour; setup's end offers it.
        setTourSeen(saved.preferences.completed ? (saved.preferences.tour_seen ?? []) : null);
        setCoordination(saved.preferences.general.coordinated_projects);
        if (!saved.preferences.general.coordinated_projects) setBoardProject(null);
      },
      () => {
        // Without saved preferences New chat simply waits for a choice.
        if (live) setDefaultAgent(null);
      },
    );
    return () => {
      live = false;
    };
  }, [active, setup]);
  const [collapsed, setCollapsed] = useState(false);
  // The tour: what it is doing, the stops this profile has seen, and what the window showed
  // before a flight, which its end puts back.
  const [tour, setTour] = useState<TourState | null>(null);
  const [tourSeen, setTourSeen] = useState<readonly string[] | null>(null);
  const tourBefore = useRef<{ selected: string | null; composing: boolean; collapsed: boolean }>(
    null,
  );
  const arrived = useRef(false);
  const organization = useSyncExternalStore(
    workspace?.subscribe ?? noSubscribe,
    workspace?.getSnapshot ?? emptyWorkspace,
    workspace?.getSnapshot ?? emptyWorkspace,
  );
  const [archived, setArchived] = useState(false);
  const [projectDialog, setProjectDialog] = useState<{ project: ProjectSummary | null } | null>(
    null,
  );
  const [chatOptions, setChatOptions] = useState<string | null>(null);
  const [palette, setPalette] = useState(false);
  const [chatCommands, setChatCommands] = useState<readonly PaletteCommand[]>([]);
  // Docked terminals belong to the screen, not the open chat, so a chat's shells keep running
  // while another chat shows. Leaving the screen stops them all.
  const [terminals] = useState(() =>
    transport.projectTools ? new TerminalPanels(transport.projectTools, createXtermRuntime) : null,
  );
  useEffect(() => () => void terminals?.dispose(), [terminals]);
  const chatIds = snapshot.chats?.map((chat) => chat.id).join(",") ?? "";
  // A new or externally created chat can also create a project. Live status text is merged below.
  // biome-ignore lint/correctness/useExhaustiveDependencies: Membership refresh depends on the set of listed IDs, not each status update.
  useEffect(() => {
    void workspace?.refresh();
  }, [workspace, chatIds]);
  const liveById = new Map(snapshot.chats?.map((chat) => [chat.id, chat]));
  const displayed = organization.data
    ? organization.data.chats
        .filter((entry) => entry.archived === archived)
        .map((entry) => liveById.get(entry.chat.id) ?? entry.chat)
    : snapshot.chats;
  // Saved chats first, then the live list, so a chat's current title names its agent's edits.
  const knownChats = memoryChats([
    ...(organization.data?.chats.map((entry) => entry.chat) ?? []),
    ...(snapshot.chats ?? []),
  ]);
  const memoryBanners = memoryEdits.notices.map((notice) => {
    const project =
      organization.data?.projects.find((project) => project.id === notice.projectId)?.name ?? null;
    return {
      key: `${notice.projectId}:${notice.revision}`,
      ...agentEditToast(notice, knownChats, project),
      text: agentEditText(notice, knownChats, project),
      state: notice.state,
      onReview: onOpenMemory
        ? () => {
            memoryEdits.dismiss(notice);
            onOpenMemory(notice.projectId);
          }
        : undefined,
      onUndo: () => void memoryEdits.undo(notice),
      onDismiss: () => memoryEdits.dismiss(notice),
    };
  });
  // A project that went away (deleted, or the experiment turned off) closes its page.
  const boardPage =
    coordination && boardProject !== null
      ? (organization.data?.projects.find((project) => project.id === boardProject) ?? null)
      : null;
  const optionsEntry = organization.data?.chats.find((entry) => entry.chat.id === chatOptions);
  const selectedEntry = organization.data?.chats.find((entry) => entry.chat.id === selected);
  // The open chat's Plan with or Build with, saved with the chat; a refusal says why in the menu.
  const listedChat = selectedEntry?.chat.id ?? null;
  const setRole = useCallback(
    async (role: ChatRole, choice: RoleModel | null) => {
      if (!workspace || listedChat === null) return;
      const outcome = await workspace.perform(() =>
        workspace.api.setRole(listedChat, role, choice),
      );
      if (!outcome.ok) throw new Error(outcome.error);
    },
    [workspace, listedChat],
  );
  const selectedProject =
    organization.data?.projects.find((project) => project.id === selectedEntry?.project_id) ?? null;
  const selectedFolder = selectedProject?.folders.find(
    (folder) => folder.id === selectedEntry?.folder_id,
  );
  // A fresh New chat selects the default agent once detection says it can run here; a choice the
  // person made (the dock, a click) is never replaced.
  const preselected = useRef(new WeakSet<NewChatDraft>());
  useEffect(() => {
    if (selected !== null || defaultAgent === undefined || preselected.current.has(draft)) return;
    const agent = newChatAgent(
      draft.context,
      defaultAgent,
      snapshot.agents,
      draft.getSnapshot().agentId,
    );
    if (agent === "wait") return;
    preselected.current.add(draft);
    if (agent !== null) draft.chooseAgent(agent);
  }, [selected, draft, defaultAgent, snapshot.agents]);
  // Setup's demo: New chat opens with the Demo agent chosen, which the default never replaces.
  // biome-ignore lint/correctness/useExhaustiveDependencies: Only a new request opens New chat.
  useEffect(() => {
    if (!newChatRequest) return;
    const next = new NewChatDraft(transport);
    preselected.current.add(next);
    next.chooseAgent(newChatRequest.agent);
    setSelected(null);
    setComposing(true);
    setDraft(next);
  }, [newChatRequest]);
  // A notification's chat opens only once the lists confirm this profile has it (docs/native-shell.md).
  const [pendingOpen, setPendingOpen] = useState<string | null>(null);
  useEffect(() => {
    if (openChatRequest) setPendingOpen(openChatRequest.id);
  }, [openChatRequest]);
  useEffect(() => {
    if (pendingOpen === null) return;
    // Only the database's own lists confirm it: the lists kept from the last launch are there to
    // be looked at, and count as not read yet.
    const read = organization.cached ? null : organization.data;
    const target = navigationTarget(pendingOpen, {
      chats: snapshot.chatsCached ? null : snapshot.chats,
      chatsFailed: snapshot.chatsError !== null,
      workspace: read,
      workspacePending: workspace !== undefined && read === null && organization.error === null,
    });
    if (target.kind === "wait") return;
    setPendingOpen(null);
    if (target.kind === "open") {
      setArchived(target.archived);
      setBoardProject(null);
      setSelected(target.chatId);
    }
  }, [
    pendingOpen,
    snapshot.chats,
    snapshot.chatsCached,
    snapshot.chatsError,
    organization,
    workspace,
  ]);

  useEffect(() => {
    app.start();
    return () => app.stop();
  }, [app]);

  const hasChat = (displayed?.length ?? 0) > 0;
  const opening = sidebarOpening(snapshot, organization, workspace !== undefined);
  const startTour = (stops: readonly TourStop[]) => {
    if (stops.length === 0) return;
    tourBefore.current = { selected, composing, collapsed };
    setCollapsed(false);
    setTour({ phase: "flight", stops });
  };
  /** Saves what a flight or a "Not now" means this profile has seen. */
  const recordTour = (stops: readonly string[]) => {
    if (stops.length === 0) return;
    setTourSeen((seen) => [...new Set([...(seen ?? []), ...stops])]);
    // A save that fails costs only a second offer on a later launch.
    void markTourSeen(setup, stops).then(setTourSeen, () => undefined);
  };
  // The tour is offered, never started by itself: once the window shows its own lists, to a
  // profile that never met it, and the stops added since to one that has.
  useEffect(() => {
    if (!offersTour || !active || tour !== null || tourSeen === null || opening) return;
    const offer = tourOffer(tourSeen, hasChat);
    if (!offer) return;
    const arrive = tourArrive && !arrived.current;
    const timer = setTimeout(
      () => {
        arrived.current = true;
        setTour({ phase: "offer", offer, arrive });
      },
      arrive ? 0 : TOUR_OFFER_AFTER_MS,
    );
    return () => clearTimeout(timer);
  }, [offersTour, active, tour, tourSeen, opening, hasChat, tourArrive]);
  // Leaving the chats (Settings, the Dashboard) puts the tour away; coming back offers it again.
  useEffect(() => {
    if (active) return;
    const before = tourBefore.current;
    tourBefore.current = null;
    if (before) setCollapsed(before.collapsed);
    setTour(null);
  }, [active]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Only a new request starts a flight.
  useEffect(() => {
    if (tourRequest) startTour(reachable(TOUR_STOPS, hasChat));
  }, [tourRequest]);

  useEffect(() => {
    if (!active) return;
    const toggle = (event: KeyboardEvent) => {
      // Not with Alt: AltGr (Ctrl+Alt on Windows) types the backslash on layouts such as German.
      if (
        event.defaultPrevented ||
        !(event.metaKey || event.ctrlKey) ||
        event.altKey ||
        event.key !== "\\" ||
        document.querySelector("dialog[open]")
      )
        return;
      event.preventDefault();
      setCollapsed((value) => !value);
    };
    window.addEventListener("keydown", toggle);
    return () => window.removeEventListener("keydown", toggle);
  }, [active]);

  return (
    <div
      className="chat-app"
      data-glass={info.glass}
      data-os={info.os}
      data-sidebar-collapsed={collapsed}
    >
      <aside className="chat-sidebar" aria-label="Sidebar">
        <SidebarBand collapsed={collapsed} onToggle={() => setCollapsed((value) => !value)} />
        <ChatList
          active={active}
          onSearch={transport.search && active ? () => setPalette(true) : undefined}
          chats={displayed}
          error={organization.error ?? snapshot.chatsError}
          opening={opening}
          onRetry={() => {
            void app.reloadChats();
            void workspace?.refresh();
          }}
          organization={organization.data}
          archived={archived}
          onShowArchived={workspace ? () => setArchived((value) => !value) : undefined}
          onNewProject={workspace ? () => setProjectDialog({ project: null }) : undefined}
          onProjectNewChat={
            workspace
              ? (project) => {
                  const data = organization.data;
                  if (!data) return;
                  flyWing(() => {
                    setBoardProject(null);
                    setSelected(null);
                    setComposing(true);
                    setArchived(false);
                    setDraft(new NewChatDraft(transport, projectChatContext(project, data)));
                  });
                }
              : undefined
          }
          onCustomize={
            onCustomize
              ? () =>
                  onCustomize(
                    selectedProject?.id ??
                      (selected === null ? draft.context?.projectId : null) ??
                      null,
                    selected ?? undefined,
                  )
              : undefined
          }
          dashboardOpen={dashboardOpen}
          onDashboard={onDashboard}
          onProjectSettings={workspace ? (project) => setProjectDialog({ project }) : undefined}
          onChatOptions={workspace ? (chat) => setChatOptions(chat.id) : undefined}
          onProjectBoard={
            coordination && workspace ? (project) => setBoardProject(project.id) : undefined
          }
          boardProject={boardProject}
          members={members}
          selected={boardProject === null ? selected : null}
          onSelect={(chatId) => {
            setBoardProject(null);
            setSelected(chatId);
          }}
          onNewChat={() =>
            flyWing(() => {
              setBoardProject(null);
              setSelected(null);
              setComposing(true);
              setDraft(new NewChatDraft(transport));
            })
          }
        />
        <SidebarFooter
          onSettings={onSettings}
          wingAway={tour !== null && active}
          update={
            updates ? <AppUpdate api={updates} working={chatsWorking(snapshot.chats)} /> : undefined
          }
          notifications={
            <Notifications
              items={noticeItems({
                storage: snapshot.storage,
                env: snapshot.env,
                onRetryEnv: () => void app.retryEnv(),
                recoveryNotice: info.recovery_notice,
                onRecoveryDismissed,
                notices: info.notices.filter((notice) => !dismissedNotices.has(notice)),
                onDismissNotice: (notice) =>
                  setDismissedNotices((current) => toggled(current, notice, true)),
                memoryEdits: memoryBanners,
                updates: availableUpdates(snapshot.agents)
                  .filter((update) => !dismissedUpdates.has(update.key))
                  .map((update) => ({
                    key: `update:${update.key}`,
                    ...(failedUpdates.has(update.key) ? { level: "error" as const } : {}),
                    node: (
                      <AgentUpdateBanner
                        update={update}
                        working={workingChats(snapshot.chats, update.agentId)}
                        actions={updateActions}
                        onDismiss={() => updateNotices.dismiss(update.key)}
                        onFailed={(failed) =>
                          setFailedUpdates((current) => toggled(current, update.key, failed))
                        }
                      />
                    ),
                  })),
              })}
            />
          }
        />
      </aside>
      <main className="chat-main">
        <div className="chat-drag" data-tauri-drag-region="" />
        <Banners
          storage={snapshot.storage}
          memoryEdits={memoryBanners.filter((edit) => !toastedEdits.has(edit.key))}
          onExpire={(key) => setToastedEdits((current) => toggled(current, key, true))}
          legal={
            legal
              ? {
                  ...legal,
                  onAccept: () => {
                    acceptLegal();
                    setLegal(null);
                  },
                }
              : null
          }
        />
        {boardPage ? (
          <ProjectPage
            key={boardPage.id}
            api={coordinator}
            project={boardPage}
            chats={snapshot.chats ?? []}
            agents={snapshot.agents}
            onOpenChat={(chatId) => {
              setBoardProject(null);
              setArchived(false);
              setSelected(chatId);
            }}
          />
        ) : selected === null && !composing ? (
          <LaunchArt />
        ) : selected === null ? (
          <NewChat
            key={draft.key}
            workspace={workspace}
            agents={snapshot.agents}
            refreshing={snapshot.agentsRefreshing}
            draft={draft}
            onRefreshAgents={() => void app.refreshAgents()}
            onCreated={(chat) => {
              app.addChat(chat);
              if (createdChatOpens(showing.current, draft)) setSelected(chat.id);
              void workspace?.refresh();
            }}
          />
        ) : (
          <ChatView
            active={active}
            onOpenChat={(id) => {
              setSelected(id);
              void app.reloadChats();
              void workspace?.refresh();
            }}
            key={selected}
            chatId={selected}
            onCommands={setChatCommands}
            terminals={terminals}
            transport={transport}
            listed={snapshot.chats?.find((chat) => chat.id === selected) ?? null}
            project={selectedProject?.name ?? null}
            folderLabel={selectedFolder?.display_name ?? null}
            storage={snapshot.storage}
            agents={snapshot.agents}
            onManageAccounts={onAgents}
            roles={selectedEntry?.roles ?? null}
            onSetRole={workspace && listedChat !== null ? setRole : undefined}
          />
        )}
      </main>
      {palette && active ? (
        <CommandPalette
          transport={transport}
          chatId={selected}
          onClose={() => setPalette(false)}
          onChat={(chatId) => {
            setBoardProject(null);
            setSelected(chatId);
          }}
          onProject={
            workspace
              ? (id) => {
                  const project = organization.data?.projects.find((project) => project.id === id);
                  if (project) setProjectDialog({ project });
                }
              : undefined
          }
          commands={[
            ...(selected !== null ? chatCommands : []),
            ...(onSettings
              ? [
                  {
                    id: "settings",
                    title: "Open settings",
                    detail: "Preferences and application updates",
                    run: onSettings,
                  },
                ]
              : []),
            ...(onInstructions
              ? [
                  {
                    id: "instructions",
                    title: "Edit instructions",
                    detail: "Your user files: one for every agent or one per agent",
                    run: () => onInstructions({ type: "global" }),
                  },
                  ...(selected !== null
                    ? [
                        {
                          id: "chat-instructions",
                          title: "Edit this chat's instructions",
                          detail: "AGENTS.md and CLAUDE.md in the chat's folder",
                          run: () => {
                            const chat = liveById.get(selected);
                            onInstructions(
                              { type: "chat", chat_id: selected },
                              chat
                                ? `This chat: ${projectName(chat.folder)}`
                                : "This chat's folder",
                            );
                          },
                        },
                      ]
                    : []),
                ]
              : []),
            {
              id: "new",
              title: "New chat",
              detail: "Choose an agent and project",
              run: () =>
                flyWing(() => {
                  setSelected(null);
                  setComposing(true);
                  setDraft(new NewChatDraft(transport));
                }),
            },
            ...(workspace
              ? [
                  {
                    id: "project",
                    title: "New project",
                    detail: "Add a folder to your workspace",
                    run: () => setProjectDialog({ project: null }),
                  },
                  {
                    id: "archived",
                    title: archived ? "Show active chats" : "Show archived chats",
                    detail: "Change the sidebar view",
                    run: () => setArchived((value) => !value),
                  },
                ]
              : []),
            ...(onAgents
              ? [
                  {
                    id: "manage-agents",
                    title: "Open agents",
                    detail: "Install, update and sign in to agents",
                    run: onAgents,
                  },
                ]
              : []),
            {
              id: "agents",
              title: "Refresh agents",
              detail: "Check installed agent availability",
              run: () => {
                void app.refreshAgents();
              },
            },
            {
              id: "sidebar",
              title: collapsed ? "Expand sidebar" : "Collapse sidebar",
              detail: "Make room for the conversation",
              run: () => setCollapsed((value) => !value),
            },
            {
              id: "tour",
              title: "Take the tour",
              detail: "The wing shows you around, about a minute",
              run: () => startTour(reachable(TOUR_STOPS, hasChat)),
            },
            {
              id: "diagnostics",
              title: "Open diagnostics",
              detail: "Storage and app information",
              run: onDiagnostics,
            },
          ]}
        />
      ) : null}
      {tour && active ? (
        <Tour
          state={tour}
          onTake={() => {
            if (tour.phase === "offer") startTour(tour.offer.stops);
          }}
          onDecline={() => {
            setTour(null);
            recordTour(seenAfter("declined", []));
          }}
          onPrepare={(stop) => {
            if (stop.place === "window") return;
            setBoardProject(null);
            if (stop.place === "new-chat") {
              setSelected(null);
              setComposing(true);
              return;
            }
            // A stop inside a chat: the chat that was open, else the first one listed.
            const chat = tourBefore.current?.selected ?? displayed?.[0]?.id ?? null;
            if (chat !== null) setSelected(chat);
          }}
          onEnd={(outcome, visited) => {
            const before = tourBefore.current;
            tourBefore.current = null;
            setTour(null);
            if (before) {
              setSelected(before.selected);
              setComposing(before.composing);
              setCollapsed(before.collapsed);
            }
            recordTour(seenAfter(outcome, visited));
            restoreFocusSoon();
          }}
        />
      ) : null}
      {projectDialog && workspace ? (
        <ProjectDialog
          key={`${info.profile?.id}:${projectDialog.project?.id ?? "new"}`}
          project={projectDialog.project}
          profileId={info.profile?.id}
          onLocated={() => app.reloadChats()}
          store={workspace}
          transport={transport}
          agents={snapshot.agents}
          onClose={() => setProjectDialog(null)}
          onInstructions={
            onInstructions
              ? (folderId) => {
                  setProjectDialog(null);
                  onInstructions({ type: "folder", folder_id: folderId });
                }
              : undefined
          }
        />
      ) : null}
      {optionsEntry && workspace ? (
        <ChatActionsDialog
          entry={optionsEntry}
          chat={liveById.get(optionsEntry.chat.id) ?? optionsEntry.chat}
          store={workspace}
          transport={transport}
          onChanged={(chat) => {
            app.addChat(chat);
            void app.reloadChats();
          }}
          onClose={() => setChatOptions(null)}
        />
      ) : null}
    </div>
  );
}
