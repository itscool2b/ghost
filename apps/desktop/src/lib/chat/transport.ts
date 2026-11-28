import type {
  AgentAuthStatus,
  AgentEntry,
  AgentMaintenance,
  Autonomy,
  BlobText,
  ChatAttach,
  ChatStatusUpdate,
  ChatSummary,
  ConfigValue,
  EnvStatus,
  EventBatch,
  HistoryPage,
  PromptAck,
  QueuedPrompt,
  SessionControlChange,
  SessionLists,
  StorageStatus,
  UiAnswer,
} from "../../bindings";
import { type AgentAccountsApi, agentAccountsApi } from "../agentAccounts";
import { type BrowserStore, browserStore } from "../browser";
import { changesApi } from "../changes";
import { type ChatImagesApi, chatImagesApi } from "../chatImages";
import { type ComposerTransport, composerApi } from "../composer";
import { type ConversationTransport, conversationApi } from "../conversation";
import { type GitWorkspaceApi, gitWorkspaceApi } from "../gitWorkspace";
import {
  agentAuthStatus,
  agentMaintenance,
  agentSetConnection,
  agentsList,
  agentsRefresh,
  agentUpdate,
  agentUpdateOutput,
  chatAcceptPlan,
  chatAttach,
  chatBlob,
  chatCancelQueued,
  chatCreate,
  chatDeclinePlan,
  chatDetach,
  chatEditQueued,
  chatForceStop,
  chatHistory,
  chatInterrupt,
  chatLists,
  chatMoveQueued,
  chatQueuedText,
  chatRespondPermission,
  chatRespondUi,
  chatResumeQueue,
  chatRevealLog,
  chatSend,
  chatSendNow,
  chatSetAutonomy,
  chatSetControl,
  chatSetOption,
  chatStart,
  chatStop,
  chatsList,
  envRefresh,
  envStatus,
  pickFolder,
  statusSubscribe,
  storageStatus,
} from "../ipc";
import { type MobileStore, mobileStore } from "../mobile";
import { type ProjectToolsApi, projectToolsApi } from "../projectTools";
import { type SearchTransport, searchApi } from "../search";
import { type TerminalTransport, updateTerminalTransport } from "../terminal";
import { type WorktreeRunApi, worktreeRunApi } from "../worktreeRun";
import { type WorktreeSetupApi, worktreeSetupApi } from "../worktreeSetup";
import { type FileLinkApi, fileLinkApi } from "./fileLinks";

/**
 * Everything the chat screens ask of the shell. The stores take this interface, so Vitest drives
 * them with a fake (`testing.ts`); `tauriTransport` is the real one over `lib/ipc.ts`, whose
 * wrappers throw `CommandError`s with the shell's codes.
 */
export interface Transport {
  readonly setup?: WorktreeSetupApi;
  readonly git?: GitWorkspaceApi;
  readonly conversation?: ConversationTransport;
  readonly search?: SearchTransport;
  readonly changes?: import("../changes").ChangesTransport;
  readonly composer?: ComposerTransport;
  readonly images?: ChatImagesApi;
  /** Reviewed project shells and editors for a saved chat's folder (the docked terminal panel). */
  readonly projectTools?: ProjectToolsApi;
  /** Added agent accounts, for continuing a chat past a usage limit on another one. */
  readonly accounts?: AgentAccountsApi;
  /** A chat's file names, checked against its folder and opened in the person's editor. */
  readonly files?: FileLinkApi;
  /** Run in a worktree chat: the folder setup's Run command, with the worktree's ports. */
  readonly run?: WorktreeRunApi;
  /** The browser beside the chat (the Browser plugin). Absent: no browser is offered. */
  readonly browser?: BrowserStore;
  /** The device beside the chat (the Mobile plugin). Absent: no device is offered. */
  readonly mobile?: MobileStore;
  agentMaintenance(agentId: string): Promise<AgentMaintenance>;
  agentAuthStatus(agentId: string): Promise<AgentAuthStatus>;
  agentUpdate(agentId: string): Promise<void>;
  /** What the agent's last update printed, when it printed anything. Read after an update ends. */
  agentUpdateOutput?(agentId: string): Promise<string | null>;
  /** An agent's update command in the terminal sheet, after Update failed in the background. */
  readonly updateTerminal?: TerminalTransport;
  /**
   * An agent's Connection setting on this device: one of its connection ids, or `null` for
   * Automatic. Answers every agent as `agentsList` does.
   */
  agentSetConnection(agentId: string, connection: string | null): Promise<AgentEntry[]>;
  agentsList(): Promise<AgentEntry[]>;
  agentsRefresh(): Promise<AgentEntry[]>;
  envStatus(): Promise<EnvStatus>;
  envRefresh(): Promise<EnvStatus>;
  pickFolder(): Promise<string | null>;
  chatsList(): Promise<ChatSummary[]>;
  chatCreate(agentId: string, folder: string): Promise<ChatSummary>;
  statusSubscribe(onUpdate: (update: ChatStatusUpdate) => void, onEnd: () => void): Promise<string>;
  storageStatus(): Promise<StorageStatus>;

  /** The join protocol's first half: frames reach `onFrame` from before this resolves on. */
  attach(
    chatId: string,
    onFrame: (frame: EventBatch) => void,
    onEnd: () => void,
  ): Promise<ChatAttach>;
  detach(subscriptionId: string): Promise<void>;
  history(chatId: string, beforeSeq: number, maxBytes?: number): Promise<HistoryPage>;
  lists(chatId: string): Promise<SessionLists>;
  send(chatId: string, text: string): Promise<PromptAck>;
  cancelQueued(chatId: string, promptId: string): Promise<void>;
  /** The words of a queued prompt as typed. */
  queuedText(chatId: string, promptId: string): Promise<string>;
  /** Replaces a queued prompt's words; answers the queue after the change. */
  editQueued(chatId: string, promptId: string, text: string): Promise<QueuedPrompt[]>;
  /** Moves a queued prompt to `index` (0 goes next); answers the queue after the change. */
  moveQueued(chatId: string, promptId: string, index: number): Promise<QueuedPrompt[]>;
  resumeQueue(chatId: string): Promise<void>;
  /** "Send now": the queued prompt goes next; answers the queue after the change. */
  sendNow(chatId: string, promptId: string): Promise<QueuedPrompt[]>;
  interrupt(chatId: string): Promise<void>;
  forceStop(chatId: string): Promise<void>;
  start(chatId: string, fresh: boolean): Promise<void>;
  stop(chatId: string): Promise<void>;
  respondPermission(chatId: string, requestId: string, optionId: string | null): Promise<void>;
  /**
   * Accepts a plan waiting for approval as the person edited it. Offered only where the agent
   * takes one (`capabilities.plan_edits`); the core refuses it otherwise.
   */
  acceptPlan?(chatId: string, requestId: string, optionId: string, plan: string): Promise<void>;
  /**
   * Declines a plan waiting for approval with the person's words, which the agent reads with
   * the refusal. Offered only where the agent takes them (`capabilities.plan_notes`); the core
   * refuses it otherwise.
   */
  declinePlan?(chatId: string, requestId: string, optionId: string, note: string): Promise<void>;
  respondUi(chatId: string, requestId: string, answer: UiAnswer | null): Promise<void>;
  setOption(chatId: string, optionId: string, value: ConfigValue): Promise<SessionLists>;
  setControl?(chatId: string, change: SessionControlChange): Promise<SessionLists>;
  setAutonomy(chatId: string, autonomy: Autonomy): Promise<SessionLists>;
  blob(chatId: string, sha256: string, offset: number, len: number): Promise<BlobText>;
  revealLog(chatId: string): Promise<void>;
}

/** The shell, through the typed command wrappers. */
export const tauriTransport: Transport = {
  browser: browserStore,
  mobile: mobileStore,
  git: gitWorkspaceApi,
  files: fileLinkApi,
  run: worktreeRunApi,
  conversation: conversationApi,
  search: searchApi,
  changes: changesApi,
  composer: composerApi,
  images: chatImagesApi,
  projectTools: projectToolsApi,
  setup: worktreeSetupApi,
  accounts: agentAccountsApi,
  agentMaintenance,
  agentAuthStatus,
  agentUpdate,
  agentUpdateOutput,
  updateTerminal: updateTerminalTransport,
  agentSetConnection,
  agentsList,
  agentsRefresh,
  envStatus,
  envRefresh,
  pickFolder,
  chatsList,
  chatCreate,
  statusSubscribe,
  storageStatus,
  attach: chatAttach,
  detach: chatDetach,
  history: (chatId, beforeSeq, maxBytes) => chatHistory(chatId, beforeSeq, maxBytes),
  lists: chatLists,
  send: chatSend,
  cancelQueued: chatCancelQueued,
  queuedText: chatQueuedText,
  editQueued: chatEditQueued,
  moveQueued: chatMoveQueued,
  resumeQueue: chatResumeQueue,
  sendNow: chatSendNow,
  interrupt: chatInterrupt,
  forceStop: chatForceStop,
  start: chatStart,
  stop: chatStop,
  respondPermission: chatRespondPermission,
  acceptPlan: chatAcceptPlan,
  declinePlan: chatDeclinePlan,
  respondUi: chatRespondUi,
  setOption: chatSetOption,
  setAutonomy: chatSetAutonomy,
  setControl: chatSetControl,
  blob: chatBlob,
  revealLog: chatRevealLog,
};
