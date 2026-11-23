/**
 * The remote-sessions relay. A desktop keeps one outbound WebSocket here (its "host" socket);
 * paired browsers and bots reach it through this Worker, so the desktop opens no port.
 *
 * - A browser's traffic is sealed end to end between the browser and the desktop
 *   (docs/remote-sessions.md). The relay forwards opaque text and cannot read or forge it.
 * - A bot speaks MCP over HTTPS, so its calls are readable here in passing. The relay turns each
 *   tool call into one request to the desktop, which checks the bot's token itself.
 *
 * One Durable Object per desktop holds its sockets. Its only stored state is the hash of the
 * desktop's key and the hashes of its bot tokens.
 */
import { DurableObject } from "cloudflare:workers";
import TOOLS from "./tools.json";
import FIRST_TOOLS from "./tools-first.json";

/**
 * A tool as its file describes it. `full` is this relay's own mark, on a tool or on one of its
 * arguments: only a full-control link is shown it. `since` is the first set of tools the tool
 * is in (2 when it says nothing): a copy of the app that answers an earlier set is not shown it.
 * Neither is part of what MCP lists.
 */
interface Tool {
  readonly name: string;
  readonly full?: boolean;
  readonly since?: number;
  readonly inputSchema: {
    readonly properties?: Record<string, { readonly full?: boolean; readonly since?: number }>;
    readonly [key: string]: unknown;
  };
  readonly [key: string]: unknown;
}
interface Listed {
  readonly name: string;
  readonly [key: string]: unknown;
}

/** The tools as MCP lists them to a link of one level, without this relay's marks. */
function listed(tools: readonly Tool[], wide: boolean, version = 2): Listed[] {
  return tools
    .filter((tool) => (wide || !tool.full) && (tool.since ?? 2) <= version)
    .map(({ full: _full, since: _since, inputSchema, ...tool }) => ({
      ...tool,
      inputSchema: {
        ...inputSchema,
        properties: Object.fromEntries(
          Object.entries(inputSchema.properties ?? {})
            .filter(([, argument]) => (wide || !argument.full) && (argument.since ?? 2) <= version)
            .map(([name, { full: _mark, since: _since, ...argument }]) => [name, argument]),
        ),
      },
    }));
}

/**
 * The sets of tools. A copy of the app answers the set it was built with and says which in its
 * bot links (`v`): the first one (a copy older than the number), or 2 and up, which list the
 * current tools a copy that old answers (a tool's `since`). A bot is shown the set its computer
 * answers, so the relay and the app can be updated apart. A copy newer than this relay is shown
 * the newest set the relay knows.
 */
const FIRST = {
  chat: listed(FIRST_TOOLS as unknown as readonly Tool[], false),
  full: listed(FIRST_TOOLS as unknown as readonly Tool[], true),
  instructions:
    "o7 runs coding agents (Claude Code, Codex and others) on a person's computer. These tools reach the chats on that computer while o7 is open there with remote access on. Start with list_chats. After send_message or start_chat, call wait_for_chat until the agent finishes or asks for something, and answer waiting requests with respond_to_request. Agents there can edit files and run commands, so send only what the person asked for.",
};
/** What each set's bot is told first, by the set's number. */
const INSTRUCTIONS: Record<number, string> = {
  2: "o7 runs coding agents (Claude Code, Codex and others) on a person's computer; these tools reach its chats while o7 is open there. Hand the agent a whole task in one send and let it work: it plans, edits and tests by itself. send and wait already return the turn's result, so do not read a chat you just heard from, and do not ask more often than wait does. Agents edit files and run commands, so send only what the person asked for.",
  3: "o7 runs coding agents (Claude Code, Codex and others) on a person's computer; these tools reach its chats while o7 is open there. Hand the agent a whole task in one send and let it work: it plans, edits and tests by itself. send and wait already return the turn's result, with the commands that ran and what still fails, so do not read a chat you just heard from, and do not ask more often than wait does. When you come back, inbox says what needs you in one call. Agents edit files and run commands, so send only what the person asked for.",
  4: "o7 runs coding agents (Claude Code, Codex and others) on a person's computer, on the person's own subscriptions; these tools reach its chats while o7 is open there. For new work, call task once: o7 starts an agent on its own branch, has it plan, build, check and commit, and has a second agent review it, then tells you when it is all done if this link has a call back. Then end your turn; do not wait or poll. send continues a chat, and like wait returns the turn's result with the commands that ran and what still fails. When you come back, inbox says what needs you in one call. Agents edit files and run commands, so hand over only what the person asked for.",
  5: "o7 runs coding agents (Claude Code, Codex and others) on a person's computer, on the person's own subscriptions; these tools reach its chats while o7 is open there. For new work, call task once: o7 starts an agent on its own branch, has it plan, build, check and commit, and has a second agent review it, then tells you when it is all done if this link has a call back. Then end your turn; do not wait or poll. send continues a chat, and like wait returns the turn's result with the commands that ran and what still fails. When you come back, inbox says what needs you in one call. Where look is listed, it shows a chat's page or emulator as a picture, when seeing the work matters. Agents edit files and run commands, so hand over only what the person asked for.",
  6: "o7 runs coding agents (Claude Code, Codex and others) on a person's computer, on the person's own subscriptions; these tools reach its chats while o7 is open there. For new work, call task once: o7 starts an agent on its own branch, has it plan, build, check and commit, and has a second agent review it, then tells you when it is all done if this link has a call back. Then end your turn; do not wait or poll. Work that can wait goes to task with when tonight. send continues a chat, and like wait returns the turn's result with the commands that ran and what still fails. When you come back, inbox says what needs you in one call. Where look is listed, it shows a chat's page or emulator as a picture, when seeing the work matters. Agents edit files and run commands, so hand over only what the person asked for.",
};
/** The newest set this relay knows. */
const NEWEST_TOOLS = 6;
interface ToolSet {
  readonly chat: Listed[];
  readonly full: Listed[];
  readonly instructions: string;
}
const SETS = new Map<number, ToolSet>();
/** The set a link of version `v` is shown. */
function setFor(v: number): ToolSet {
  if (v < 2) return FIRST;
  const version = Math.min(Math.floor(v), NEWEST_TOOLS);
  const kept = SETS.get(version);
  if (kept) return kept;
  const made: ToolSet = {
    chat: listed(TOOLS as unknown as readonly Tool[], false, version),
    full: listed(TOOLS as unknown as readonly Tool[], true, version),
    instructions: INSTRUCTIONS[version] ?? INSTRUCTIONS[NEWEST_TOOLS] ?? "",
  };
  SETS.set(version, made);
  return made;
}

/**
 * The tools a read-only link is shown: those that only read (`readOnlyHint`). The computer
 * refuses anything else from such a link whatever is listed; this keeps the list honest.
 */
function readOnly(tools: readonly Listed[]): Listed[] {
  return tools.filter(
    (tool) => (tool.annotations as { readOnlyHint?: unknown } | undefined)?.readOnlyHint === true,
  );
}
const READ_INSTRUCTIONS =
  "o7 runs coding agents (Claude Code, Codex and others) on a person's computer; this link reads its chats while o7 is open there, and cannot send, answer or change anything. inbox says what needs the person in one call; read and wait return what a chat did.";

/** The numbers links from before `versions` were kept are known by. */
const CURRENT_TOOLS = 2;
const LATEST_TOOLS = 3;

export interface Env {
  readonly ROOMS: DurableObjectNamespace<Room>;
  /** The website's origin: the origin a browser socket comes from. */
  readonly APP_ORIGIN: string;
  /**
   * An origin the website had before, still let in while the remote page is served there for
   * browsers paired before the move. Unset once that page is gone.
   */
  readonly FORMER_ORIGIN?: string;
  /** Tests only: how long a desktop may go without a keep-alive, in ms (`HOST_SILENT_MS`). */
  readonly HOST_SILENT_MS?: string;
}

const PREFIX = "/api/remote/v1/";
/** 16 random bytes, base64url. */
const HOST_ID = /^[A-Za-z0-9_-]{22}$/;
/** 32 random bytes, base64url. */
const SECRET = /^[A-Za-z0-9_-]{43}$/;
/** 8 random bytes, base64url. */
const BOT_ID = /^[A-Za-z0-9_-]{11}$/;
const CID_LENGTH = 16;

/** Browsers the desktop has accepted: each proved it holds a pairing or device key. */
const MAX_PROVEN = 16;
/**
 * Browsers that have not proved anything yet. Knowing a desktop's id is not a credential, so
 * these sockets are few, and a newcomer takes the place of the oldest: nobody can hold the door
 * shut against a paired device by opening sockets.
 */
const MAX_WAITING = 8;
const MAX_WAITING_PER_ADDRESS = 3;
/** How long an accepted browser keeps its place after the desktop reconnects, to prove itself again. */
const REPROVE_MS = 30_000;
const MAX_BOTS = 32;
/** A browser's largest message: a prompt of a few thousand words, sealed. */
const CLIENT_MESSAGE_BYTES = 96 * 1024;
/** A desktop's largest message: one page of a chat's history, sealed. */
const HOST_MESSAGE_BYTES = 2 * 1024 * 1024;
const CLIENT_MESSAGES_PER_WINDOW = 240;
const WINDOW_MS = 10_000;
const MCP_BODY_BYTES = 256 * 1024;
/**
 * Until the desktop accepts it, a browser's socket has shown no key: it gets the handshake's few
 * small messages and nothing more, so knowing a computer's id sends that computer next to nothing.
 */
const UNPROVEN_MESSAGE_BYTES = 2048;
const UNPROVEN_MESSAGES = 4;
/** A room no desktop has connected to for this long forgets what it stored. */
const IDLE_ROOM_MS = 30 * 24 * 60 * 60 * 1000;
const HOST_ID_LABEL = "auracode remote v1 host id";
const MAX_BOT_CALLS = 8;
/**
 * The desktop says "ping" every 25 seconds. A socket that has said nothing for three of those
 * belongs to a computer that slept or lost its network without closing it: it is treated as gone,
 * so a browser is told its computer is away instead of waiting on a socket nobody reads.
 */
const HOST_SILENT_MS = 75_000;
/** How often, at most, the time the desktop was last heard is written beside its socket. */
const HEARD_NOTE_MS = 5_000;
/**
 * How long the desktop has to answer one tool call. A call may wait for an agent's turn, for up
 * to 200 seconds when the bot asks (`WAIT_MAX` in crates/o7-remote/src/bot/lean.rs), so that a
 * long turn costs a bot a few calls and not one every half minute.
 */
const BOT_CALL_MS = 250_000;

const MCP_VERSIONS = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/** How long ago `ms` was, in words a bot can pass on. */
function ago(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "less than a minute ago";
  if (minutes < 60) return minutes === 1 ? "a minute ago" : `${minutes} minutes ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return hours === 1 ? "an hour ago" : `${hours} hours ago`;
  return `${Math.round(hours / 24)} days ago`;
}

/** A call that found no computer to go to: nothing happened. `seen` is when it was last there. */
function offline(seen: number | undefined): string {
  const when = seen === undefined ? "" : ` It was last connected ${ago(Date.now() - seen)}.`;
  return `Nothing was done: the computer is not reachable. It is asleep or offline, o7 is closed there, or remote access is off.${when} Tell your person; it answers again once o7 runs there with remote access on.`;
}

/** The computer went away while a call was with it, so whether it happened is not known. */
const WENT_AWAY =
  "The computer went away during this call, so it is not known whether it was done. Once it is back, read the chat before trying again.";

const HEADERS = { "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff" };
const CORS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "POST, OPTIONS",
  "Access-Control-Allow-Headers":
    "Authorization, Content-Type, Accept, Mcp-Protocol-Version, Mcp-Session-Id",
  "Access-Control-Max-Age": "86400",
};

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...HEADERS, "Content-Type": "application/json", ...headers },
  });
}

const notFound = (headers: Record<string, string> = {}) =>
  json(404, { error: "not_found" }, headers);

/**
 * A computer's id is the start of a hash of its key, so the key is the only thing that can claim
 * the id: nobody can take an id first, or take it over if the room's storage is ever lost, and a
 * request that fails here never reaches a room.
 */
async function hostIdOf(key: string): Promise<string | null> {
  if (!SECRET.test(key)) return null;
  let raw: Uint8Array;
  try {
    raw = Uint8Array.from(atob(key.replaceAll("-", "+").replaceAll("_", "/")), (char) =>
      char.charCodeAt(0),
    );
  } catch {
    return null;
  }
  if (raw.length !== 32) return null;
  const label = new TextEncoder().encode(HOST_ID_LABEL);
  const input = new Uint8Array(label.length + raw.length);
  input.set(label);
  input.set(raw, label.length);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", input)).slice(0, 16);
  return btoa(String.fromCharCode(...digest))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
}

/** The address a limit counts by: an IPv6 network, since one subscriber holds a whole /64. */
function network(address: string): string {
  if (!address.includes(":")) return address;
  const [head = "", tail] = address.toLowerCase().split("::");
  const groups = head.split(":").filter(Boolean);
  // "::" stands for the zero groups left out; only the first four groups matter here.
  while (groups.length < 4 && tail !== undefined) groups.push("0");
  return groups.slice(0, 4).join(":");
}

function isUpgrade(request: Request): boolean {
  return request.method === "GET" && request.headers.get("Upgrade")?.toLowerCase() === "websocket";
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (!url.pathname.startsWith(PREFIX)) return notFound();
    const [kind, hostId] = url.pathname.slice(PREFIX.length).split("/");
    if (kind === "health" && hostId === undefined)
      return json(200, {
        ok: true,
        protocol: 1,
        tools: setFor(NEWEST_TOOLS).chat.length,
        full_tools: setFor(NEWEST_TOOLS).full.length,
        second_tools: setFor(2).chat.length,
        second_full_tools: setFor(2).full.length,
        first_tools: FIRST.chat.length,
        first_full_tools: FIRST.full.length,
      });
    if (!hostId || !HOST_ID.test(hostId)) return notFound();
    const origin = request.headers.get("Origin");
    if (kind === "host") {
      // Only the desktop app holds a host key, and it is not a browser.
      if (!isUpgrade(request) || origin !== null) return json(400, { error: "invalid_request" });
      const key = /^Bearer (.+)$/.exec(request.headers.get("Authorization") ?? "")?.[1] ?? "";
      const own = await hostIdOf(key);
      if (own === null || !same(own, hostId)) return json(401, { error: "unauthorized" });
    } else if (kind === "client") {
      if (!isUpgrade(request)) return json(400, { error: "invalid_request" });
      // An exact match against the one or two origins this deployment names, never a pattern.
      const known = !!origin && (origin === env.APP_ORIGIN || origin === env.FORMER_ORIGIN);
      if (!known) return json(403, { error: "forbidden" });
    } else if (kind === "mcp") {
      if (request.method === "OPTIONS") return new Response(null, { status: 204, headers: CORS });
    } else {
      return notFound();
    }
    return env.ROOMS.get(env.ROOMS.idFromName(hostId)).fetch(request);
  },
} satisfies ExportedHandler<Env>;

type Attachment =
  | {
      readonly role: "host";
      /** When the socket opened, then when it last sent something, to within a few seconds. */
      readonly since?: number;
      /** Ended for going silent; browsers were already told, so its close says nothing more. */
      readonly silent?: boolean;
    }
  | {
      readonly role: "client";
      readonly cid: string;
      /** When the socket opened, and the address it came from. */
      readonly since: number;
      readonly from: string;
      /** The desktop accepted its session. */
      readonly proven: boolean;
      /** Messages sent before the desktop accepted it. Kept here, so it outlives hibernation. */
      readonly sent?: number;
      /** Set when the desktop reconnected: accepted only until then unless it accepts again. */
      readonly until?: number;
      /** This side ended the socket and the desktop already knows. */
      readonly told?: boolean;
    };
type ClientAttachment = Extract<Attachment, { role: "client" }>;

interface BotReply {
  readonly text: string;
  readonly error: boolean;
  /** A picture that goes with the text (`look`). */
  readonly image?: { readonly mime: "image/png" | "image/jpeg"; readonly data: string };
}

/** The largest picture an answer may carry, in base64: the desktop sends at most this. */
const PICTURE_BYTES = 1536 * 1024;
const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;

/** The picture the desktop sent with an answer, when it is one MCP can carry: a PNG or a JPEG. */
function picture(value: unknown): BotReply["image"] {
  if (value === null || typeof value !== "object") return undefined;
  const { mime, data } = value as { mime?: unknown; data?: unknown };
  if (mime !== "image/png" && mime !== "image/jpeg") return undefined;
  if (typeof data !== "string" || data.length === 0 || data.length > PICTURE_BYTES)
    return undefined;
  return BASE64.test(data) ? { mime, data } : undefined;
}

interface RpcMessage {
  readonly jsonrpc?: unknown;
  readonly id?: unknown;
  readonly method?: unknown;
  readonly params?: unknown;
}

function randomId(bytes: number): string {
  const raw = crypto.getRandomValues(new Uint8Array(bytes));
  return btoa(String.fromCharCode(...raw))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
}

async function sha256(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return btoa(String.fromCharCode(...new Uint8Array(digest)))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
}

/** Whether the desktop's current socket accepted a browser, or still may within its grace time. */
function accepted(is: ClientAttachment): boolean {
  return is.proven && (is.until === undefined || is.until > Date.now());
}

/** Compares two hashes without stopping at the first difference. */
function same(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let index = 0; index < a.length; index += 1)
    diff |= a.charCodeAt(index) ^ b.charCodeAt(index);
  return diff === 0;
}

export class Room extends DurableObject<Env> {
  /** Bot calls waiting for the desktop's answer. A waiting call keeps this object awake. */
  readonly #calls = new Map<string, (reply: BotReply) => void>();
  /**
   * The calls a bot may cancel: its link and the request's own id, to the call waiting under it.
   * A bot that gives up on a long wait says so (`notifications/cancelled`); without this its
   * call held one of the computer's few places until the desktop answered or the time ran out.
   */
  readonly #cancellable = new Map<string, string>();
  /** Messages each browser sent in the current window. Lost on hibernation, which only helps. */
  readonly #rates = new Map<string, { since: number; count: number }>();
  /**
   * When the desktop last sent anything but a keep-alive. A computer pushing a long history up a
   * slow line may have its next keep-alive waiting behind that data: it is plainly there. Also
   * written to the socket's attachment, at most every few seconds, so it outlives hibernation.
   */
  #heard = 0;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Keep-alives are answered without waking the object.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  override async fetch(request: Request): Promise<Response> {
    const [kind, , reference] = new URL(request.url).pathname.slice(PREFIX.length).split("/");
    if (kind === "host") return this.#acceptHost(request);
    if (kind === "client") return this.#acceptClient(request);
    if (kind === "mcp") return this.#mcp(request, reference ?? "");
    return notFound();
  }

  /**
   * The desktop's socket, while it is open and its keep-alives still arrive. When it has gone
   * silent, every browser but `asking` is told it is away; the caller answers `asking` itself.
   */
  #host(asking?: WebSocket): WebSocket | null {
    const host =
      this.ctx.getWebSockets("host").find((socket) => socket.readyState === WebSocket.OPEN) ?? null;
    if (!host) return null;
    const noted = (host.deserializeAttachment() as { since?: number } | null)?.since;
    const pinged = this.ctx.getWebSocketAutoResponseTimestamp(host)?.getTime();
    // A socket from before this was counted has no time at all: it is left alone.
    if (noted === undefined && pinged === undefined) return host;
    const silent = Number(this.env.HOST_SILENT_MS) || HOST_SILENT_MS;
    const heard = Math.max(noted ?? 0, pinged ?? 0, this.#heard);
    if (Date.now() - heard <= silent) return host;
    host.serializeAttachment({ role: "host", silent: true } satisfies Attachment);
    end(host, 4004, "silent");
    this.#hostGone(host, asking, heard);
    return null;
  }

  #clients(): WebSocket[] {
    return this.ctx
      .getWebSockets("client")
      .filter((socket) => socket.readyState === WebSocket.OPEN);
  }

  /**
   * The desktop's socket. The Worker already checked that its key is the one this room's id is
   * made from; here the room only notes that it has an owner.
   */
  async #acceptHost(_request: Request): Promise<Response> {
    if ((await this.ctx.storage.get("host")) === undefined)
      await this.ctx.storage.put("host", true);
    await this.ctx.storage.deleteAlarm();

    const earlier = this.ctx.getWebSockets("host");
    // Browsers that still had a session with the desktop start over; a socket that went silent
    // already told them it was gone.
    const replacing = earlier.some(
      (socket) =>
        socket.readyState === WebSocket.OPEN &&
        !(socket.deserializeAttachment() as { silent?: boolean } | null)?.silent,
    );
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["host"]);
    pair[1].serializeAttachment({ role: "host", since: Date.now() } satisfies Attachment);
    for (const socket of earlier) end(socket, 4000, "replaced");
    // The new socket knows none of the open browsers: each starts its session again.
    for (const client of this.#clients()) {
      const attachment = client.deserializeAttachment() as Attachment;
      if (attachment.role !== "client") continue;
      // A browser accepted before keeps its place while it starts again, for a short time:
      // the new socket has to accept it too.
      if (attachment.proven)
        client.serializeAttachment({
          ...attachment,
          until: Date.now() + REPROVE_MS,
        } satisfies Attachment);
      if (replacing) client.send("h0");
      client.send("h1");
      pair[1].send(`o${attachment.cid}`);
    }
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  async #acceptClient(request: Request): Promise<Response> {
    // An id no desktop ever claimed has nothing behind it.
    if ((await this.ctx.storage.get("host")) === undefined) return notFound();
    const from = network(request.headers.get("CF-Connecting-IP") ?? "");
    const waiting = this.#clients()
      .map((socket) => ({ socket, is: socket.deserializeAttachment() as ClientAttachment }))
      .filter((entry) => !accepted(entry.is))
      .sort((a, b) => a.is.since - b.is.since);
    const fromHere = waiting.filter((entry) => entry.is.from === from);
    const leaving =
      fromHere.length >= MAX_WAITING_PER_ADDRESS
        ? fromHere[0]
        : waiting.length >= MAX_WAITING
          ? waiting[0]
          : undefined;
    if (leaving) {
      end(leaving.socket, 4002, "unproven");
      this.#host()?.send(`c${leaving.is.cid}`);
    }
    const cid = randomId(12);
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["client", `c:${cid}`]);
    pair[1].serializeAttachment({
      role: "client",
      cid,
      since: Date.now(),
      from,
      proven: false,
    } satisfies Attachment);
    const host = this.#host(pair[1]);
    pair[1].send(host ? "h1" : "h0");
    host?.send(`o${cid}`);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  override async webSocketMessage(socket: WebSocket, message: string | ArrayBuffer) {
    if (typeof message !== "string") return socket.close(1003, "text only");
    const attachment = socket.deserializeAttachment() as Attachment;
    if (attachment.role === "host") return this.#fromHost(socket, message);
    if (message.length > CLIENT_MESSAGE_BYTES) return socket.close(1009, "too large");
    if (!message.startsWith("m")) return socket.close(1008, "unexpected");
    if (!accepted(attachment)) {
      const sent = (attachment.sent ?? 0) + 1;
      socket.serializeAttachment({ ...attachment, sent } satisfies Attachment);
      if (message.length > UNPROVEN_MESSAGE_BYTES || sent > UNPROVEN_MESSAGES)
        return socket.close(1008, "prove a key first");
    }
    if (!this.#allowed(attachment.cid)) return socket.close(1008, "too many messages");
    const host = this.#host(socket);
    if (host) host.send(`m${attachment.cid} ${message.slice(1)}`);
    else socket.send("h0");
  }

  /** Nobody has used this room for a long time: forget its bot links and its owner's mark. */
  override async alarm() {
    if (this.#host() === null) await this.ctx.storage.deleteAll();
  }

  #allowed(cid: string): boolean {
    const now = Date.now();
    const rate = this.#rates.get(cid);
    if (!rate || now - rate.since > WINDOW_MS) {
      this.#rates.set(cid, { since: now, count: 1 });
      return true;
    }
    rate.count += 1;
    return rate.count <= CLIENT_MESSAGES_PER_WINDOW;
  }

  async #fromHost(socket: WebSocket, message: string) {
    if (message.length > HOST_MESSAGE_BYTES) return socket.close(1009, "too large");
    const now = Date.now();
    if (now - this.#heard > HEARD_NOTE_MS)
      socket.serializeAttachment({ role: "host", since: now } satisfies Attachment);
    this.#heard = now;
    const kind = message[0];
    if (kind === "m" || kind === "c" || kind === "a") {
      const cid = message.slice(1, 1 + CID_LENGTH);
      const client = this.ctx.getWebSockets(`c:${cid}`)[0];
      if (!client || client.readyState !== WebSocket.OPEN) {
        if (kind !== "c") socket.send(`c${cid}`);
      } else if (kind === "a") {
        // The desktop opened a sealed message from this browser: it holds a key.
        const is = client.deserializeAttachment() as ClientAttachment;
        if (is.proven && is.until === undefined) return;
        const proven = this.#clients().filter(
          (other) =>
            other !== client && accepted(other.deserializeAttachment() as ClientAttachment),
        ).length;
        if (proven >= MAX_PROVEN) {
          end(client, 4003, "too many");
          socket.send(`c${cid}`);
        } else {
          client.serializeAttachment({
            ...is,
            proven: true,
            until: undefined,
          } satisfies Attachment);
        }
      } else if (kind === "m") {
        client.send(`m${message.slice(2 + CID_LENGTH)}`);
      } else {
        end(client, 4001, "closed");
      }
      return;
    }
    let body: unknown;
    try {
      body = JSON.parse(message.slice(1));
    } catch {
      return socket.close(1008, "unexpected");
    }
    if (kind === "l") return this.#saveBots(body);
    if (kind === "b") {
      const reply = body as { rid?: unknown; text?: unknown; error?: unknown; image?: unknown };
      if (typeof reply.rid !== "string") return;
      this.#calls.get(reply.rid)?.({
        text: typeof reply.text === "string" ? reply.text : "",
        error: reply.error === true,
        image: picture(reply.image),
      });
      return;
    }
    socket.close(1008, "unexpected");
  }

  /**
   * The desktop's current bot links, as `[{ id, hash, full, v }]`: what a bot's URL is checked
   * against, whether it is shown the full-control tools, and which set of tools its computer
   * answers. The desktop checks each call's level itself; the marks here only keep the list a
   * bot is shown honest.
   */
  async #saveBots(body: unknown) {
    const bots: Record<string, string> = {};
    const full: string[] = [];
    const current: string[] = [];
    const latest: string[] = [];
    const read: string[] = [];
    const versions: Record<string, number> = {};
    if (Array.isArray(body))
      for (const entry of body.slice(0, MAX_BOTS)) {
        const {
          id,
          hash,
          full: wide,
          read: reading,
          v,
        } = (entry ?? {}) as {
          id?: unknown;
          hash?: unknown;
          full?: unknown;
          read?: unknown;
          v?: unknown;
        };
        if (
          typeof id === "string" &&
          BOT_ID.test(id) &&
          typeof hash === "string" &&
          SECRET.test(hash)
        ) {
          bots[id] = hash;
          // A read-only link is never shown more, whatever else it says.
          if (reading === true) read.push(id);
          else if (wide === true) full.push(id);
          // A copy newer than this relay answers at least the latest set this relay knows.
          if (typeof v === "number" && v >= CURRENT_TOOLS) current.push(id);
          if (typeof v === "number" && v >= LATEST_TOOLS) latest.push(id);
          versions[id] = typeof v === "number" && Number.isFinite(v) ? v : 1;
        }
      }
    await this.ctx.storage.put({ bots, full, current, latest, read, versions });
  }

  override async webSocketClose(socket: WebSocket, code: number, reason: string) {
    this.#left(socket);
    try {
      socket.close(code === 1005 || code === 1006 ? 1000 : code, reason);
    } catch {
      // Already closed.
    }
  }

  override async webSocketError(socket: WebSocket) {
    this.#left(socket);
  }

  #left(socket: WebSocket) {
    const attachment = socket.deserializeAttachment() as Attachment | null;
    if (attachment?.role === "client") {
      this.#rates.delete(attachment.cid);
      if (!attachment.told) this.#host()?.send(`c${attachment.cid}`);
      return;
    }
    if ((attachment as { silent?: boolean } | null)?.silent) return;
    this.#hostGone(socket);
  }

  /** The desktop's socket `socket` closed or went silent; it was last heard `heard`. */
  #hostGone(socket: WebSocket, except?: WebSocket, heard = Date.now()) {
    // A replaced host socket closes after its successor opened: the desktop is still here.
    const successor = this.ctx
      .getWebSockets("host")
      .some((other) => other !== socket && other.readyState === WebSocket.OPEN);
    if (successor) return;
    // The desktop is gone: if it stays gone, what the room stored for it goes too.
    void this.ctx.storage.setAlarm(Date.now() + IDLE_ROOM_MS);
    // When it was last here, for a bot told it is away. Gone with the room.
    void this.ctx.storage.put("seen", heard);
    for (const client of this.#clients()) if (client !== except) client.send("h0");
    for (const finish of [...this.#calls.values()]) finish({ text: WENT_AWAY, error: true });
  }

  async #mcp(request: Request, reference: string): Promise<Response> {
    const [id = "", inPath = ""] = reference.split(".");
    const token =
      inPath || (/^Bearer (.+)$/.exec(request.headers.get("Authorization") ?? "")?.[1] ?? "");
    const bots = (await this.ctx.storage.get<Record<string, string>>("bots")) ?? {};
    const known = BOT_ID.test(id) && Object.hasOwn(bots, id) ? bots[id] : undefined;
    // A wrong or removed link does not exist; nothing here invites a sign-in flow.
    if (!known || !SECRET.test(token) || !same(known, await sha256(token))) return notFound(CORS);
    if (request.method !== "POST")
      return json(405, { error: "method_not_allowed" }, { ...CORS, Allow: "POST, OPTIONS" });
    const text = await request.text();
    if (text.length > MCP_BODY_BYTES) return json(413, { error: "too_large" }, CORS);
    let body: unknown;
    try {
      body = JSON.parse(text);
    } catch {
      return json(400, rpcError(null, -32700, "Parse error"), CORS);
    }
    const batch = Array.isArray(body);
    const messages = (batch ? body : [body]) as RpcMessage[];
    if (messages.length === 0 || messages.length > 16)
      return json(400, rpcError(null, -32600, "Invalid request"), CORS);
    const wide = ((await this.ctx.storage.get<string[]>("full")) ?? []).includes(id);
    const reading = ((await this.ctx.storage.get<string[]>("read")) ?? []).includes(id);
    // A link's set by its number; one kept before numbers were stored, by the lists then kept.
    const version =
      (await this.ctx.storage.get<Record<string, number>>("versions"))?.[id] ??
      (((await this.ctx.storage.get<string[]>("latest")) ?? []).includes(id)
        ? 3
        : ((await this.ctx.storage.get<string[]>("current")) ?? []).includes(id)
          ? 2
          : 1);
    const set = setFor(version);
    const replies: unknown[] = [];
    for (const message of messages) {
      const reply = await this.#rpc(
        message,
        id,
        token,
        reading ? readOnly(set.chat) : wide ? set.full : set.chat,
        reading ? READ_INSTRUCTIONS : set.instructions,
      );
      if (reply !== null) replies.push(reply);
    }
    // Notifications and responses get no body.
    if (replies.length === 0) return new Response(null, { status: 202, headers: CORS });
    return json(200, batch ? replies : replies[0], CORS);
  }

  async #rpc(
    message: RpcMessage,
    bot: string,
    token: string,
    tools: readonly Listed[],
    instructions: string,
  ): Promise<unknown> {
    if (message === null || typeof message !== "object" || message.jsonrpc !== "2.0")
      return rpcError(null, -32600, "Invalid request");
    const id = message.id;
    if (typeof message.method !== "string") return null;
    const wantsReply = typeof id === "string" || typeof id === "number";
    const params = (message.params ?? {}) as Record<string, unknown>;
    if (!wantsReply) {
      // Only a link's own waiting call is ended, and only by the id that call was made with.
      const asked = params.requestId;
      if (
        message.method === "notifications/cancelled" &&
        (typeof asked === "string" || typeof asked === "number")
      ) {
        const rid = this.#cancellable.get(cancelKey(bot, asked));
        if (rid !== undefined)
          this.#calls.get(rid)?.({ text: "Cancelled. The chat carries on.", error: true });
      }
      return null;
    }
    switch (message.method) {
      case "initialize": {
        const asked = params.protocolVersion;
        return rpcResult(id, {
          protocolVersion:
            typeof asked === "string" && MCP_VERSIONS.includes(asked) ? asked : MCP_VERSIONS[0],
          capabilities: { tools: { listChanged: false } },
          serverInfo: { name: "o7", title: "o7", version: "2.0.0" },
          instructions,
        });
      }
      case "ping":
        return rpcResult(id, {});
      case "tools/list":
        return rpcResult(id, { tools });
      case "tools/call": {
        const name = params.name;
        const args = params.arguments ?? {};
        // A tool outside this link's level does not exist for it.
        if (typeof name !== "string" || !tools.some((tool) => tool.name === name))
          return rpcError(id, -32602, "Unknown tool");
        if (args === null || typeof args !== "object" || Array.isArray(args))
          return rpcError(id, -32602, "Tool arguments must be an object");
        const reply = await this.#call({ bot, token, tool: name, args }, cancelKey(bot, id));
        // Only look shows a picture, and only a full-control link is listed look.
        const shown = name === "look" ? reply.image : undefined;
        return rpcResult(id, {
          content: shown
            ? [
                { type: "text", text: reply.text },
                { type: "image", data: shown.data, mimeType: shown.mime },
              ]
            : [{ type: "text", text: reply.text }],
          isError: reply.error,
        });
      }
      default:
        return rpcError(id, -32601, "Method not found");
    }
  }

  /** One tool call, answered by the desktop. */
  #call(
    request: { bot: string; token: string; tool: string; args: unknown },
    cancel: string,
  ): Promise<BotReply> {
    const host = this.#host();
    if (!host)
      return this.ctx.storage
        .get<number>("seen")
        .then((seen) => ({ text: offline(seen), error: true }));
    if (this.#calls.size >= MAX_BOT_CALLS)
      return Promise.resolve({
        text: "Too many calls are waiting on this computer. Wait for one to finish.",
        error: true,
      });
    const rid = randomId(9);
    return new Promise<BotReply>((resolve) => {
      const finish = (reply: BotReply) => {
        clearTimeout(timer);
        this.#calls.delete(rid);
        if (this.#cancellable.get(cancel) === rid) this.#cancellable.delete(cancel);
        resolve(reply);
      };
      const timer = setTimeout(
        () => finish({ text: "The computer did not answer in time. Try again.", error: true }),
        BOT_CALL_MS,
      );
      this.#calls.set(rid, finish);
      this.#cancellable.set(cancel, rid);
      try {
        host.send(`b${JSON.stringify({ rid, ...request })}`);
      } catch {
        finish({ text: offline(Date.now()), error: true });
      }
    });
  }
}

/**
 * Ends a socket from this side. The last message says so in words, because a peer can see the
 * closing code late or not at all.
 */
function end(socket: WebSocket, code: number, reason: string) {
  try {
    const attachment = socket.deserializeAttachment() as Attachment | null;
    if (attachment?.role === "client")
      socket.serializeAttachment({ ...attachment, told: true } satisfies Attachment);
    socket.send("x");
    socket.close(code, reason);
  } catch {
    // Already closed.
  }
}

/** A waiting call's name for cancelling: ids are a link's own, and a number is not its string. */
function cancelKey(bot: string, id: string | number) {
  return `${bot} ${typeof id} ${id}`;
}

function rpcResult(id: unknown, result: unknown) {
  return { jsonrpc: "2.0", id, result };
}

function rpcError(id: unknown, code: number, message: string) {
  return { jsonrpc: "2.0", id, error: { code, message } };
}
