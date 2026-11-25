/**
 * A browser's remote session with one computer (docs/remote-sessions.md): the socket to the
 * relay, the sealed channel through it, and the state the page draws.
 *
 * The chat itself is folded by the desktop app's own reducer, from the same events its window
 * gets, so a turn reads the same here as there.
 *
 * Staying live on a phone:
 * - A phone that slept can hold a socket that still says it is open and carries nothing. When the
 *   page comes back to the front (`wake`), it asks the relay for a quick answer and starts a new
 *   session at once when none comes, rather than waiting out the keep-alive.
 * - A request with no answer in time means the same: the session is checked, and replaced if dead.
 * - A chat tapped while the session is starting opens as soon as it is up.
 * - Live events are folded as they arrive, and drawn at most once a frame.
 */

import { THEME_IDS } from "@o7/ui/graphite";
import type {
  ChatAttach,
  ChatSnapshot,
  ChatStatusUpdate,
  EventBatch,
  HistoryPage,
  ProjectSummary,
  PromptAck,
  UiAnswer,
  WorkspaceChat,
} from "../../../desktop/src/bindings";
import {
  applyEntries,
  compactToolUpdates,
  type Entry,
  emptyTranscript,
  type Transcript,
  withTasks,
} from "../../../desktop/src/lib/chat/transcript";
import { Channel, decode, encode, importSecret, randomBytes } from "./channel";
import {
  deleteHost,
  type Look,
  loadHosts,
  onHostsChanged,
  type SavedHost,
  saveHost,
} from "./storage";

export type { Look } from "./storage";

export interface RemoteAgent {
  readonly id: string;
  readonly name: string;
  readonly available: boolean;
}

/** One thing a chat lets a person choose, as the app names it. */
export interface Choice {
  readonly value: string;
  readonly name: string;
}

/** A chat's models and thinking levels, and what it is set to. */
export interface Choices {
  readonly models: readonly Choice[];
  readonly model: string | null;
  readonly thinking: readonly Choice[];
  readonly thinking_level: string | null;
}

export interface Overview {
  readonly host_name: string;
  readonly projects: readonly ProjectSummary[];
  readonly chats: readonly WorkspaceChat[];
  readonly agents: readonly RemoteAgent[];
  readonly default_agent: string | null;
}

export type Phase =
  /** Nothing is paired and no pairing link was opened. */
  | { readonly name: "empty" }
  | { readonly name: "connecting" }
  /** The relay answers; the computer is off, asleep or has o7 closed. */
  | { readonly name: "away" }
  | { readonly name: "ready" }
  /**
   * This pairing is over: the link was spent, or the computer no longer accepts this browser
   * (`refused` names it, so the page can offer to try again or to forget it). `back` names the
   * computer this page was with before a pairing link that did not work, to go back to.
   */
  | {
      readonly name: "ended";
      readonly reason: string;
      readonly refused?: string;
      readonly back?: string;
    };

export interface OpenChat {
  readonly id: string;
  readonly snapshot: ChatSnapshot;
  readonly transcript: Transcript;
  /** Older history exists and can be loaded. */
  readonly hasMore: boolean;
  readonly loadingEarlier: boolean;
}

export interface State {
  readonly phase: Phase;
  readonly hosts: readonly SavedHost[];
  /** The computer this session is with. */
  readonly host: { readonly id: string; readonly name: string } | null;
  readonly overview: Overview | null;
  /** The chat being opened, before its history has arrived. */
  readonly opening: string | null;
  readonly chat: OpenChat | null;
  /**
   * What this computer's o7 offers beyond the first version's requests, as it said when
   * the session opened. A control is shown only when its feature is named here: a request an
   * older o7 does not know ends the session.
   */
  readonly features: readonly string[];
  /** How the computer looks: its theme and appearance, as it said, or as it last said. */
  readonly look: Look | null;
}

/** A request the computer refused, in its own words. */
export class RemoteError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

type Credential =
  | { readonly kind: "pair"; readonly hostId: string; readonly id: string; readonly key: CryptoKey }
  | { readonly kind: "device"; readonly host: SavedHost };

interface Pending {
  readonly resolve: (data: unknown) => void;
  readonly reject: (error: Error) => void;
}

const VERSION = 1;
const PING_MS = 25_000;
const PONG_MS = 10_000;
/** After the page was away: how long the relay has to answer before the session is replaced. */
const PROBE_MS = 3_000;
/** Most requests; starting a chat or handing it over waits on an agent and gets longer. */
const REQUEST_MS = 15_000;
/**
 * A message, a stop or an answer acts on the computer: giving up early invites sending it twice,
 * so these wait as long as they always did.
 */
const ACT_MS = 30_000;
const SLOW_REQUEST_MS = 45_000;

/** A look as the computer sent it, kept only when its names are known. */
export function lookOf(value: unknown): Look | null {
  if (value === null || typeof value !== "object") return null;
  const { theme, appearance } = value as { theme?: unknown; appearance?: unknown };
  if (typeof theme !== "string" || !(THEME_IDS as readonly string[]).includes(theme)) return null;
  if (appearance !== "system" && appearance !== "light" && appearance !== "dark") return null;
  return { theme, appearance };
}

/** This browser in a few words, as the computer's Settings will list it. */
export function deviceName(agent: string): string {
  const platform = /iPhone/.test(agent)
    ? "iPhone"
    : /iPad/.test(agent)
      ? "iPad"
      : /Android/.test(agent)
        ? "Android"
        : /Mac OS X|Macintosh/.test(agent)
          ? "Mac"
          : /Windows/.test(agent)
            ? "Windows PC"
            : /Linux/.test(agent)
              ? "Linux"
              : "Browser";
  const browser = /Edg\//.test(agent)
    ? "Edge"
    : /Firefox|FxiOS/.test(agent)
      ? "Firefox"
      : /Chrome|CriOS/.test(agent)
        ? "Chrome"
        : /Safari/.test(agent)
          ? "Safari"
          : null;
  return browser ? `${platform} (${browser})` : platform;
}

/** A pairing link's fragment: the computer's id, the link's id and its key. */
export function parsePairing(
  fragment: string,
): { hostId: string; id: string; key: Uint8Array<ArrayBuffer> } | null {
  const [hostId, id, key, ...rest] = fragment.replace(/^#/, "").split(".");
  if (!hostId || !id || !key || rest.length > 0) return null;
  const bytes = decode(key, 32);
  if (!decode(hostId, 16) || !decode(id, 8) || !bytes) return null;
  return { hostId, id, key: bytes };
}

/** Draws on the next frame while the page is shown, and soon after while it is hidden. */
function nextPaint(work: () => void): () => void {
  if (typeof document !== "undefined" && document.visibilityState === "visible") {
    const frame = requestAnimationFrame(work);
    return () => cancelAnimationFrame(frame);
  }
  const timer = setTimeout(work, 120);
  return () => clearTimeout(timer);
}

export class RemoteClient {
  #state: State = {
    phase: { name: "connecting" },
    hosts: [],
    host: null,
    overview: null,
    opening: null,
    chat: null,
    features: [],
    look: null,
  };
  readonly #listeners = new Set<() => void>();
  readonly #origin: string;
  #credential: Credential | null = null;
  #socket: WebSocket | null = null;
  #channel: Channel | null = null;
  #nonce: Uint8Array<ArrayBuffer> | null = null;
  #ready = false;
  /** Incoming messages are opened one after another, in the order they arrived. */
  #inbox: Promise<void> = Promise.resolve();
  /** Outgoing messages are sealed one after another, so their numbers match their order. */
  #outbox: Promise<void> = Promise.resolve();
  readonly #pending = new Map<number, Pending>();
  #nextId = 1;
  #retries = 0;
  #retry: ReturnType<typeof setTimeout> | null = null;
  #ping: ReturnType<typeof setInterval> | null = null;
  #pong: ReturnType<typeof setTimeout> | null = null;
  #stopped = false;
  /** The open chat's every entry, oldest first: an earlier page rebuilds the transcript. */
  #entries: Entry[] = [];
  #cut: string | null = null;
  #before: number | null = null;
  #expected = 0;
  /** Live frames that arrived while the chat's history was still on its way. */
  #early: EventBatch[] = [];
  #refetch: ReturnType<typeof setTimeout> | null = null;
  /** The open being asked for, so a second tap waits on the same one. */
  #opening: { readonly chatId: string; readonly done: Promise<void> } | null = null;
  /** Live entries folded into `#entries` and not drawn yet, and the session state they bring. */
  #fresh: { readonly chatId: string; entries: Entry[]; snapshot: ChatSnapshot } | null = null;
  #paint: (() => void) | null = null;
  /** The computer this page was with when a pairing link took over, to go back to if it fails. */
  #back: string | null = null;
  #unwatch: (() => void) | null = null;

  /** `origin` is the relay's WebSocket origin, the page's own unless a test says otherwise. */
  constructor(origin: string) {
    this.#origin = origin;
  }

  readonly getState = (): State => this.#state;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  };

  #set(patch: Partial<State>) {
    this.#state = { ...this.#state, ...patch };
    for (const listener of this.#listeners) listener();
  }

  /**
   * Loads what is paired and connects: with the pairing link in `fragment` when there is one,
   * else to the computer paired most recently.
   */
  async start(fragment: string): Promise<void> {
    this.#watch();
    if (fragment.replace(/^#/, "") !== "") return this.pair(fragment);
    const hosts = await loadHosts();
    this.#set({ hosts, look: lookOf(hosts[0]?.look) });
    if (hosts[0]) {
      this.#credential = { kind: "device", host: hosts[0] };
      this.#connect();
    } else {
      this.#set({ phase: { name: "empty" } });
    }
  }

  /**
   * Pairs with the computer a link names, whatever this page is doing: a session with another
   * computer ends, and every computer paired before stays paired. If the link does not work, the
   * page offers the way back to the computer it was with.
   */
  async pair(fragment: string): Promise<void> {
    this.#watch();
    const current = this.#credential;
    const back =
      current?.kind === "device"
        ? current.host.hostId
        : this.#state.phase.name === "ended"
          ? (this.#state.phase.back ?? null)
          : this.#back;
    const hosts = await loadHosts();
    const pairing = parsePairing(fragment);
    this.#forgetChat();
    this.#set({
      hosts,
      host: null,
      overview: null,
      chat: null,
      opening: null,
      features: [],
      look: lookOf(hosts.find((entry) => entry.hostId === pairing?.hostId)?.look),
    });
    this.#back = back;
    // The session with the computer this page was with ends here, whatever the link turns out to be.
    this.#credential = null;
    if (!pairing) {
      this.#end("This pairing link is not complete.");
      return;
    }
    this.#credential = {
      kind: "pair",
      hostId: pairing.hostId,
      id: pairing.id,
      key: await importSecret(pairing.key),
    };
    pairing.key.fill(0);
    this.#stopped = false;
    this.#retries = 0;
    this.#connect();
  }

  /** Switches to another paired computer. */
  use(hostId: string) {
    const host = this.#state.hosts.find((entry) => entry.hostId === hostId);
    if (!host) return;
    this.#watch();
    this.#back = null;
    this.#credential = { kind: "device", host };
    this.#forgetChat();
    this.#set({ host: null, overview: null, chat: null, opening: null, look: lookOf(host.look) });
    this.#connect();
  }

  stop() {
    this.#stopped = true;
    this.#unwatch?.();
    this.#unwatch = null;
    this.#close();
  }

  /** Follows pairings saved or deleted by this browser's other remote pages. */
  #watch() {
    this.#unwatch ??= onHostsChanged(() => void this.#hostsChanged());
  }

  /**
   * Another page of this browser paired or forgot a computer. The list follows, and a session whose
   * computer was paired again there moves to the new key: the old one is about to stop working.
   */
  async #hostsChanged() {
    const hosts = await loadHosts();
    this.#set({ hosts });
    const credential = this.#credential;
    if (credential?.kind !== "device") return;
    const saved = hosts.find((entry) => entry.hostId === credential.host.hostId);
    if (saved && saved.deviceId !== credential.host.deviceId) this.use(saved.hostId);
  }

  /**
   * The page came back to the front, or the network came back: a socket that says it is open
   * may be dead, so the relay is asked for a quick answer, and a new session starts at once when
   * it gives none.
   */
  wake() {
    if (this.#stopped || !this.#credential) return;
    const socket = this.#socket;
    if (!socket || socket.readyState > WebSocket.OPEN) {
      this.#retries = 0;
      this.#connect();
      return;
    }
    this.#probe(socket);
  }

  /** Asks the relay to answer within a moment; a new session starts when it does not. */
  #probe(socket: WebSocket) {
    if (socket.readyState !== WebSocket.OPEN) return;
    if (this.#pong !== null) clearTimeout(this.#pong);
    this.#pong = setTimeout(() => {
      if (socket !== this.#socket) return;
      this.#retries = 0;
      this.#connect();
    }, PROBE_MS);
    try {
      socket.send("ping");
    } catch {
      this.#retries = 0;
      this.#connect();
    }
  }

  #close() {
    if (this.#retry !== null) clearTimeout(this.#retry);
    if (this.#ping !== null) clearInterval(this.#ping);
    if (this.#pong !== null) clearTimeout(this.#pong);
    this.#retry = this.#ping = this.#pong = null;
    const socket = this.#socket;
    this.#socket = null;
    if (socket) {
      socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
      try {
        socket.close();
      } catch {
        // Already closing.
      }
    }
    this.#sessionOver("The connection closed.");
  }

  /** The sealed session is gone; whatever waited on it fails. */
  #sessionOver(reason: string) {
    this.#channel = null;
    this.#nonce = null;
    this.#ready = false;
    for (const pending of this.#pending.values()) pending.reject(new Error(reason));
    this.#pending.clear();
  }

  #connect() {
    const credential = this.#credential;
    if (!credential || this.#stopped) return;
    this.#close();
    if (this.#state.phase.name !== "away") this.#set({ phase: { name: "connecting" } });
    const hostId = credential.kind === "pair" ? credential.hostId : credential.host.hostId;
    const socket = new WebSocket(`${this.#origin}/api/remote/v1/client/${hostId}`);
    this.#socket = socket;
    socket.onmessage = (event) => {
      if (typeof event.data !== "string" || socket !== this.#socket) return;
      const text = event.data;
      this.#inbox = this.#inbox.then(() => this.#received(socket, text)).catch(() => {});
    };
    socket.onclose = () => {
      if (socket !== this.#socket) return;
      this.#socket = null;
      this.#sessionOver("The connection closed.");
      if (this.#state.phase.name === "ended" || this.#stopped) return;
      if (this.#state.phase.name === "ready") this.#set({ phase: { name: "connecting" } });
      // At once, then 1, 2, 4, 8, then every 15 seconds.
      const wait =
        this.#retries === 0 ? 0 : Math.min(15_000, 1000 * 2 ** Math.min(this.#retries - 1, 4));
      this.#retries += 1;
      this.#retry = setTimeout(() => this.#connect(), wait);
    };
    this.#ping = setInterval(() => {
      if (socket.readyState !== WebSocket.OPEN) return;
      socket.send("ping");
      this.#pong ??= setTimeout(() => socket.close(), PONG_MS);
    }, PING_MS);
  }

  async #received(socket: WebSocket, text: string) {
    if (socket !== this.#socket) return;
    if (text === "pong") {
      if (this.#pong !== null) clearTimeout(this.#pong);
      this.#pong = null;
      return;
    }
    if (text === "h0") {
      this.#sessionOver("The computer went away.");
      this.#set({ phase: { name: "away" } });
      return;
    }
    if (text === "h1") return this.#hello(socket);
    // The relay ended this socket; the close that follows connects again.
    if (text === "x") return;
    if (!text.startsWith("m")) return;
    const kind = text[1];
    const body = text.slice(2);
    try {
      if (kind === "R") return this.#refused(body);
      if (kind === "W") return await this.#welcome(body);
      if (kind === "E" && this.#channel) return await this.#opened(await this.#channel.open(body));
    } catch {
      // A message this session cannot open ends it; a new one starts on reconnect.
      socket.close();
    }
  }

  #hello(socket: WebSocket) {
    const credential = this.#credential;
    if (!credential) return;
    this.#sessionOver("The session started again.");
    this.#nonce = randomBytes(16);
    const id = credential.kind === "pair" ? credential.id : credential.host.deviceId;
    socket.send(
      `mH${JSON.stringify({ v: VERSION, kind: credential.kind, id, nonce: encode(this.#nonce) })}`,
    );
    if (this.#state.phase.name === "away") this.#set({ phase: { name: "connecting" } });
  }

  /**
   * The computer's answer to this side's hello. Exactly one is accepted per hello: a second one,
   * replayed by whoever carries the messages, would derive the same keys again and start their
   * message numbers over, which must never happen. So the nonce is spent the moment it is used,
   * and a welcome with no hello waiting for it ends the session.
   */
  async #welcome(body: string) {
    const credential = this.#credential;
    const nonce = this.#nonce;
    this.#nonce = null;
    const welcome = JSON.parse(body) as { v?: unknown; nonce?: unknown };
    const hostNonce = typeof welcome.nonce === "string" ? decode(welcome.nonce, 16) : null;
    if (!credential || !nonce || this.#channel || welcome.v !== VERSION || !hostNonce)
      throw new Error("unexpected welcome");
    const [key, id] =
      credential.kind === "pair"
        ? [credential.key, credential.id]
        : [credential.host.key, credential.host.deviceId];
    this.#channel = await Channel.create(
      "client",
      key,
      nonce,
      hostNonce,
      `${credential.kind}|${id}`,
    );
  }

  #refused(body: string) {
    const reason = (JSON.parse(body) as { reason?: unknown }).reason;
    const credential = this.#credential;
    if (reason === "busy") return;
    if (reason === "version") {
      this.#end("This page and the o7 on that computer do not match. Update o7.");
      return;
    }
    if (credential?.kind === "device" && reason === "elsewhere") {
      // A pairing reaches the workspace it was made in. The key is still good there.
      const host = credential.host;
      this.#end(
        `${host.hostName} has another workspace open. This browser is paired with a different one: open that workspace in o7, or pair this browser again from the one that is open.`,
        host.hostId,
      );
      return;
    }
    if (credential?.kind === "device") {
      // Nothing proves who sent a refusal, so it deletes nothing: the key stays until the person
      // decides. If the device was removed on the computer, forgetting it here tidies up.
      void this.#refusedDevice(credential.host);
    } else {
      this.#end(
        "This pairing link was already used or has expired. Create a new one in o7: Settings, Remote access, Pair.",
      );
    }
  }

  /**
   * The computer turned this browser's key away. When another page of this browser paired the
   * computer again meanwhile, the new key is in storage: the session moves to it. Otherwise the
   * person decides what happens to the key.
   */
  async #refusedDevice(host: SavedHost) {
    const hosts = await loadHosts();
    const saved = hosts.find((entry) => entry.hostId === host.hostId);
    if (saved && saved.deviceId !== host.deviceId) {
      this.#set({ hosts });
      this.use(saved.hostId);
      return;
    }
    this.#end(
      `${host.hostName} no longer accepts this browser. If you removed it in o7's Settings, pair again from there.`,
      host.hostId,
    );
  }

  /**
   * Ends this pairing with `reason`. After a pairing link that did not work, the page offers the
   * computer it was with before, when there was one.
   */
  #end(reason: string, refused?: string) {
    const back =
      this.#credential?.kind !== "device" && this.#back && this.#back !== refused
        ? this.#back
        : null;
    this.#credential = null;
    this.#back = null;
    this.#forgetChat();
    this.#set({
      phase: {
        name: "ended",
        reason,
        ...(refused ? { refused } : {}),
        ...(back ? { back } : {}),
      },
      host: null,
      overview: null,
      chat: null,
      opening: null,
    });
    this.#close();
  }

  /** One opened message: a reply to a request, or something the computer says by itself. */
  async #opened(text: string) {
    const message = JSON.parse(text) as Record<string, unknown>;
    // A session's first sealed message is the computer's "ready", once; nothing else comes first.
    if ((message.ev === "ready") === this.#ready) throw new Error("out of place");
    if (typeof message.re === "number") {
      const pending = this.#pending.get(message.re);
      this.#pending.delete(message.re);
      if (!pending) return;
      if (message.ok === true) pending.resolve(message.data);
      else {
        const error = (message.error ?? {}) as { code?: unknown; message?: unknown };
        pending.reject(
          new RemoteError(
            typeof error.code === "string" ? error.code : "failed",
            typeof error.message === "string" ? error.message : "The computer refused.",
          ),
        );
      }
      return;
    }
    switch (message.ev) {
      case "ready": {
        // Opening this proved the computer holds the key. Now this side proves the same.
        this.#ready = true;
        const look = lookOf(message.look);
        this.#set({
          features: Array.isArray(message.features)
            ? message.features.filter((name): name is string => typeof name === "string")
            : [],
          ...(look ? { look } : {}),
        });
        void this.#begin(String(message.host_name ?? ""), look).catch((error: unknown) => {
          // The computer refused to pair, in its own words (its devices are full, the link
          // closed): that is the end of this link, and reconnecting would only spend it again.
          if (error instanceof RemoteError && this.#credential?.kind === "pair") {
            this.#end(error.message);
            return;
          }
          this.#socket?.close();
        });
        break;
      }
      case "chat":
        this.#status(message.update as ChatStatusUpdate);
        break;
      case "frame":
        this.#frame(message.batch as EventBatch);
        break;
      case "lost": {
        const open = this.#state.chat;
        if (open && open.id === message.chat_id) void this.#reopen(open.id);
        break;
      }
      case "reset":
        this.#forgetChat();
        this.#set({ chat: null, opening: null });
        void this.refresh().catch(() => {});
        break;
    }
  }

  /** The session is up: pair if this was a pairing link, then show the computer's chats. */
  async #begin(hostName: string, look: Look | null) {
    const credential = this.#credential;
    if (!credential) return;
    this.#retries = 0;
    let host: SavedHost;
    if (credential.kind === "pair") {
      const paired = (await this.#request({
        op: "pair",
        name: deviceName(navigator.userAgent),
      })) as { device_id: string; device_key: string };
      const raw = decode(paired.device_key, 32);
      if (!raw) throw new Error("unexpected device key");
      host = {
        hostId: credential.hostId,
        hostName,
        deviceId: paired.device_id,
        key: await importSecret(raw),
        pairedAt: Date.now(),
        ...(look ? { look } : {}),
      };
      raw.fill(0);
      // Paired with this computer before: the new device replaces the old one, here and there.
      const replaced = this.#state.hosts.find((entry) => entry.hostId === credential.hostId);
      await saveHost(host);
      // The session carries on as the device; the next connection uses its own key.
      this.#credential = { kind: "device", host };
      this.#back = null;
      const added = host;
      this.#set({
        hosts: [added, ...this.#state.hosts.filter((entry) => entry.hostId !== added.hostId)],
      });
      if (replaced && replaced.deviceId !== added.deviceId)
        void forgetDevice(this.#origin, replaced).catch(() => {});
    } else {
      // The computer was renamed, or changed its look: the saved record follows.
      const renamed = hostName && credential.host.hostName !== hostName;
      const restyled =
        look &&
        (credential.host.look?.theme !== look.theme ||
          credential.host.look?.appearance !== look.appearance);
      host = {
        ...credential.host,
        ...(renamed ? { hostName } : {}),
        ...(restyled ? { look } : {}),
      };
      if (renamed || restyled) {
        this.#credential = { kind: "device", host };
        void saveHost(host);
        const changed = host;
        this.#set({
          hosts: this.#state.hosts.map((entry) =>
            entry.hostId === changed.hostId ? changed : entry,
          ),
        });
      }
    }
    this.#set({
      phase: { name: "ready" },
      host: { id: host.hostId, name: host.hostName },
    });
    await this.refresh();
    // The chat that was open, or tapped while this session started, opens again.
    const following = this.#state.opening ?? this.#state.chat?.id;
    if (following) await this.#reopen(following);
  }

  /** Opens a chat again after the session changed; a failure leaves it for the list. */
  async #reopen(chatId: string) {
    this.#opening = null;
    try {
      await this.open(chatId);
    } catch {
      if (this.#state.opening === chatId || this.#state.chat?.id === chatId) {
        this.#forgetChat();
        this.#set({ chat: null, opening: null });
      }
    }
  }

  #request(body: Record<string, unknown>, timeout = REQUEST_MS): Promise<unknown> {
    const channel = this.#channel;
    const socket = this.#socket;
    if (!channel || !socket || !this.#ready || socket.readyState !== WebSocket.OPEN)
      return Promise.reject(new Error("The computer is not reachable right now."));
    const id = this.#nextId;
    this.#nextId += 1;
    return new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error("The computer did not answer in time."));
        // An answer that never came may mean a dead socket: check it, and start over if so.
        if (socket === this.#socket) this.#probe(socket);
      }, timeout);
      this.#pending.set(id, {
        resolve: (data) => {
          clearTimeout(timer);
          resolve(data);
        },
        reject: (error) => {
          clearTimeout(timer);
          reject(error);
        },
      });
      this.#outbox = this.#outbox
        .then(async () => {
          // A session that changed while this waited its turn cannot carry it.
          if (channel !== this.#channel) throw new Error("The session started again.");
          socket.send(`mE${await channel.seal(JSON.stringify({ id, ...body }))}`);
        })
        .catch((error: unknown) => {
          this.#pending.get(id)?.reject(error instanceof Error ? error : new Error("Not sent."));
          this.#pending.delete(id);
        });
    });
  }

  /** Reads the computer's projects, chats and agents again. */
  async refresh(): Promise<void> {
    const overview = (await this.#request({ op: "overview" })) as Overview;
    this.#set({
      overview,
      host: this.#state.host ? { ...this.#state.host, name: overview.host_name } : null,
    });
  }

  #status(update: ChatStatusUpdate) {
    const overview = this.#state.overview;
    if (!overview) return;
    const known = overview.chats.some((entry) => entry.chat.id === update.chat_id);
    if (!known) {
      // A chat made elsewhere: one read brings it, however many updates announce it.
      this.#refetch ??= setTimeout(() => {
        this.#refetch = null;
        void this.refresh().catch(() => {});
      }, 400);
      return;
    }
    const merge = <T extends { id: string }>(chat: T): T =>
      chat.id === update.chat_id
        ? {
            ...chat,
            status: update.status,
            needs_attention: update.needs_attention,
            title: update.title ?? (chat as { title?: string | null }).title ?? null,
            updated_at: update.updated_at,
          }
        : chat;
    const chats = overview.chats
      .map((entry) => ({ ...entry, chat: merge(entry.chat) }))
      .sort((a, b) => b.chat.updated_at - a.chat.updated_at);
    this.#paintNow();
    const open = this.#state.chat;
    this.#set({
      overview: { ...overview, chats },
      chat:
        open && open.id === update.chat_id
          ? { ...open, snapshot: { ...open.snapshot, chat: merge(open.snapshot.chat) } }
          : open,
    });
  }

  /**
   * Follows a chat: its newest history, then its live events. Tapped while the session is still
   * starting, it opens as soon as the session is up. A second tap on the same chat waits on the
   * first; one overtaken by a later tap ends quietly.
   */
  open(chatId: string): Promise<void> {
    if (this.#opening?.chatId === chatId) return this.#opening.done;
    this.#early = [];
    if (this.#state.chat?.id !== chatId) this.#forgetChat();
    this.#set({ opening: chatId, chat: this.#state.chat?.id === chatId ? this.#state.chat : null });
    if (!this.#ready) {
      // `#begin` opens it once the computer answers.
      this.#opening = null;
      return Promise.resolve();
    }
    const done = this.#attach(chatId).finally(() => {
      if (this.#opening?.chatId === chatId) this.#opening = null;
    });
    this.#opening = { chatId, done };
    return done;
  }

  async #attach(chatId: string): Promise<void> {
    let attach: ChatAttach;
    try {
      attach = (await this.#request({ op: "open", chat_id: chatId })) as ChatAttach;
    } catch (error) {
      // Overtaken by another chat, or by going back: nothing to say.
      if (this.#state.opening !== chatId) return;
      if (error instanceof RemoteError && error.code === "superseded") return;
      // The session ended while it waited: it opens again with the next one.
      if (!this.#ready) return;
      this.#set({ opening: null });
      throw error;
    }
    if (this.#state.opening !== chatId) return;
    this.#entries = compactToolUpdates(entriesOf(attach.history));
    this.#cut = attach.history.cut_turn_id;
    this.#before = attach.history.has_more ? attach.history.before_seq : null;
    this.#expected = attach.next_seq;
    this.#fresh = null;
    const transcript = withTasks(
      applyEntries(emptyTranscript(this.#cut), this.#entries),
      attach.snapshot.tasks ?? [],
    );
    this.#set({
      opening: null,
      chat: {
        id: chatId,
        snapshot: attach.snapshot,
        transcript,
        hasMore: this.#before !== null,
        loadingEarlier: false,
      },
    });
    const early = this.#early;
    this.#early = [];
    for (const batch of early) this.#frame(batch);
  }

  /** Stops following and returns to the list. */
  close() {
    this.#forgetChat();
    this.#opening = null;
    this.#set({ chat: null, opening: null });
    if (this.#ready) void this.#request({ op: "close" }).catch(() => {});
  }

  /** Drops what was kept of the open chat. */
  #forgetChat() {
    this.#paint?.();
    this.#paint = null;
    this.#fresh = null;
    this.#entries = [];
    this.#early = [];
  }

  #frame(batch: EventBatch) {
    if (this.#state.opening === batch.chat_id) {
      this.#early.push(batch);
      return;
    }
    const chat = this.#state.chat;
    if (!chat || chat.id !== batch.chat_id) return;
    if (batch.seq_start > this.#expected) {
      // Events were missed: the chat is read again from the computer.
      void this.#reopen(chat.id);
      return;
    }
    const fresh: Entry[] = [];
    let snapshot = this.#fresh?.snapshot ?? chat.snapshot;
    batch.events.forEach((event, index) => {
      const seq = batch.seq_start + index;
      if (seq < this.#expected) return;
      if (event.type === "session_updated") {
        snapshot = {
          ...snapshot,
          session: event,
          chat: { ...snapshot.chat, status: event.status },
        };
      }
      fresh.push({ seq, turnId: null, event });
    });
    this.#expected = Math.max(this.#expected, batch.seq_start + batch.events.length);
    if (fresh.length === 0) return;
    this.#entries.push(...fresh);
    // Folded now, drawn on the next frame with whatever else arrived before it.
    if (this.#fresh && this.#fresh.chatId === chat.id) {
      this.#fresh.entries.push(...fresh);
      this.#fresh.snapshot = snapshot;
    } else {
      this.#fresh = { chatId: chat.id, entries: fresh, snapshot };
    }
    this.#paint ??= nextPaint(() => {
      this.#paint = null;
      this.#paintNow();
    });
  }

  /** Draws the live entries not drawn yet. */
  #paintNow() {
    const fresh = this.#fresh;
    this.#fresh = null;
    this.#paint?.();
    this.#paint = null;
    const chat = this.#state.chat;
    if (!fresh || !chat || chat.id !== fresh.chatId) return;
    this.#set({
      chat: {
        ...chat,
        snapshot: {
          ...fresh.snapshot,
          // A status update may have arrived since: the chat's summary keeps the newer one.
          chat: { ...chat.snapshot.chat, status: fresh.snapshot.chat.status },
        },
        transcript: applyEntries(chat.transcript, fresh.entries),
      },
    });
  }

  /** Reads the page of history before what is shown. */
  async loadEarlier(): Promise<void> {
    const chat = this.#state.chat;
    if (!chat || this.#before === null || chat.loadingEarlier) return;
    this.#set({ chat: { ...chat, loadingEarlier: true } });
    try {
      const page = (await this.#request({
        op: "history",
        chat_id: chat.id,
        before_seq: this.#before,
      })) as HistoryPage;
      this.#paintNow();
      const current = this.#state.chat;
      if (!current || current.id !== chat.id) return;
      this.#entries = compactToolUpdates([...entriesOf(page), ...this.#entries]);
      this.#cut = page.cut_turn_id;
      this.#before = page.has_more ? page.before_seq : null;
      this.#set({
        chat: {
          ...current,
          transcript: withTasks(
            applyEntries(emptyTranscript(this.#cut), this.#entries),
            current.snapshot.tasks ?? [],
          ),
          hasMore: this.#before !== null,
          loadingEarlier: false,
        },
      });
    } catch (error) {
      const current = this.#state.chat;
      if (current?.id === chat.id) this.#set({ chat: { ...current, loadingEarlier: false } });
      throw error;
    }
  }

  send(chatId: string, text: string): Promise<PromptAck> {
    return this.#request({ op: "send", chat_id: chatId, text }, ACT_MS) as Promise<PromptAck>;
  }

  async interrupt(chatId: string): Promise<void> {
    await this.#request({ op: "interrupt", chat_id: chatId }, ACT_MS);
  }

  /** Answers a permission request; no option declines it. */
  async permission(chatId: string, requestId: string, optionId: string | null): Promise<void> {
    await this.#request(
      { op: "permission", chat_id: chatId, request_id: requestId, option_id: optionId },
      ACT_MS,
    );
  }

  /** Answers a question; no answer dismisses it. */
  async answer(chatId: string, requestId: string, answer: UiAnswer | null): Promise<void> {
    await this.#request({ op: "answer", chat_id: chatId, request_id: requestId, answer }, ACT_MS);
  }

  /** Starts a chat in a project's folder with its first message, and opens it. */
  async create(folderId: string, agentId: string | null, text: string): Promise<void> {
    const created = (await this.#request(
      {
        op: "create",
        folder_id: folderId,
        agent_id: agentId,
        text,
      },
      SLOW_REQUEST_MS,
    )) as { chat: WorkspaceChat };
    await this.refresh().catch(() => {});
    await this.open(created.chat.chat.id);
  }

  /** The models and thinking levels a chat offers, and what it is set to. */
  options(chatId: string): Promise<Choices> {
    return this.#request({ op: "options", chat_id: chatId }) as Promise<Choices>;
  }

  async setModel(chatId: string, model: string): Promise<void> {
    await this.#request({ op: "set_model", chat_id: chatId, model });
  }

  async setThinking(chatId: string, level: string): Promise<void> {
    await this.#request({ op: "set_thinking", chat_id: chatId, level });
  }

  /**
   * Hands a chat to another agent, as the app's own handoff does, and opens the chat it
   * continues in.
   */
  async switchAgent(chatId: string, agentId: string): Promise<void> {
    const switched = (await this.#request(
      {
        op: "switch_agent",
        chat_id: chatId,
        agent_id: agentId,
      },
      SLOW_REQUEST_MS,
    )) as { chat_id: string };
    await this.refresh().catch(() => {});
    this.#opening = null;
    await this.open(switched.chat_id);
  }

  /** Deletes a computer's key from this browser, without asking the computer anything. */
  async discard(hostId: string): Promise<void> {
    await deleteHost(hostId);
    const hosts = this.#state.hosts.filter((entry) => entry.hostId !== hostId);
    this.#set({ hosts });
    if (hosts[0]) this.use(hosts[0].hostId);
    else this.#set({ phase: { name: "empty" } });
  }

  /** Unpairs this browser: the computer forgets the device and this browser its key. */
  async forget(): Promise<void> {
    const credential = this.#credential;
    if (credential?.kind !== "device") return;
    const host = credential.host;
    // Best effort there; here the key goes whatever the computer says.
    await this.#request({ op: "forget" }).catch(() => {});
    await deleteHost(host.hostId);
    const hosts = this.#state.hosts.filter((entry) => entry.hostId !== host.hostId);
    this.#set({ hosts });
    if (hosts[0]) this.use(hosts[0].hostId);
    else {
      this.#credential = null;
      this.#forgetChat();
      this.#close();
      this.#set({
        phase: { name: "empty" },
        host: null,
        overview: null,
        chat: null,
        opening: null,
      });
    }
  }
}

/**
 * Asks a computer to forget one of this browser's old devices, over a session of that device's
 * own: after the browser paired with the computer again, so its Settings lists it once. Best
 * effort: a key the computer no longer takes, or a computer that is away, leaves things as they
 * were.
 */
async function forgetDevice(origin: string, host: SavedHost): Promise<void> {
  const socket = new WebSocket(`${origin}/api/remote/v1/client/${host.hostId}`);
  const nonce = randomBytes(16);
  let channel: Channel | null = null;
  let ready = false;
  await new Promise<void>((done) => {
    const finish = () => {
      clearTimeout(timer);
      socket.onmessage = socket.onclose = socket.onerror = null;
      try {
        socket.close();
      } catch {
        // Already closing.
      }
      done();
    };
    const timer = setTimeout(finish, REQUEST_MS);
    socket.onclose = finish;
    socket.onerror = finish;
    // The welcome and the sealed "ready" come back to back, and opening one waits on the keys the
    // other derives: messages are handled strictly one after another, as the session's own are.
    let inbox: Promise<void> = Promise.resolve();
    socket.onmessage = (event) => {
      const text = typeof event.data === "string" ? event.data : "";
      inbox = inbox.then(() => handle(text));
    };
    const handle = async (text: string) => {
      try {
        if (text === "h1") {
          const hello = { v: VERSION, kind: "device", id: host.deviceId, nonce: encode(nonce) };
          socket.send(`mH${JSON.stringify(hello)}`);
        } else if (text === "h0" || text.startsWith("mR")) {
          finish();
        } else if (text.startsWith("mW") && !channel) {
          const welcome = JSON.parse(text.slice(2)) as { nonce?: unknown };
          const hostNonce = typeof welcome.nonce === "string" ? decode(welcome.nonce, 16) : null;
          if (!hostNonce) return finish();
          channel = await Channel.create(
            "client",
            host.key,
            nonce,
            hostNonce,
            `device|${host.deviceId}`,
          );
        } else if (text.startsWith("mE") && channel) {
          const message = JSON.parse(await channel.open(text.slice(2))) as { ev?: unknown };
          if (!ready && message.ev === "ready") {
            ready = true;
            socket.send(`mE${await channel.seal(JSON.stringify({ id: 1, op: "forget" }))}`);
          } else if (ready) {
            finish();
          }
        }
      } catch {
        finish();
      }
    };
  });
}

function entriesOf(page: HistoryPage): Entry[] {
  return page.items.map((item) => ({ seq: item.seq, turnId: item.turn_id, event: item.event }));
}
