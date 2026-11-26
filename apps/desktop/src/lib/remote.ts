import { listen } from "@tauri-apps/api/event";
import {
  commands,
  type LoginStart,
  type RemoteBotLink,
  type RemotePairing,
  type RemoteStatus,
} from "../bindings";
import { inTauri, settle } from "./ipc";

/**
 * Remote access (docs/remote-sessions.md): this computer's chats from a paired browser, Ghost for
 * iPhone or a linked bot, through Ghost's relay. Settings is its only surface in the app.
 */
export interface RemoteApi {
  readonly status: () => Promise<RemoteStatus>;
  readonly setEnabled: (enabled: boolean) => Promise<RemoteStatus>;
  /** The computer does not sleep on its own while something can reach it. */
  readonly setKeepAwake: (on: boolean) => Promise<RemoteStatus>;
  /** Whether this copy of Ghost opens, in the background, when the person signs in. */
  readonly loginStart: () => Promise<LoginStart>;
  readonly setLoginStart: (on: boolean) => Promise<LoginStart>;
  /** Opens a pairing link, good once and for ten minutes. It replaces the one before it. */
  readonly pair: () => Promise<RemotePairing>;
  /** This program's path: an assistant on this computer starts it with `--ghost-bot-helper`. */
  readonly localProgram: () => Promise<string>;
  /** The open workspace's projects, for an automation to start its tasks in. */
  readonly projects: () => Promise<readonly RemoteProject[]>;
  /**
   * A new automation: its address takes what a service sends (a webhook) and starts a task in
   * `project` with `instruction`, what was sent handed over as data. Returned only here.
   */
  readonly createAutomation: (
    name: string,
    project: string,
    instruction: string,
  ) => Promise<RemoteBotLink>;
  readonly cancelPairing: () => Promise<RemoteStatus>;
  readonly removeDevice: (id: string) => Promise<RemoteStatus>;
  /** A new bot link. Its address carries the bot's token and is returned only here. */
  readonly createBot: (name: string, rights: BotRights) => Promise<RemoteBotLink>;
  /** A new address for a link, shown once; the old one stops working. */
  readonly renewBot: (id: string) => Promise<RemoteBotLink>;
  readonly removeBot: (id: string) => Promise<RemoteStatus>;
  /**
   * Where a bot link is told that a chat it acted on finished or asks for an answer. The key is
   * kept in this computer's secure storage and never comes back; `null` keeps the one kept.
   */
  readonly setBotCallback: (id: string, callback: BotCallbackInput) => Promise<RemoteStatus>;
  readonly clearBotCallback: (id: string) => Promise<RemoteStatus>;
  /** Sends the address a test message now; the status says how it went. */
  readonly testBotCallback: (id: string) => Promise<RemoteStatus>;
  /**
   * Sends Ghost for iPhone a test notification now, through the relay and Apple: at most one every
   * ten seconds. The status says when Apple last took one for it.
   */
  readonly testNotification: (id: string) => Promise<RemoteStatus>;
  /** Calls back whenever the status changed (a device paired, connected or left). */
  readonly subscribe: (changed: (status: RemoteStatus) => void) => () => void;
}

/**
 * What a bot link may do, chosen when it is made. `read` reads chats only; `chats` also sends,
 * answers questions and stops turns; `full` adds the rest of the window's chat and project
 * controls. Approving an agent's permission requests and turning on full access are separate
 * choices, off unless chosen.
 */
export interface BotRights {
  readonly level: "read" | "chats" | "full";
  readonly approve: boolean;
  readonly fullAccess: boolean;
}

/** A link's level, as its row names it. */
export function levelName(bot: { readonly read_only: boolean; readonly full: boolean }): string {
  return bot.read_only ? "Read only" : bot.full ? "Full control" : "Chats";
}

/** A project an automation can start its tasks in. */
export interface RemoteProject {
  readonly id: string;
  readonly name: string;
}

/** Which assistant's way of being woken an address is. */
export type CallbackKind = "grok" | "hermes" | "openclaw" | "webhook";

export interface BotCallbackInput {
  readonly kind: CallbackKind;
  readonly url: string;
  readonly key: string | null;
  /** OpenClaw only: where its answer goes. Both or neither. */
  readonly channel: string | null;
  readonly to: string | null;
}

export const remoteApi: RemoteApi = {
  status: () => settle(commands.remoteStatus()),
  setEnabled: (enabled) => settle(commands.remoteSetEnabled(enabled)),
  setKeepAwake: (on) => settle(commands.remoteSetKeepAwake(on)),
  loginStart: () => commands.loginStartStatus(),
  setLoginStart: (on) => settle(commands.loginStartSet(on)),
  pair: () => settle(commands.remotePair()),
  localProgram: () => settle(commands.remoteLocalProgram()),
  projects: async () =>
    (
      await settle(commands.workspaceSnapshot({ include_archived: false, project_id: null }))
    ).projects.map(({ id, name }) => ({ id, name })),
  createAutomation: (name, project, instruction) =>
    settle(commands.remoteCreateAutomation(name, project, instruction)),
  cancelPairing: () => settle(commands.remoteCancelPairing()),
  removeDevice: (id) => settle(commands.remoteRemoveDevice(id)),
  createBot: (name, rights) =>
    settle(
      commands.remoteCreateBot(
        name,
        rights.level,
        rights.level !== "read" && rights.approve,
        rights.level === "full" && rights.fullAccess,
      ),
    ),
  renewBot: (id) => settle(commands.remoteRenewBot(id)),
  removeBot: (id) => settle(commands.remoteRemoveBot(id)),
  setBotCallback: (id, callback) =>
    settle(
      commands.remoteSetBotCallback(
        id,
        callback.kind,
        callback.url,
        callback.key,
        callback.channel,
        callback.to,
      ),
    ),
  clearBotCallback: (id) => settle(commands.remoteClearBotCallback(id)),
  testBotCallback: (id) => settle(commands.remoteTestBotCallback(id)),
  testNotification: (id) => settle(commands.remoteTestNotification(id)),
  subscribe: (changed) => {
    if (!inTauri()) return () => {};
    let stopped = false;
    let stop: (() => void) | null = null;
    void listen<RemoteStatus>("auracode-remote-changed", (event) => changed(event.payload)).then(
      (unlisten) => {
        if (stopped) unlisten();
        else stop = unlisten;
      },
      () => {},
    );
    return () => {
      stopped = true;
      stop?.();
    };
  },
};

/** The connection in a few words, for the row under the switch. */
export function linkLabel(status: RemoteStatus): string {
  if (!status.enabled) return "Off. Nothing on this computer can be reached from outside.";
  switch (status.link.state) {
    case "online":
      return "On. This computer can be reached by the devices and bots below.";
    case "connecting":
    case "off":
      return "Connecting to Ghost's relay…";
    case "offline":
      return `Not reachable right now; trying again. ${status.link.reason}`;
  }
}

/** The connection as a dot's colour: green when reachable, amber while it tries, off when off. */
export function linkTone(status: RemoteStatus): "green" | "amber" | "off" {
  if (!status.enabled) return "off";
  return status.link.state === "online" ? "green" : "amber";
}

/** When something last happened, as a short phrase. */
export function ago(at: number, now: number): string {
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return hours === 1 ? "1 hour ago" : `${hours} hours ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return days === 1 ? "yesterday" : `${days} days ago`;
  return new Date(at).toLocaleDateString(undefined, { dateStyle: "medium" });
}

/** The minutes and seconds a pairing link still opens for. */
export function remaining(expiresAt: number, now: number): string {
  const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}
