import { AgentMark, PixelLoader, Wing } from "@ghost/ui/graphite";
import { ArrowUp, Check, ChevronDown, Folder, FolderOpen, RefreshCw, Search } from "lucide-react";
import {
  type CSSProperties,
  Fragment,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type {
  AgentAuthStatus,
  AgentEntry,
  AgentInstallation,
  ChatCreatePreview,
  ChatSummary,
  FolderMatches,
  ManagedWorktree,
  WorktreeChatPlan,
  WorktreeChatResult,
} from "../../bindings";
import { agentLineup } from "../../lib/chat/agentLineup";
import { agentChoice, type NewChatDraft } from "../../lib/chat/appStore";
import { holdFirstMessage } from "../../lib/chat/firstMessage";
import { chosenMatch, projectSourceFolder } from "../../lib/chat/newChatContext";
import { WorktreeChoice } from "../../lib/chat/worktreeChoice";
import { type ChatInterface, chatInterfaces, terminalSupported } from "../../lib/chatInterface";
import { errorMessage } from "../../lib/errors";
import { blockersFromChats, type FolderBusyApi, folderBusyApi } from "../../lib/folderBusy";
import { agentVersion, builtInPath } from "../../lib/format";
import { sameFolder } from "../../lib/gitWorkspace";
import { EMPTY_WORKSPACE, type WorkspaceStore } from "../../lib/workspace";
import { type WorktreeChatApi, worktreeChatApi } from "../../lib/worktreeChat";
import { type WorktreeRecoveryApi, worktreeRecoveryApi } from "../../lib/worktreeRecovery";
import { projectName } from "./ChatList";
import { FolderBusy } from "./FolderBusy";
import { closePopover, Popover } from "./Popover";
import { WorktreeChip } from "./WorktreeChip";
import { needsRecovery, WorktreeRecoveryList, worktreesToReview } from "./WorktreeRecoveryList";
import "../tour/wing.css";
import "./new-chat.css";
import { Select } from "../Select";

const noSubscribe = () => () => {};
const emptyWorkspace = () => EMPTY_WORKSPACE;

/** The download consent sentence of an agent whose first start downloads its adapter. */
export function downloadNote(agent: AgentEntry): string | null {
  if (!agent.needs_download) return null;
  // `launch` reads "<package> <version> with your <binary>" for npm adapters.
  const what = agent.launch.split(" with your ")[0] || agent.launch;
  return `Creating this chat downloads ${what} from npm (one time).`;
}

/** Installation evidence is separate from whether the version command succeeds. */
export function InstallationStatus({ installation }: { readonly installation: AgentInstallation }) {
  const channel = installation.channel;
  const label =
    channel.type === "npm"
      ? "npm"
      : channel.type === "homebrew"
        ? "Homebrew"
        : channel.type === "vendor_standalone"
          ? "Standalone"
          : channel.type === "bundled"
            ? `Managed by ${channel.application}`
            : "Install channel unknown";
  const evidence = installation.verification;
  return (
    <span className="ui-muted" data-testid="agent-installation">
      {label}
      {installation.host.type === "wsl" ? ` · WSL ${installation.host.distro}` : null}
      {evidence.state === "untested" ? (
        <small
          title={
            evidence.last_verified === null
              ? "No verified baseline"
              : `Last verified: ${evidence.last_verified}`
          }
        >
          {" "}
          · Untested version
        </small>
      ) : null}
      {evidence.state === "verified" ? (
        <small title={evidence.scope}> · {evidence.scope}</small>
      ) : null}
      {evidence.state === "unknown" ? <small> · Compatibility unverified</small> : null}
    </span>
  );
}

export function authStatusText(status: AgentAuthStatus): string {
  switch (status.state) {
    case "credentials_present":
      return "The agent reports local sign-in credentials. Access is confirmed by the agent when it runs.";
    case "sign_in_required":
      return "Sign in using the agent's own terminal login command.";
    case "unknown":
      return status.reason;
    case "unsupported":
      return "This agent has no supported sign-in status command.";
  }
}

/** What the selected agent will run, once detection found it. */
function readyLine(agent: AgentEntry): string | null {
  switch (agent.availability.state) {
    case "ready": {
      const path = builtInPath(agent.availability.path);
      return agent.availability.version === null
        ? path
        : `${agentVersion(agent.name, agent.availability.version)} (${path})`;
    }
    case "checking":
      return "Still checking this agent; the chat starts once it is found.";
    case "missing":
    case "broken":
      return null;
  }
}

/**
 * The selected agent in a few words, for the quiet line under the card: its name and version, or
 * that it is still being looked for. Its popover opens on the whole of `readyLine`.
 */
function factLine(agent: AgentEntry): string {
  switch (agent.availability.state) {
    case "ready":
      return agent.availability.version === null
        ? agent.name
        : `${agent.name} ${agentVersion(agent.name, agent.availability.version)}`;
    case "checking":
      return `Looking for ${agent.name}`;
    case "missing":
    case "broken":
      return agent.name;
  }
}

/**
 * A line whose words flip up into place one after another (new-chat.css, after X Money's opening),
 * read as one line by everyone else. `from` continues the count of a line that runs on.
 */
function Words({ text, from = 0 }: { readonly text: string; readonly from?: number }) {
  return text.split(" ").map((word, index) => (
    // biome-ignore lint/suspicious/noArrayIndexKey: a word's place in a fixed line is its identity
    <Fragment key={index}>
      {index > 0 ? " " : null}
      <span className="new-chat-word" style={{ "--word": from + index } as CSSProperties}>
        {word}
      </span>
    </Fragment>
  ));
}

/** An agent choice's state: still detected, found, or waiting on the person to sign in. */
type ChoiceState = "detecting" | "ready" | "sign-in";

const STATE_LABEL: Record<ChoiceState, string> = {
  detecting: "looking for it",
  ready: "ready",
  "sign-in": "needs sign-in",
};

/**
 * New chat, after Grok's start screen: the logo, one sentence, and one composer card holding the
 * agents that can run here (each showing whether it is still being looked for, ready, or waiting
 * on a sign-in), the folder, and Create. Agents that cannot run here wait under "Other agents"
 * with their reason. Creating the chat starts its agent; errors show under the card.
 *
 * In a Git project folder the chat gets its own worktree by default (the Codex app's per-thread
 * Worktree mode): one chip row under the card says where it will work, and its popover changes
 * that. A plain folder shows nothing extra and the chat works in it directly.
 */
export function NewChat({
  agents,
  refreshing,
  draft,
  onRefreshAgents,
  onCreated,
  workspace,
  worktreeApi = worktreeChatApi,
  recoveryApi = worktreeRecoveryApi,
  folderBusy = folderBusyApi,
}: {
  readonly agents: readonly AgentEntry[] | null;
  readonly refreshing: boolean;
  readonly draft: NewChatDraft;
  readonly onRefreshAgents: () => void;
  readonly onCreated: (chat: ChatSummary) => void;
  readonly workspace?: WorkspaceStore;
  readonly worktreeApi?: WorktreeChatApi;
  readonly recoveryApi?: WorktreeRecoveryApi;
  /** Stops the chats working in the chosen folder ("Stop these chats"). */
  readonly folderBusy?: Pick<FolderBusyApi, "stop">;
}) {
  const state = useSyncExternalStore(draft.subscribe, draft.getSnapshot, draft.getSnapshot);
  const organization = useSyncExternalStore(
    workspace?.subscribe ?? noSubscribe,
    workspace?.getSnapshot ?? emptyWorkspace,
    workspace?.getSnapshot ?? emptyWorkspace,
  );
  const group = useId();
  // Null keeps automatic project prefill separate from an explicit choice, including clearing it.
  const [folderId, setFolderId] = useState<string | null>(null);
  const [agentQuery, setAgentQuery] = useState("");
  const [retained, setRetained] = useState<ManagedWorktree[]>([]);
  const [retainedLoaded, setRetainedLoaded] = useState(!workspace);
  const [recoveryRevision, setRecoveryRevision] = useState(0);
  const [recoveryError, setRecoveryError] = useState<string | null>(null);
  useEffect(() => {
    void recoveryRevision;
    if (!workspace) return;
    setRetainedLoaded(false);
    let current = true;
    void recoveryApi.list(null).then(
      (records) => {
        if (current) {
          setRetained(records);
          setRetainedLoaded(true);
          setRecoveryError(null);
        }
      },
      (error) => {
        // A failed list says so and stops waiting: Rust still refuses a managed worktree.
        if (current) {
          setRecoveryError(errorMessage(error));
          setRetainedLoaded(true);
        }
      },
    );
    return () => {
      current = false;
    };
  }, [workspace, recoveryApi, recoveryRevision]);
  const [preview, setPreview] = useState<ChatCreatePreview | null>(null);
  const [allowBusy, setAllowBusy] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  // Read again once the chats working in the folder were stopped.
  const [previewRevision, setPreviewRevision] = useState(0);
  const folderChoice = useMemo(() => new WorktreeChoice(worktreeApi), [worktreeApi]);
  const worktree = useSyncExternalStore(
    folderChoice.subscribe,
    folderChoice.getSnapshot,
    folderChoice.getSnapshot,
  );
  const openChip = useRef<(() => void) | null>(null);
  const adding = useRef<string | null>(null);
  // This form's own work: the picked folder joining the workspace, and Create. Nothing another
  // screen runs disables it; one Create at a time is enforced here, and Rust validates each.
  const [addingFolder, setAddingFolder] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const submitted = useRef(false);
  const [createError, setCreateError] = useState<string | null>(null);
  // What Rust found the picked folder to be (the project folders it already is, however it was
  // spelled, or a managed worktree), for the path it was asked about.
  const [match, setMatch] = useState<{
    readonly path: string;
    readonly found: FolderMatches;
  } | null>(null);
  useEffect(() => {
    const path = state.folder;
    if (!workspace || !path) return;
    let current = true;
    void workspace.api.folderMatches(path).then(
      (found) => {
        if (current) setMatch({ path, found });
      },
      () => {
        // Unreadable here: adding the folder says why, in Rust's words.
        if (current) setMatch({ path, found: { path, matches: [], worktree: null } });
      },
    );
    return () => {
      current = false;
    };
  }, [workspace, state.folder]);
  // Agents whose detection finished while this screen was open settle in once; first paint does not.
  const seen = useRef<ReadonlyMap<string, string> | "waiting" | null>(null);
  const [found, setFound] = useState<ReadonlySet<string>>(() => new Set());
  useLayoutEffect(() => {
    if (agents === null) {
      if (seen.current === null) seen.current = "waiting";
      return;
    }
    const before = seen.current;
    const settled = agents
      .filter(
        (agent) =>
          agent.availability.state === "ready" &&
          (before === "waiting" || (before !== null && before.get(agent.id) === "checking")),
      )
      .map((agent) => agent.id);
    seen.current = new Map(agents.map((agent) => [agent.id, agent.availability.state]));
    if (settled.length > 0) setFound((current) => new Set([...current, ...settled]));
  }, [agents]);
  // The project folder this chat would use: chosen under Projects, or the picked folder's own. A
  // worktree is never offered: a chat starts from the folder it was made from.
  const sourceFolder = (folder: { path: string | null; worktree_of?: string | null }) =>
    !folder.worktree_of &&
    (!folder.path || !retained.some((record) => sameFolder(record.path, folder.path as string)));
  // Rust's answer for the folder picked now: the project folder it already is (the draft's
  // project first, then the one used last), so another spelling of it never makes a new project.
  const answer = match && match.path === state.folder ? match.found : null;
  const matched = chosenMatch(answer, draft.context?.projectId ?? null);
  // The draft's project (every project without one), and the project a picked folder is in.
  const shownProject = (project: { readonly id: string }) =>
    !draft.context || project.id === draft.context.projectId || project.id === matched?.project_id;
  const folders =
    organization.data?.projects
      .filter(shownProject)
      .flatMap((project) => project.folders)
      .filter(sourceFolder) ?? [];
  const picked = state.folder ? folders.find((folder) => folder.path === state.folder) : undefined;
  const automaticFolder = projectSourceFolder(draft.context, organization.data, retained);
  const requestedFolder = folderId ?? (state.folder ? "" : automaticFolder) ?? "";
  const knownFolder =
    (folders.some((folder) => folder.id === requestedFolder) ? requestedFolder : "") ||
    matched?.folder_id ||
    picked?.id ||
    "";
  const retainedPick =
    !knownFolder && state.folder
      ? retained.find((record) => sameFolder(record.path, state.folder as string))
      : undefined;
  const managedPicked =
    !knownFolder && state.folder ? (answer?.worktree ?? retainedPick) : undefined;
  // A picked folder waits for Rust's answer before it is used or added, so it never joins twice.
  const pickPending = Boolean(workspace && state.folder && !folderId && !answer);
  const loaded = organization.data !== null;
  // A picked folder in no project joins the workspace by itself (the effect below). Create waits
  // for exactly that, from Rust's answer until the folder is in the workspace (and while the
  // workspace still loads, before the effect can add it), so a click never adds the folder a
  // second time or reviews one still joining. A failed add ends the wait: Create then tries
  // itself and says why if it still cannot.
  const joining = Boolean(
    workspace &&
      state.folder &&
      !folderId &&
      answer &&
      answer.matches.length === 0 &&
      !managedPicked &&
      !picked &&
      adding.current !== state.folder,
  );
  useEffect(() => {
    void previewRevision;
    let current = true;
    setAllowBusy(false);
    setPreview(null);
    setPreviewError(null);
    if (knownFolder && workspace)
      void workspace.api.preview(knownFolder).then(
        (value) => {
          if (current) setPreview(value);
        },
        (error) => {
          if (current) setPreviewError(errorMessage(error));
        },
      );
    return () => {
      current = false;
    };
  }, [knownFolder, workspace, previewRevision]);
  // A picked folder joins the workspace as its own project at once (as a project is added in the
  // Codex app), so its Git state is reviewed before Create. A failure leaves it to Create.
  useEffect(() => {
    const path = state.folder;
    if (
      !workspace ||
      !path ||
      folderId ||
      !loaded ||
      !answer ||
      answer.matches.length > 0 ||
      managedPicked ||
      picked ||
      adding.current === path
    )
      return;
    adding.current = path;
    setAddingFolder(true);
    void workspace
      .perform(async () => {
        if (draft.context) return workspace.api.addFolder(draft.context.projectId, path);
        const project = await workspace.api.createProject({
          project: {
            name: projectName(path),
            color: null,
            default_agent: draft.getSnapshot().agentId,
            default_model: null,
          },
          folder: path,
        });
        return project.folders[0] ?? null;
      })
      .then((outcome) => {
        if (adding.current === path) setAddingFolder(false);
        if (!outcome.ok) {
          // Create adds the folder itself then, and says why if it still cannot.
          if (draft.getSnapshot().folder === path) setCreateError(outcome.error);
          return;
        }
        const id = outcome.value?.id;
        if (id && draft.getSnapshot().folder === path) setFolderId(id);
      });
  }, [workspace, state.folder, folderId, loaded, answer, managedPicked, picked, draft]);
  // The one lineup every agent list reads, so "Other agents" counts the same everywhere.
  const { usable: available, others: unavailable } = agentLineup(agents ?? []);
  const selected = available.find((agent) => agent.id === state.agentId) ?? null;
  // Chat or Terminal for the new chat, starting from this device's default (Settings > Chats).
  const [interfaceChoice, setInterfaceChoice] = useState<ChatInterface>(() =>
    chatInterfaces.defaultInterface(),
  );
  const terminalOffered = selected !== null && terminalSupported(selected.id, agents);
  const terminalMode = terminalOffered && interfaceChoice === "terminal";
  // Type first: what the person writes before choosing goes to the chat this makes.
  const [message, setMessage] = useState("");
  const messageBox = useRef<HTMLTextAreaElement>(null);
  const hasMessage = !terminalMode && message.trim() !== "";
  // The box takes the keyboard as the page opens, so typing starts at once.
  useEffect(() => {
    messageBox.current?.focus({ preventScroll: true });
  }, []);
  // Every way a chat is made ends here: it opens as chosen, Chat for agents without a terminal,
  // and a chat that opens as Chat sends the words it was started with (its view sends them, and
  // keeps them in its message box if they cannot go).
  const created = (chat: ChatSummary) => {
    const opensIn = terminalSupported(chat.agent_id, agents) ? interfaceChoice : "chat";
    chatInterfaces.set(chat.id, opensIn);
    if (opensIn === "chat") holdFirstMessage(chat.id, message);
    onCreated(chat);
  };
  // Rust reviews the worktree for the chosen agent, or the first one that can run until one is.
  const reviewAgent = selected?.id ?? available[0]?.id ?? null;
  useEffect(() => {
    folderChoice.target(workspace && knownFolder ? knownFolder : null, reviewAgent);
  }, [folderChoice, workspace, knownFolder, reviewAgent]);
  // Leaving New chat drops every review on its way.
  useEffect(() => () => folderChoice.target(null, null), [folderChoice]);
  const worktreeMode = worktree.kind === "git" && worktree.options.mode === "worktree";
  // Rust persisted setup admission before exposing the chat. Its normal composer remains usable.
  const madeWorktree = (result: WorktreeChatResult, _plan: WorktreeChatPlan) => {
    created(result.chat.chat);
  };
  const choiceState = (agent: AgentEntry): ChoiceState =>
    agent.availability.state === "checking"
      ? "detecting"
      : agent.id === state.agentId && state.auth?.state === "sign_in_required"
        ? "sign-in"
        : "ready";
  const note = selected ? downloadNote(selected) : null;
  const ready = selected ? readyLine(selected) : null;
  // A worktree chat never shares the project folder, so only Local waits on its running chats.
  const busy = !worktreeMode && preview !== null && preview.busy_chats.length > 0;
  const projectOfChat = (chatId: string) => {
    const saved = organization.data?.chats.find((entry) => entry.chat.id === chatId);
    return (
      organization.data?.projects.find((project) => project.id === saved?.project_id)?.name ?? null
    );
  };
  // The worktree chip's "Stop these chats": stop the chats working in the folder, then look again.
  const stopBusy = () => {
    if (!preview || preview.busy_chats.length === 0) return;
    folderBusy.stop(preview.busy_chats.map((chat) => chat.id)).then(
      () => setPreviewRevision((value) => value + 1),
      (error: unknown) => setPreviewError(errorMessage(error)),
    );
  };
  // Nothing waits for the retained list: Rust marks worktree folders in the workspace, and says
  // whether a picked folder is one.
  const canCreate =
    selected !== null &&
    !pickPending &&
    !joining &&
    !managedPicked &&
    (state.folder !== null || knownFolder !== "") &&
    !addingFolder &&
    !submitting &&
    !worktree.creating &&
    !state.creating &&
    state.maintenanceBusy !== "update";
  const chosenFolder =
    preview?.folder.path ??
    folders.find((folder) => folder.id === knownFolder)?.path ??
    state.folder;
  const projects = workspace
    ? (organization.data?.projects ?? [])
        .filter(shownProject)
        .map((project) => ({ ...project, folders: project.folders.filter(sourceFolder) }))
        .filter((project) => project.folders.length > 0)
    : [];
  const selectedProject = projects.find((project) =>
    project.folders.some((folder) => folder.id === knownFolder),
  );
  const recoveries = worktreesToReview(
    retainedLoaded ? retained : [],
    organization.data ? new Set(organization.data.chats.map((entry) => entry.chat.id)) : null,
    (record) =>
      !selectedProject ||
      selectedProject.folders.some(
        (folder) => folder.path !== null && sameFolder(folder.path, record.source),
      ),
  );
  const selectedState = selected ? choiceState(selected) : null;
  const starting = state.creating || worktree.creating || submitting;
  const startLabel = hasMessage
    ? "Start chat and send"
    : terminalMode
      ? "Open terminal"
      : "Start chat";
  return (
    <form
      className="new-chat"
      aria-label="New chat"
      data-testid="new-chat"
      onSubmit={(event) => {
        event.preventDefault();
        if (!canCreate || (busy && !allowBusy) || submitted.current) return;
        if (!workspace) {
          void draft.create().then((chat) => {
            if (chat) created(chat);
          });
          return;
        }
        const agentId = state.agentId;
        submitted.current = true;
        setSubmitting(true);
        setCreateError(null);
        void workspace
          .perform(async () => {
            let destination = knownFolder;
            let added = false;
            if (!destination && state.folder) {
              if (draft.context) {
                const folder = await workspace.api.addFolder(draft.context.projectId, state.folder);
                destination = folder.id;
              } else {
                const project = await workspace.api.createProject({
                  project: {
                    name: projectName(state.folder),
                    color: null,
                    default_agent: agentId,
                    default_model: null,
                  },
                  folder: state.folder,
                });
                destination = project.folders[0]?.id ?? "";
              }
              added = true;
              setFolderId(destination);
            }
            if (!destination || !agentId) return null;
            // The latest review decides: a worktree (zero clicks by default) or the folder itself.
            folderChoice.target(destination, agentId);
            const decision = await folderChoice.resolve();
            if (decision.type === "stop") {
              if (folderChoice.getSnapshot().error) openChip.current?.();
              return null;
            }
            if (decision.type === "worktree") {
              // A folder first reviewed by this Create shows its worktree before making one.
              if (added) return null;
              const result = await folderChoice.create(decision.plan);
              return { type: "worktree" as const, result, plan: decision.plan };
            }
            const checked = await workspace.api.preview(destination);
            setPreview(checked);
            if (checked.busy_chats.length > 0 && !allowBusy) return null;
            const entry = await workspace.api.createChat({
              folder_id: destination,
              agent_id: agentId,
              allow_busy_folder: allowBusy,
            });
            return { type: "local" as const, entry };
          })
          .then((outcome) => {
            submitted.current = false;
            setSubmitting(false);
            if (!outcome.ok) {
              setCreateError(outcome.error);
              return;
            }
            const made = outcome.value;
            if (made?.type === "local") created(made.entry.chat);
            else if (made?.type === "worktree") madeWorktree(made.result, made.plan);
          });
      }}
    >
      {/*
       * The wing, alive (it blinks and watches the pointer), over one line in two tones: "New
       * chat" in ink and where it starts in grey (owner, 26 November 2026: "wayyyy sexier, wayyyy
       * sleeker"). The message box's own words say what to do, so no sentence repeats them.
       */}
      <header className="new-chat-hero">
        <div className="new-chat-wing">
          <Wing look />
        </div>
        <h1 className="new-chat-title">
          <Words text="New chat" />
          {draft.context ? (
            <span className="new-chat-tone">
              {" "}
              <Words text={`in ${draft.context.projectName}`} from={2} />
            </span>
          ) : null}
        </h1>
      </header>
      {/*
       * Type first (owner, 17 November 2026): the message box leads, as the Grok Bot app's new
       * chat does, with the agent, the folder and Chat or Terminal as chips under the words.
       */}
      <div className="new-chat-card" data-layout="type-first">
        {terminalMode ? (
          <p className="new-chat-terminal-note" data-testid="new-chat-terminal-note">
            {selected?.name ?? "The agent"} opens in its own terminal here: type to it there once it
            starts.
          </p>
        ) : (
          <textarea
            ref={messageBox}
            className="new-chat-message"
            data-testid="new-chat-message"
            data-tour="first-message"
            aria-label="First message"
            placeholder={selected ? `Ask ${selected.name} to…` : "What should the agent do?"}
            rows={3}
            value={message}
            onChange={(event) => setMessage(event.currentTarget.value)}
            onKeyDown={(event) => {
              // Enter sends, as in a chat; Shift+Enter starts a new line.
              if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return;
              event.preventDefault();
              event.currentTarget.form?.requestSubmit();
            }}
          />
        )}
        <div className="new-chat-controls">
          <div className="new-chat-chips">
            <div className="new-chat-agents" data-testid="new-chat-agent" data-tour="agents">
              {agents === null ? (
                <span className="new-chat-scan" role="status">
                  <span className="new-chat-scan-pulse" aria-hidden="true">
                    <span className="ui-pulse" />
                  </span>
                  Looking for agents
                </span>
              ) : available.length === 0 ? (
                <span className="new-chat-none">No agent can run here yet.</span>
              ) : (
                <Popover
                  label="Choose agent"
                  triggerLabel={`${selected?.name ?? "Select an agent"}. Choose agent`}
                  className="new-chat-agent-picker"
                  trigger={
                    <>
                      {selected ? <AgentMark agent={selected.id} size={20} /> : null}
                      <span className="new-chat-selected-name">
                        {selected?.name ?? "Select an agent"}
                      </span>
                      <ChevronDown size={14} />
                    </>
                  }
                  onOpenChange={(open) => {
                    if (!open) setAgentQuery("");
                  }}
                >
                  <label className="new-chat-agent-search">
                    <Search size={14} aria-hidden="true" />
                    <input
                      className="g-input"
                      type="search"
                      aria-label="Search agents"
                      placeholder="Search agents…"
                      value={agentQuery}
                      onChange={(event) => setAgentQuery(event.currentTarget.value)}
                    />
                  </label>
                  <fieldset className="new-chat-choices">
                    <legend className="sr-only">Agent</legend>
                    {available
                      .filter((agent) =>
                        agent.name.toLowerCase().includes(agentQuery.toLowerCase()),
                      )
                      .map((agent) => {
                        const checked = agent.id === state.agentId;
                        const choice = choiceState(agent);
                        return (
                          <label
                            key={agent.id}
                            className="new-chat-agent"
                            data-state={choice}
                            data-found={found.has(agent.id) ? "" : undefined}
                          >
                            <input
                              type="radio"
                              name={group}
                              value={agent.id}
                              checked={checked}
                              aria-label={`${agent.name}, ${STATE_LABEL[choice]}`}
                              onChange={(event) => {
                                draft.chooseAgent(agent.id);
                                const popup = event.currentTarget.closest("details");
                                closePopover(event.currentTarget);
                                popup?.querySelector("summary")?.focus();
                              }}
                            />
                            {/* Still looked for: the mark waits dimmed inside a soft circular pulse. */}
                            <span className="new-chat-agent-mark">
                              <AgentMark
                                agent={agent.id}
                                size={24}
                                state={choice === "sign-in" ? "waiting" : "idle"}
                              />
                            </span>
                            <span className="new-chat-agent-name">{agent.name}</span>
                            <small>{STATE_LABEL[choice]}</small>
                            {checked ? <Check size={14} aria-hidden="true" /> : null}
                          </label>
                        );
                      })}
                  </fieldset>
                  {!available.some((agent) =>
                    agent.name.toLowerCase().includes(agentQuery.toLowerCase()),
                  ) ? (
                    <p className="chat-picker-empty">No matching agents.</p>
                  ) : null}
                </Popover>
              )}
            </div>
            {projects.length > 0 ? (
              <Select
                className="g-select new-chat-projects"
                aria-label="Project folder"
                value={knownFolder}
                onChange={(event) => {
                  setFolderId(event.currentTarget.value);
                  const project = organization.data?.projects.find((item) =>
                    item.folders.some((folder) => folder.id === event.currentTarget.value),
                  );
                  // The project's agent fills an empty choice only: an agent already picked here
                  // stays picked when the folder changes.
                  if (project?.default_agent && draft.getSnapshot().agentId === null)
                    draft.chooseAgent(project.default_agent);
                }}
              >
                <option value="">{draft.context ? "Select folder" : "Projects"}</option>
                {projects.map((project) => (
                  <optgroup key={project.id} label={project.name}>
                    {project.folders.map((folder) => (
                      <option
                        key={folder.id}
                        value={folder.id}
                        disabled={folder.folder_state !== "ok"}
                      >
                        {folder.display_name}
                        {folder.folder_state !== "ok" ? " (unavailable on this device)" : ""}
                      </option>
                    ))}
                  </optgroup>
                ))}
              </Select>
            ) : null}
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28 new-chat-pick"
              data-testid="new-chat-pick-folder"
              aria-label={state.picking ? "Choosing…" : "Choose folder"}
              title="Choose folder"
              disabled={state.picking}
              onClick={() => {
                void draft.chooseFolder().then((folder) => {
                  if (folder !== null) setFolderId("");
                });
              }}
            >
              <FolderOpen size={14} aria-hidden="true" />
            </button>
            {terminalOffered ? (
              <span className="new-chat-interface" data-testid="new-chat-interface">
                <fieldset className="g-segmented new-chat-interface-choice" aria-label="Open in">
                  {(["chat", "terminal"] as const).map((choice) => (
                    <button
                      key={choice}
                      type="button"
                      className="g-segment"
                      aria-pressed={interfaceChoice === choice}
                      data-test-interface={choice}
                      onClick={() => setInterfaceChoice(choice)}
                    >
                      {choice === "chat" ? "Chat" : "Terminal"}
                    </button>
                  ))}
                </fieldset>
              </span>
            ) : null}
          </div>
          <div className="new-chat-send">
            <button
              type="button"
              className="g-button g-button-ghost g-button-icon g-button-28 new-chat-refresh"
              data-testid="new-chat-refresh"
              aria-label={refreshing ? "Refreshing…" : "Refresh agents"}
              title="Refresh agents"
              disabled={refreshing}
              onClick={onRefreshAgents}
            >
              {refreshing ? (
                <PixelLoader size={12} tempo="thinking" />
              ) : (
                <RefreshCw size={14} aria-hidden="true" />
              )}
            </button>
            <button
              type="submit"
              className="g-button g-button-primary new-chat-start"
              data-testid="new-chat-create"
              data-with-message={hasMessage ? "" : undefined}
              disabled={!canCreate || (busy && !allowBusy)}
              aria-label={startLabel}
              title={hasMessage ? `${startLabel} (Enter)` : undefined}
            >
              {starting ? (
                "Starting…"
              ) : hasMessage ? (
                <ArrowUp size={16} strokeWidth={2.25} aria-hidden="true" />
              ) : terminalMode ? (
                "Open terminal"
              ) : (
                "Start chat"
              )}
            </button>
          </div>
        </div>
      </div>
      {/*
       * Under the card, one quiet line: the folder by its name (the whole path in its tooltip) and
       * where the chat works in it, then at the right the agent's facts and the agents that cannot
       * run here, each a popover. Consent, warnings and errors follow under the line.
       */}
      <div className="new-chat-context">
        <p className="new-chat-where">
          <Folder className="new-chat-folder-icon" size={14} aria-hidden="true" />
          <span
            className="new-chat-path"
            data-testid="new-chat-folder"
            data-test-folder={state.folder ?? ""}
            data-empty={chosenFolder ? undefined : ""}
            title={chosenFolder ?? undefined}
          >
            {chosenFolder ? projectName(chosenFolder) : "No folder chosen"}
          </span>
          {worktree.kind === "none" || worktree.kind === "plain" ? (
            <span className="new-chat-where-mode">
              {chosenFolder
                ? worktreeMode
                  ? "Starts in a separate worktree"
                  : "Starts in this folder"
                : "Select a folder to continue"}
            </span>
          ) : null}
        </p>
        <WorktreeChip
          view={worktree}
          busyChats={preview?.busy_chats.length ?? 0}
          onStopBusy={stopBusy}
          openRef={openChip}
          onPropose={(change, field) => folderChoice.propose(change, field)}
          onClearError={() => folderChoice.clearError()}
        />
        {selected || unavailable.length > 0 ? (
          <span className="new-chat-facts">
            {selected ? (
              <Popover
                label={`About ${selected.name}`}
                triggerLabel={`${factLine(selected)}. Details`}
                className="new-chat-more"
                trigger={
                  <>
                    <span className="new-chat-more-line" data-state={selectedState ?? undefined}>
                      {factLine(selected)}
                    </span>
                    <ChevronDown size={12} aria-hidden="true" />
                  </>
                }
              >
                <div className="new-chat-more-body">
                  {ready ? <p className="new-chat-more-ready">{ready}</p> : null}
                  {selected.description ? <p>{selected.description}</p> : null}
                  {selected.installation ? (
                    <InstallationStatus installation={selected.installation} />
                  ) : null}
                  {selected.latest_release?.version ? (
                    <small className="ui-muted">
                      Latest on {selected.latest_release.source}: {selected.latest_release.version}
                      {selected.latest_release.stale ? " (cached; refresh unavailable)" : null}
                    </small>
                  ) : null}
                  {selected.installation ? (
                    <section className="new-chat-maintenance" aria-label="Agent maintenance">
                      {state.maintenanceBusy === "checking" ? (
                        <small className="ui-muted">Checking available actions…</small>
                      ) : null}
                      {state.maintenance?.auth_status ||
                      state.maintenance?.update_unavailable === null ? (
                        <div className="new-chat-actions">
                          {state.maintenance?.auth_status ? (
                            <button
                              type="button"
                              className="g-button g-button-outline g-button-28"
                              disabled={state.maintenanceBusy !== null}
                              onClick={() => void draft.checkAuth()}
                            >
                              {state.maintenanceBusy === "auth"
                                ? "Checking sign-in…"
                                : "Check sign-in"}
                            </button>
                          ) : null}
                          {state.maintenance?.update_unavailable === null ? (
                            <button
                              type="button"
                              className="g-button g-button-outline g-button-28"
                              disabled={state.maintenanceBusy !== null || state.creating}
                              onClick={() => void draft.updateAgent().then(() => onRefreshAgents())}
                            >
                              {state.maintenanceBusy === "update"
                                ? "Updating agent…"
                                : "Update agent"}
                            </button>
                          ) : null}
                        </div>
                      ) : null}
                      {state.maintenance?.update_unavailable ? (
                        <small className="ui-muted">{state.maintenance.update_unavailable}</small>
                      ) : null}
                      {state.auth ? (
                        <span
                          role="status"
                          className="new-chat-auth"
                          data-state={
                            state.auth.state === "sign_in_required" ? "sign-in" : undefined
                          }
                        >
                          {authStatusText(state.auth)}
                        </span>
                      ) : null}
                      {state.maintenanceMessage ? (
                        <span role="status">{state.maintenanceMessage}</span>
                      ) : null}
                      {state.maintenanceError ? (
                        <span role="alert" className="ui-error">
                          {state.maintenanceError}
                        </span>
                      ) : null}
                    </section>
                  ) : null}
                </div>
              </Popover>
            ) : null}
            {unavailable.length > 0 ? (
              <Popover
                label="Other agents"
                triggerLabel={`Other agents, ${unavailable.length}`}
                className="new-chat-more"
                trigger={
                  <>
                    <span className="new-chat-more-label">Other agents</span>
                    <span className="new-chat-more-count">{unavailable.length}</span>
                    <ChevronDown size={12} aria-hidden="true" />
                  </>
                }
              >
                <ul className="new-chat-unavailable">
                  {unavailable.map((agent) => (
                    <li key={agent.id} data-test-unavailable={agent.id}>
                      <AgentMark
                        agent={agent.id}
                        size={24}
                        ghost={agent.availability.state === "missing"}
                        state={agent.availability.state === "broken" ? "error" : "idle"}
                        label={agent.availability.state === "broken" ? "Failed" : "Not available"}
                      />
                      <span>
                        <strong>{agent.name}</strong>
                        <span>{agentChoice(agent).reason}</span>
                      </span>
                    </li>
                  ))}
                </ul>
              </Popover>
            ) : null}
          </span>
        ) : null}
      </div>
      <div className="new-chat-below">
        {managedPicked ? (
          <p className="ui-error" role="alert">
            This folder is a managed worktree on {managedPicked.branch}. Choose its original project
            folder: {managedPicked.source}.
          </p>
        ) : null}
        {matched &&
        knownFolder === matched.folder_id &&
        matched.project_id !== draft.context?.projectId ? (
          <p className="new-chat-note" data-testid="new-chat-folder-project">
            This folder is in {matched.project_name}, so the chat starts there.
          </p>
        ) : null}
        {recoveryError ? (
          <p role="alert" className="ui-error">
            Could not load retained worktrees: {recoveryError}
          </p>
        ) : null}
        {recoveries.length > 0 ? (
          // One quiet line, as Other agents is; it opens by itself only when an operation stopped
          // half way and needs a look.
          <details
            className="new-chat-more"
            data-testid="new-chat-retained"
            open={recoveries.some(needsRecovery) || undefined}
          >
            <summary>
              <span className="new-chat-more-label">Retained worktrees</span>
              <span className="new-chat-more-count">{recoveries.length}</span>
            </summary>
            <div className="new-chat-more-body">
              <p>
                Folders kept from chats that are gone, or whose last change needs a review. Their
                files and branches are untouched.
              </p>
              <WorktreeRecoveryList
                records={recoveries}
                api={recoveryApi}
                heading={false}
                onChanged={() => setRecoveryRevision((value) => value + 1)}
              />
            </div>
          </details>
        ) : null}
        {preview && busy ? (
          <FolderBusy
            className="new-chat-busy"
            heading="This folder has active work"
            blockers={blockersFromChats(preview.busy_chats, projectOfChat)}
            api={folderBusy}
            onStopped={() => setPreviewRevision((value) => value + 1)}
          >
            <label>
              <input
                type="checkbox"
                checked={allowBusy}
                onChange={(event) => setAllowBusy(event.currentTarget.checked)}
              />
              Start another chat in this folder. Agents may edit the same files.
            </label>
          </FolderBusy>
        ) : null}
        {note !== null ? <p className="new-chat-note">{note}</p> : null}
        {(previewError ?? createError ?? organization.error) ? (
          <p role="alert" className="new-chat-error">
            {previewError ?? createError ?? organization.error}
          </p>
        ) : null}
        {state.error !== null ? (
          <p role="alert" className="new-chat-error" data-testid="new-chat-error">
            {state.error}
          </p>
        ) : null}
      </div>
    </form>
  );
}
