import { Channel } from "@tauri-apps/api/core";
import {
  type AgentAuthStatus,
  type AgentEntry,
  type AgentMaintenance,
  type AppError,
  type AppErrorCode,
  type AppInfo,
  type Autonomy,
  type BlobText,
  type ChatAttach,
  type ChatStatusUpdate,
  type ChatSummary,
  type ChatTerminalStarted,
  type ConfigValue,
  commands,
  type DemoKind,
  type EnvStatus,
  type EventBatch,
  type FolderBlockers,
  type HistoryPage,
  type PromptAck,
  type QueuedPrompt,
  type SessionControlChange,
  type SessionLists,
  type SmokeReport,
  type StorageStatus,
  type TerminalChunk,
  type UiAnswer,
} from "../bindings";

/**
 * Thin typed wrappers over the generated bindings: results become values or thrown
 * `CommandError`s, and the stream channel becomes a callback. Components call these, never
 * `invoke` with string command names.
 */

/** `CommandError.code` when Tauri refused the call before any command ran. */
export const IPC_REJECTED = "ipc_rejected";

/** Shown when a rejection carries no message at all. */
const NO_REASON = "The app's shell refused the request without giving a reason.";

export class CommandError extends Error {
  /** The shell's stable code, or `ipc_rejected` for a call Tauri refused (see `toCommandError`). */
  readonly code: AppErrorCode | typeof IPC_REJECTED;
  /** What stands in the way of a folder change, for `folder_busy`. */
  readonly blockers: FolderBlockers | null;

  constructor(
    code: AppErrorCode | typeof IPC_REJECTED,
    message: string,
    blockers: FolderBlockers | null = null,
  ) {
    super(message);
    this.name = "CommandError";
    this.code = code;
    this.blockers = blockers;
  }
}

function isAppError(value: unknown): value is AppError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    typeof value.code === "string" &&
    "message" in value &&
    typeof value.message === "string"
  );
}

/**
 * The error to throw for a failed call. A command's own failure arrives as its `AppError`
 * (`{ code, message }`). When Tauri refuses a call before the command runs (arguments that do not
 * deserialize, a permission the window lacks, state that is not managed) it rejects with a plain
 * string instead. The generated bindings pass both on as "the error" typed `AppError`, so this
 * checks the shape rather than trusting the type, and keeps Tauri's message either way.
 */
export function toCommandError(rejection: unknown): CommandError {
  if (rejection instanceof CommandError) return rejection;
  if (isAppError(rejection)) {
    return new CommandError(
      rejection.code,
      rejection.message || NO_REASON,
      rejection.blockers ?? null,
    );
  }
  if (rejection instanceof Error) {
    return new CommandError(IPC_REJECTED, rejection.message || NO_REASON);
  }
  if (typeof rejection === "string" && rejection.trim() !== "") {
    return new CommandError(IPC_REJECTED, rejection);
  }
  return new CommandError(IPC_REJECTED, NO_REASON);
}

type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: AppError };

/** A fallible command: its value, or a thrown `CommandError`. */
export async function settle<T>(call: Promise<CommandResult<T>>): Promise<T> {
  let result: CommandResult<T>;
  try {
    result = await call;
  } catch (error) {
    // The bindings rethrow `Error`s (no IPC at all, for one) instead of returning them.
    throw toCommandError(error);
  }
  if (result.status === "error") throw toCommandError(result.error);
  return result.data;
}

/** A command that cannot fail by itself: Tauri can still refuse the call. */
async function infallible<T>(call: Promise<T>): Promise<T> {
  try {
    return await call;
  } catch (error) {
    throw toCommandError(error);
  }
}

/** Whether the page runs inside the Tauri shell (rather than a plain browser on the dev server). */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function getAppInfo(): Promise<AppInfo> {
  return infallible(commands.appInfo());
}

/**
 * Starts a demo stream; `onBatch` receives every frame, in order. Resolves with the session id
 * once the shell has accepted the stream (frames may arrive before that).
 */
export async function startDemoStream(
  kind: DemoKind,
  tokensPerSec: number,
  onBatch: (batch: EventBatch) => void,
): Promise<string> {
  // Inside the async function, so a missing IPC bridge rejects instead of throwing synchronously.
  const channel = new Channel<EventBatch>();
  channel.onmessage = onBatch;
  return settle(commands.startDemoStream(kind, Math.round(tokensPerSec), channel));
}

export function cancelDemoStream(sessionId: string): Promise<void> {
  return infallible(commands.cancelDemoStream(sessionId));
}

export async function windowReady(): Promise<void> {
  await settle(commands.windowReady());
}

export async function sendSmokeReport(report: SmokeReport): Promise<void> {
  await settle(commands.smokeReport(report));
}

/**
 * Shows the data folder selected in the system file manager. The shell decides which folder:
 * the page cannot name a path to reveal.
 */
export async function revealDataDir(): Promise<void> {
  await settle(commands.revealDataDir());
}

/**
 * Dismisses the database repair notice (`recovery_notice`) for good. The shell decides which
 * repair: the one startup reported, never a path the page names. Rejects with a
 * `CommandError` (`acknowledge_failed`) when the shell could not, and the notice then stays.
 */
export async function acknowledgeRecovery(): Promise<void> {
  await settle(commands.acknowledgeRecovery());
}

/**
 * A channel that also says when the shell ended it. Tauri ends a channel when the Rust side drops
 * its last handle (a chat view's subscription was removed, or its session stopped), but its
 * JavaScript `Channel` only unregisters the callback, silently. `cleanupCallback` runs exactly then,
 * after every message sent before the end was delivered, so this wraps it per instance. It is a
 * private method of @tauri-apps/api 2.12.4 (pinned exactly); `ipc.test.ts` fails if it changes.
 */
export function endingChannel<T>(onMessage: (message: T) => void, onEnd: () => void): Channel<T> {
  const channel = new Channel<T>(onMessage);
  const internals = channel as unknown as { cleanupCallback: () => void };
  const cleanup = internals.cleanupCallback.bind(channel);
  internals.cleanupCallback = () => {
    cleanup();
    onEnd();
  };
  return channel;
}

/** Every agent o7 knows, from the detector's cache; never waits. */
export function agentsList(): Promise<AgentEntry[]> {
  return infallible(commands.agentsList());
}

/** Resolves the environment again and probes every agent (15 s at most). */
export function agentsRefresh(): Promise<AgentEntry[]> {
  return infallible(commands.agentsRefresh());
}

export function agentMaintenance(agentId: string): Promise<AgentMaintenance> {
  return settle(commands.agentMaintenance(agentId));
}

export function agentAuthStatus(agentId: string): Promise<AgentAuthStatus> {
  return settle(commands.agentAuthStatus(agentId));
}

export function terminalReserve(): Promise<string> {
  return settle(commands.terminalReserve());
}
export function agentLoginStart(
  terminalId: string,
  agentId: string,
  rows: number,
  cols: number,
): Promise<string> {
  return settle(commands.agentLoginStart(terminalId, agentId, rows, cols));
}
/** The agent's own update command in a reserved terminal, after Update failed in the background. */
export function agentUpdateStart(
  terminalId: string,
  agentId: string,
  rows: number,
  cols: number,
): Promise<string> {
  return settle(commands.agentUpdateStart(terminalId, agentId, rows, cols));
}
/** A terminal for a chat in its agent's own interface: several per window, unlike sign-in's. */
export function chatTerminalReserve(): Promise<string> {
  return settle(commands.chatTerminalReserve());
}
/** Opens a chat in its agent's own interface in a terminal from `chatTerminalReserve`. */
export function chatTerminalStart(
  terminalId: string,
  chatId: string,
  rows: number,
  cols: number,
): Promise<ChatTerminalStarted> {
  return settle(commands.chatTerminalStart(terminalId, chatId, rows, cols));
}
/** The terminal chat's terminal ended: its chat gets its session back. */
export function chatTerminalEnded(terminalId: string): Promise<void> {
  return settle(commands.chatTerminalEnded(terminalId)).then(() => undefined);
}
export function terminalRead(terminalId: string): Promise<TerminalChunk> {
  return settle(commands.terminalRead(terminalId));
}
export async function terminalWrite(terminalId: string, bytes: number[]): Promise<void> {
  await settle(commands.terminalWrite(terminalId, bytes));
}
export async function terminalResize(
  terminalId: string,
  rows: number,
  cols: number,
): Promise<void> {
  await settle(commands.terminalResize(terminalId, rows, cols));
}
export async function terminalClose(terminalId: string): Promise<void> {
  await settle(commands.terminalClose(terminalId));
}

/** An agent's Connection setting (`null`: Automatic); answers every agent as `agentsList` does. */
export function agentSetConnection(
  agentId: string,
  connection: string | null,
): Promise<AgentEntry[]> {
  return settle(commands.agentSetConnection(agentId, connection));
}

export async function agentUpdate(agentId: string): Promise<void> {
  await settle(commands.agentUpdate(agentId));
}

/** The last lines the agent's updater printed in this run of o7, as plain text. */
export function agentUpdateOutput(agentId: string): Promise<string | null> {
  return settle(commands.agentUpdateOutput(agentId));
}

export function envStatus(): Promise<EnvStatus> {
  return infallible(commands.envStatus());
}

/** Starts resolving the environment again; returns the status at once. */
export function envRefresh(): Promise<EnvStatus> {
  return infallible(commands.envRefresh());
}

/** The system folder picker: the chosen folder, or `null` when the user cancelled. */
export function pickFolder(): Promise<string | null> {
  return settle(commands.pickFolder());
}

export function chatsList(): Promise<ChatSummary[]> {
  return settle(commands.chatsList());
}

export function chatCreate(agentId: string, folder: string): Promise<ChatSummary> {
  return settle(commands.chatCreate(agentId, folder));
}

/**
 * Opens a view of a chat. `onFrame` receives every frame of the view's channel, from before the
 * call resolves on (the join protocol buffers them); `onEnd` runs when the shell ends the channel.
 */
export async function chatAttach(
  chatId: string,
  onFrame: (frame: EventBatch) => void,
  onEnd: () => void,
): Promise<ChatAttach> {
  // Inside the async function, so a missing IPC bridge rejects instead of throwing synchronously.
  const channel = endingChannel<EventBatch>(onFrame, onEnd);
  return settle(commands.chatAttach(chatId, channel));
}

export function chatDetach(subscriptionId: string): Promise<void> {
  return infallible(commands.chatDetach(subscriptionId));
}

export function chatHistory(
  chatId: string,
  beforeSeq: number,
  maxBytes: number | null = null,
): Promise<HistoryPage> {
  return settle(commands.chatHistory(chatId, beforeSeq, maxBytes));
}

export function chatLists(chatId: string): Promise<SessionLists> {
  return settle(commands.chatLists(chatId));
}

export function chatFolderContext(chatId: string) {
  return settle(commands.chatFolderContext(chatId));
}

export function chatSend(chatId: string, text: string): Promise<PromptAck> {
  return settle(commands.chatSend(chatId, text));
}

export async function chatCancelQueued(chatId: string, promptId: string): Promise<void> {
  await settle(commands.chatCancelQueued(chatId, promptId));
}

export function chatQueuedText(chatId: string, promptId: string): Promise<string> {
  return settle(commands.chatQueuedText(chatId, promptId));
}

export function chatEditQueued(
  chatId: string,
  promptId: string,
  text: string,
): Promise<QueuedPrompt[]> {
  return settle(commands.chatEditQueued(chatId, promptId, text));
}

export function chatMoveQueued(
  chatId: string,
  promptId: string,
  index: number,
): Promise<QueuedPrompt[]> {
  return settle(commands.chatMoveQueued(chatId, promptId, index));
}

export async function chatResumeQueue(chatId: string): Promise<void> {
  await settle(commands.chatResumeQueue(chatId));
}

/** "Send now": the queued prompt goes next (interrupting a running turn); answers the queue. */
export function chatSendNow(chatId: string, promptId: string): Promise<QueuedPrompt[]> {
  return settle(commands.chatSendNow(chatId, promptId));
}

export async function chatInterrupt(chatId: string): Promise<void> {
  await settle(commands.chatInterrupt(chatId));
}

export async function chatForceStop(chatId: string): Promise<void> {
  await settle(commands.chatForceStop(chatId));
}

export async function chatStart(chatId: string, fresh: boolean): Promise<void> {
  await settle(commands.chatStart(chatId, fresh));
}

export async function chatStop(chatId: string): Promise<void> {
  await settle(commands.chatStop(chatId));
}

/** Answers a permission card with one of its options; `null` cancels it. */
export async function chatRespondPermission(
  chatId: string,
  requestId: string,
  optionId: string | null,
): Promise<void> {
  await settle(commands.chatRespondPermission(chatId, requestId, optionId));
}

/** Accepts a plan waiting for approval as the person edited it. */
export async function chatAcceptPlan(
  chatId: string,
  requestId: string,
  optionId: string,
  plan: string,
): Promise<void> {
  await settle(commands.chatAcceptPlan(chatId, requestId, optionId, plan));
}

/** Answers a question card; `null` dismisses it. */
export async function chatRespondUi(
  chatId: string,
  requestId: string,
  answer: UiAnswer | null,
): Promise<void> {
  await settle(commands.chatRespondUi(chatId, requestId, answer));
}

export function chatSetOption(
  chatId: string,
  optionId: string,
  value: ConfigValue,
): Promise<SessionLists> {
  return settle(commands.chatSetOption(chatId, optionId, value));
}

export function chatSetControl(
  chatId: string,
  change: SessionControlChange,
): Promise<SessionLists> {
  return settle(commands.chatSetControl(chatId, change));
}

export function chatSetAutonomy(chatId: string, autonomy: Autonomy): Promise<SessionLists> {
  return settle(commands.chatSetAutonomy(chatId, autonomy));
}

/** One page of a full output, ending on a character boundary; continue from `next_offset`. */
export function chatBlob(
  chatId: string,
  sha256: string,
  offset: number,
  len: number,
): Promise<BlobText> {
  return settle(commands.chatBlob(chatId, sha256, offset, len));
}

/** Shows the chat's newest protocol log; the shell chooses the file. */
export async function chatRevealLog(chatId: string): Promise<void> {
  await settle(commands.chatRevealLog(chatId));
}

/**
 * Subscribes this webview to chat-list updates, replacing its earlier subscription. `onEnd` runs
 * when the shell ends the channel.
 */
export async function statusSubscribe(
  onUpdate: (update: ChatStatusUpdate) => void,
  onEnd: () => void,
): Promise<string> {
  const channel = endingChannel<ChatStatusUpdate>(onUpdate, onEnd);
  return settle(commands.statusSubscribe(channel));
}

export function storageStatus(): Promise<StorageStatus> {
  return infallible(commands.storageStatus());
}
