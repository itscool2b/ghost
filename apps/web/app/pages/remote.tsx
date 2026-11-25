import { AgentMark, Logo, PixelLoader, StatusDot } from "@o7/ui/graphite";
import {
  type FormEvent,
  memo,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import type {
  PermissionOption,
  SessionStatus,
  UiAnswer,
  UiPrompt,
  UiQuestion,
  WorkspaceChat,
} from "../../../desktop/src/bindings";
import {
  type RenderedItem,
  type RenderedSegment,
  renderCache,
  stopReasonLabel,
} from "../../../desktop/src/lib/chat/transcript";
import {
  type Look,
  type OpenChat,
  type Overview,
  parsePairing,
  RemoteClient,
  type State,
} from "../remote/client";
import { takePairingLink } from "../remote/link";
import { Markdown } from "../remote/markdown";
import { ChatPickers } from "../remote/pickers";
import { keepsPairings, loadHosts, type SavedHost } from "../remote/storage";
// The desktop's themes, one per agent: the page wears the one its computer wears.
import "@o7/ui/themes.css";
import "../styles/remote.css";

export const meta = () => [
  { title: "Remote · o7" },
  { name: "robots", content: "noindex" },
  { name: "referrer", content: "no-referrer" },
];

/** The relay is this site. A page served from this machine may name another, for development. */
function relayOrigin(): string {
  const local = location.hostname === "localhost" || location.hostname === "127.0.0.1";
  const named = local ? new URLSearchParams(location.search).get("relay") : null;
  if (named && /^wss?:\/\/(localhost|127\.0\.0\.1):\d+$/.test(named)) return named;
  return `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}`;
}

/** o7's own default, until the computer says otherwise. */
const DEFAULT_LOOK: Look = { theme: "claude", appearance: "system" };

/**
 * Puts the computer's theme on the page: its palette, its appearance (the system's when it follows
 * the system, followed live), and the browser's own bars in the window's colour.
 */
function useLook(look: Look | null) {
  const { theme, appearance } = look ?? DEFAULT_LOOK;
  useLayoutEffect(() => {
    const root = document.documentElement;
    const query = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => {
      root.dataset.palette = theme;
      root.dataset.theme =
        appearance === "system" ? (query.matches ? "light" : "dark") : appearance;
      const paint = getComputedStyle(root).getPropertyValue("--g-window").trim();
      for (const tag of document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]'))
        if (paint) tag.content = paint;
    };
    apply();
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  }, [theme, appearance]);
  useEffect(
    () => () => {
      // Leaving the page gives the site its own appearance back.
      delete document.documentElement.dataset.palette;
      delete document.documentElement.dataset.theme;
    },
    [],
  );
}

/** A keyboard takes a good part of the screen; the browser's own bars coming and going do not. */
const KEYBOARD_PX = 120;

/**
 * Keeps the page the size and place of what the phone shows. An on-screen keyboard covers the
 * page's foot without resizing it (iOS), and the browser then scrolls the whole page to reveal the
 * field, which hides the header and leaves dead space under the message box. The page takes the
 * visible area's height and top instead, so the message box sits on the keyboard.
 */
function useVisibleArea() {
  useLayoutEffect(() => {
    const area = window.visualViewport;
    if (!area) return;
    const root = document.documentElement;
    const sync = () => {
      // Zoomed in with two fingers, the visible area is a part of the page: leave the page be.
      if (area.scale > 1.01) return;
      root.style.setProperty("--remote-height", `${area.height}px`);
      root.style.setProperty("--remote-top", `${area.offsetTop}px`);
      root.toggleAttribute("data-remote-keyboard", window.innerHeight - area.height > KEYBOARD_PX);
    };
    sync();
    area.addEventListener("resize", sync);
    area.addEventListener("scroll", sync);
    return () => {
      area.removeEventListener("resize", sync);
      area.removeEventListener("scroll", sync);
      root.style.removeProperty("--remote-height");
      root.style.removeProperty("--remote-top");
      root.removeAttribute("data-remote-keyboard");
    };
  }, []);
}

export default function RemotePage() {
  useVisibleArea();
  const [client, setClient] = useState<RemoteClient | null>(null);
  // A pairing link waits for a tap. Opening the page pairs nothing by itself, so a program that
  // only opens links (a mail or chat scanner) cannot spend the link or become a device.
  const [asking, setAsking] = useState<string | null>(null);
  // Whether the client has started: a link on a fresh page waits for the tap before anything
  // connects, and "Not now" then connects to what is already paired.
  const started = useRef(false);
  useEffect(() => {
    const next = new RemoteClient(relayOrigin());
    const link = takePairingLink(window);
    if (link === "") {
      started.current = true;
      void next.start("");
    } else setAsking(link);
    setClient(next);
    // A link can also arrive while the page is open: a scanned code or a pasted link changes only
    // the part after the `#`, which reloads nothing. It asks the same question over whatever is on
    // screen.
    const arrived = () => {
      const fresh = takePairingLink(window);
      if (fresh !== "") setAsking(fresh);
    };
    // Back in front, back online, or restored from the back-forward cache: check the session.
    const wake = () => {
      if (document.visibilityState === "visible") next.wake();
    };
    const shown = () => {
      arrived();
      wake();
    };
    window.addEventListener("hashchange", arrived);
    document.addEventListener("visibilitychange", wake);
    window.addEventListener("online", wake);
    window.addEventListener("focus", wake);
    window.addEventListener("pageshow", shown);
    return () => {
      window.removeEventListener("hashchange", arrived);
      document.removeEventListener("visibilitychange", wake);
      window.removeEventListener("online", wake);
      window.removeEventListener("focus", wake);
      window.removeEventListener("pageshow", shown);
      next.stop();
    };
  }, []);
  return (
    <main id="main" className="remote">
      {client && asking !== null ? (
        <Themed client={client}>
          <PairQuestion
            link={asking}
            client={client}
            onPair={() => {
              setAsking(null);
              started.current = true;
              void client.pair(asking);
            }}
            onNotNow={() => {
              setAsking(null);
              // On a fresh page nothing has connected yet; on a page in use, nothing changes.
              if (!started.current) {
                started.current = true;
                void client.start("");
              }
            }}
          />
        </Themed>
      ) : client ? (
        <Themed client={client}>
          <Remote client={client} />
        </Themed>
      ) : (
        <Centered title="Remote">
          <p>Connecting to your computer.</p>
          <noscript>
            <p>This page needs JavaScript: it encrypts everything in your browser.</p>
          </noscript>
        </Centered>
      )}
    </main>
  );
}

/**
 * Asks before a pairing link is used. A computer this browser is already paired with is named: the
 * link pairs it again, with a new key that replaces the old one.
 */
function PairQuestion({
  link,
  client,
  onPair,
  onNotNow,
}: {
  readonly link: string;
  readonly client: RemoteClient;
  readonly onPair: () => void;
  readonly onNotNow: () => void;
}) {
  const state = useSyncExternalStore(client.subscribe, client.getState, client.getState);
  const hostId = parsePairing(link)?.hostId ?? null;
  const [saved, setSaved] = useState<readonly SavedHost[]>(state.hosts);
  useEffect(() => {
    let live = true;
    void loadHosts().then((hosts) => {
      if (live) setSaved(hosts);
    });
    return () => {
      live = false;
    };
  }, []);
  const known = hostId ? saved.find((host) => host.hostId === hostId) : undefined;
  return (
    <Centered title={known ? "Pair this browser again?" : "Pair this browser?"}>
      {known ? (
        <p>
          This browser is already paired with {known.hostName}. Pairing again gives it a new key
          there, and the old one stops working.
        </p>
      ) : (
        <p>
          This link pairs this browser with your computer. Once paired, it can read your chats there
          and direct your agents, which can edit files and run commands.
        </p>
      )}
      <p>Go on only if you opened this link from your own computer just now.</p>
      <p className="remote-card-actions">
        <button type="button" className="g-button g-button-primary remote-button" onClick={onPair}>
          {known ? "Pair again" : "Pair this browser"}
        </button>
        <button type="button" className="g-button remote-button remote-quiet" onClick={onNotNow}>
          Not now
        </button>
      </p>
    </Centered>
  );
}

function Themed({
  client,
  children,
}: {
  readonly client: RemoteClient;
  readonly children: ReactNode;
}) {
  const state = useSyncExternalStore(client.subscribe, client.getState, client.getState);
  useLook(state.look);
  return children;
}

function Remote({ client }: { readonly client: RemoteClient }) {
  const state = useSyncExternalStore(client.subscribe, client.getState, client.getState);
  if (state.phase.name === "empty")
    return (
      <Centered title="Reach your computer from here.">
        <p>
          In o7 on your computer, open Settings, then Remote access, and choose Pair. Scan the code
          it shows with this device, or open its link here.
        </p>
        <p>
          <a className="g-button remote-button remote-quiet" href="/download">
            Get o7
          </a>
        </p>
      </Centered>
    );
  if (state.phase.name === "ended") {
    const ended = state.phase;
    const back = ended.back ? state.hosts.find((host) => host.hostId === ended.back) : undefined;
    return (
      <Centered title="This pairing is over.">
        <p role="status">{ended.reason}</p>
        {ended.refused ? (
          <Refused client={client} hostId={ended.refused} />
        ) : back ? (
          <p>
            <button
              type="button"
              className="g-button g-button-primary remote-button"
              onClick={() => client.use(back.hostId)}
            >
              Back to {back.hostName}
            </button>
          </p>
        ) : state.hosts.length > 0 ? (
          <p>
            <button
              type="button"
              className="g-button g-button-primary remote-button"
              onClick={() => client.use(state.hosts[0]?.hostId ?? "")}
            >
              Open {state.hosts[0]?.hostName}
            </button>
          </p>
        ) : null}
      </Centered>
    );
  }
  if (state.chat || state.opening) return <ChatScreen client={client} state={state} />;
  return <ListScreen client={client} state={state} />;
}

/** A computer turned this browser away: try again, or delete its key here. */
function Refused({ client, hostId }: { readonly client: RemoteClient; readonly hostId: string }) {
  return (
    <p className="remote-card-actions">
      <button
        type="button"
        className="g-button g-button-primary remote-button"
        onClick={() => client.use(hostId)}
      >
        Try again
      </button>
      <button
        type="button"
        className="g-button remote-button remote-quiet"
        onClick={() => void client.discard(hostId)}
      >
        Forget this computer
      </button>
    </p>
  );
}

function Centered({ title, children }: { readonly title: string; readonly children: ReactNode }) {
  return (
    <div className="remote-centered">
      <a className="remote-mark" href="/" aria-label="o7">
        <Logo size={64} />
      </a>
      <h1>{title}</h1>
      {children}
    </div>
  );
}

/** Where the session stands, in a word or two. */
function linkWords(state: State): { readonly text: string; readonly tone: "ok" | "wait" | "away" } {
  switch (state.phase.name) {
    case "ready":
      return { text: "Connected", tone: "ok" };
    case "away":
      return { text: "Computer is away", tone: "away" };
    default:
      return { text: "Connecting", tone: "wait" };
  }
}

/** The connection's state as a dot and its words; a loader while it connects. */
function LinkState({ state }: { readonly state: State }) {
  const link = linkWords(state);
  return (
    <p className="remote-link" data-tone={link.tone} role="status">
      {link.tone === "wait" ? (
        <PixelLoader size={10} tempo="tool" />
      ) : (
        <span className="remote-link-dot" aria-hidden="true" />
      )}
      {link.text}
    </p>
  );
}

type Tone = "working" | "attention" | "idle" | "failed";

function statusWords(chat: WorkspaceChat["chat"]): { readonly text: string; readonly tone: Tone } {
  if (chat.needs_attention) return { text: "Needs you", tone: "attention" };
  const words: Record<SessionStatus, readonly [string, Tone]> = {
    running: ["Working", "working"],
    starting: ["Starting", "working"],
    cancelling: ["Stopping", "working"],
    stopping: ["Stopping", "working"],
    ready: ["Idle", "idle"],
    stopped: ["Stopped", "idle"],
    failed: ["Failed", "failed"],
    auth_required: ["Needs sign-in on the computer", "failed"],
  };
  const [text, tone] = words[chat.status];
  return { text, tone };
}

/** An agent's mark with the chat's state in its place, as the desktop's sidebar shows it. */
function Mark({ chat, size }: { readonly chat: WorkspaceChat["chat"]; readonly size: number }) {
  const { tone } = statusWords(chat);
  const state =
    tone === "attention"
      ? "waiting"
      : tone === "working"
        ? "tool"
        : tone === "failed"
          ? "error"
          : "idle";
  return (
    <AgentMark
      agent={chat.agent_id}
      size={size}
      state={state}
      label={`${chat.agent_name}, ${statusWords(chat).text}`}
    />
  );
}

function ago(at: number, now: number): string {
  const minutes = Math.floor((now - at) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return days < 30
    ? `${days}d`
    : new Date(at).toLocaleDateString(undefined, { dateStyle: "medium" });
}

const message = (error: unknown): string =>
  error instanceof Error && error.message ? error.message : "Something went wrong.";

function ListScreen({ client, state }: { readonly client: RemoteClient; readonly state: State }) {
  const [creating, setCreating] = useState(false);
  const [menu, setMenu] = useState(false);
  const [error, setError] = useState("");
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);
  const overview = state.overview;
  const groups = useMemo(() => {
    if (!overview) return [];
    return overview.projects
      .map((project) => ({
        project,
        chats: overview.chats.filter((entry) => entry.project_id === project.id && !entry.archived),
      }))
      .filter((group) => group.chats.length > 0)
      .sort((a, b) => (b.chats[0]?.chat.updated_at ?? 0) - (a.chats[0]?.chat.updated_at ?? 0));
  }, [overview]);
  const waiting = useMemo(
    () => overview?.chats.filter((entry) => !entry.archived && entry.chat.needs_attention) ?? [],
    [overview],
  );
  const projectName = (id: string) =>
    overview?.projects.find((project) => project.id === id)?.name ?? "";
  const folders = overview?.projects.flatMap((project) => project.folders) ?? [];
  const open = (id: string) => {
    setError("");
    client.open(id).catch((failure) => setError(message(failure)));
  };
  const row = (chat: WorkspaceChat["chat"], project?: string) => {
    const status = statusWords(chat);
    return (
      <li key={chat.id}>
        <button
          type="button"
          className="remote-chat-row"
          data-tone={status.tone}
          onClick={() => open(chat.id)}
        >
          <Mark chat={chat} size={28} />
          <span className="remote-chat-text">
            <strong>{chat.title || "Untitled chat"}</strong>
            <span>
              {chat.agent_name} · {status.text}
              {project ? ` · ${project}` : ""}
            </span>
          </span>
          <time dateTime={new Date(chat.updated_at).toISOString()}>
            {ago(chat.updated_at, now)}
          </time>
        </button>
      </li>
    );
  };
  return (
    <div className="remote-screen remote-list-screen">
      <header className="remote-bar">
        <a className="remote-mark" href="/" aria-label="o7">
          <Logo size={30} />
        </a>
        <div className="remote-bar-title">
          <h1>{state.host?.name || overview?.host_name || "Your computer"}</h1>
          <LinkState state={state} />
        </div>
        <button
          type="button"
          className="remote-icon"
          aria-label="This computer"
          aria-expanded={menu}
          onClick={() => setMenu(!menu)}
        >
          <Dots />
        </button>
      </header>
      {menu ? <HostMenu client={client} state={state} onClose={() => setMenu(false)} /> : null}
      <div className="remote-list">
        {error ? (
          <p className="remote-error" role="alert">
            {error}
          </p>
        ) : null}
        {state.phase.name === "away" ? (
          <p className="remote-banner">
            Your computer is not answering. Open o7 there, and check it is awake and online. This
            page connects by itself when it is back.
          </p>
        ) : null}
        {overview && groups.length === 0 ? (
          <p className="remote-hint">
            {folders.length > 0
              ? "No chats yet. Start one below."
              : "No projects yet. Add one in o7 on your computer."}
          </p>
        ) : null}
        {!overview && state.phase.name !== "away" ? <ListSkeleton /> : null}
        {waiting.length > 0 ? (
          <section aria-labelledby="remote-waiting">
            <h2 id="remote-waiting">Needs you</h2>
            <ul className="remote-chats">
              {waiting.map((entry) => row(entry.chat, projectName(entry.project_id)))}
            </ul>
          </section>
        ) : null}
        {groups.map(({ project, chats }) => (
          <section key={project.id} aria-labelledby={`remote-project-${project.id}`}>
            <h2 id={`remote-project-${project.id}`}>{project.name}</h2>
            <ul className="remote-chats">
              {chats.filter((entry) => !entry.chat.needs_attention).map((entry) => row(entry.chat))}
            </ul>
          </section>
        ))}
      </div>
      {overview && folders.length > 0 ? (
        <div className="remote-dock">
          <button
            type="button"
            className="g-button g-button-primary remote-button remote-new"
            disabled={state.phase.name !== "ready"}
            onClick={() => setCreating(true)}
          >
            <Plus />
            New chat
          </button>
        </div>
      ) : null}
      {creating && overview ? (
        <NewChat client={client} overview={overview} onClose={() => setCreating(false)} />
      ) : null}
    </div>
  );
}

/** Where the chats will be, while the computer reads them. */
function ListSkeleton() {
  return (
    <div className="remote-skeleton" role="status" aria-label="Reading your chats">
      {[0, 1, 2, 3].map((index) => (
        <span
          key={index}
          className="remote-skeleton-row"
          style={{ animationDelay: `${index * 90}ms` }}
        />
      ))}
    </div>
  );
}

function HostMenu({
  client,
  state,
  onClose,
}: {
  readonly client: RemoteClient;
  readonly state: State;
  readonly onClose: () => void;
}) {
  const others = state.hosts.filter((host) => host.hostId !== state.host?.id);
  return (
    <div className="remote-menu">
      {others.map((host) => (
        <button
          key={host.hostId}
          type="button"
          className="g-button remote-button remote-quiet"
          onClick={() => {
            onClose();
            client.use(host.hostId);
          }}
        >
          Switch to {host.hostName}
        </button>
      ))}
      <button
        type="button"
        className="g-button remote-button remote-quiet remote-danger"
        onClick={() => {
          onClose();
          void client.forget();
        }}
      >
        Forget this computer
      </button>
      <p>
        Forgetting unpairs this browser. Its key is deleted here and stops working there.
        {keepsPairings() ? "" : " This browser keeps pairings only until the page closes."}
      </p>
    </div>
  );
}

/** A sheet that rises from the bottom: a modal dialog, closed by Escape or a tap outside. */
function Sheet({
  busy = false,
  onClose,
  children,
}: {
  readonly busy?: boolean;
  readonly onClose: () => void;
  readonly children: (titleId: string) => ReactNode;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const id = useId();
  const closing = useRef({ busy, onClose });
  closing.current = { busy, onClose };
  useEffect(() => {
    const node = dialog.current;
    node?.showModal();
    // A tap on the backdrop, outside the sheet. Escape closes it from the keyboard (onCancel).
    const tapped = (event: MouseEvent) => {
      if (event.target === node && !closing.current.busy) closing.current.onClose();
    };
    node?.addEventListener("click", tapped);
    return () => {
      node?.removeEventListener("click", tapped);
      node?.close();
    };
  }, []);
  return (
    <dialog
      ref={dialog}
      className="remote-dialog"
      aria-labelledby={id}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      {children(id)}
    </dialog>
  );
}

function NewChat({
  client,
  overview,
  onClose,
}: {
  readonly client: RemoteClient;
  readonly overview: Overview;
  readonly onClose: () => void;
}) {
  const folders = overview.projects.flatMap((project) =>
    project.folders.map((folder) => ({ project, folder })),
  );
  const agents = overview.agents.filter((agent) => agent.available);
  const [folder, setFolder] = useState(folders[0]?.folder.id ?? "");
  const [agent, setAgent] = useState("");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const ids = useId();
  const project = folders.find((entry) => entry.folder.id === folder)?.project;
  const usual = project?.default_agent ?? overview.default_agent;
  const usualName = agents.find((entry) => entry.id === usual)?.name;
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!text.trim() || busy) return;
    setBusy(true);
    setError("");
    try {
      await client.create(folder, agent || null, text);
      onClose();
    } catch (failure) {
      setError(message(failure));
      setBusy(false);
    }
  };
  return (
    <Sheet busy={busy} onClose={onClose}>
      {(titleId) => (
        <form onSubmit={(event) => void submit(event)}>
          <h2 id={titleId}>New chat</h2>
          <label htmlFor={`${ids}-folder`}>Folder</label>
          <select
            id={`${ids}-folder`}
            className="g-select"
            value={folder}
            onChange={(event) => setFolder(event.currentTarget.value)}
          >
            {folders.map(({ project: owner, folder: entry }) => (
              <option key={entry.id} value={entry.id}>
                {owner.name === entry.display_name
                  ? entry.display_name
                  : `${owner.name} · ${entry.display_name}`}
              </option>
            ))}
          </select>
          <label htmlFor={`${ids}-agent`}>Agent</label>
          <select
            id={`${ids}-agent`}
            className="g-select"
            value={agent}
            onChange={(event) => setAgent(event.currentTarget.value)}
          >
            <option value="">{usualName ? `${usualName} (usual)` : "The usual one"}</option>
            {agents
              .filter((entry) => entry.id !== usual)
              .map((entry) => (
                <option key={entry.id} value={entry.id}>
                  {entry.name}
                </option>
              ))}
          </select>
          <label htmlFor={`${ids}-text`}>Message</label>
          <textarea
            id={`${ids}-text`}
            className="g-textarea"
            rows={4}
            value={text}
            placeholder="What should the agent do?"
            onChange={(event) => setText(event.currentTarget.value)}
          />
          {error ? (
            <p className="remote-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="remote-dialog-actions">
            <button
              type="button"
              className="g-button remote-button remote-quiet"
              disabled={busy}
              onClick={onClose}
            >
              Cancel
            </button>
            <button
              type="submit"
              className="g-button g-button-primary remote-button"
              disabled={busy || !text.trim()}
            >
              {busy ? "Starting" : "Start chat"}
            </button>
          </div>
        </form>
      )}
    </Sheet>
  );
}

/** A message sent from here and not yet in the chat: queued behind a turn, or on its way. */
interface Sending {
  readonly id: number;
  readonly text: string;
  /** How many of the chat's messages read the same when it was sent. */
  readonly before: number;
  readonly queued: boolean;
}

/** What the person sent, without the project memory o7 attaches after it. */
const MEMORY = "\n\nProject memory (Aura reference context, revision ";
function ownWords(text: string): string {
  const at = text.indexOf(MEMORY);
  return at >= 0 && text.slice(at).includes("\nUse this as project background.")
    ? text.slice(0, at)
    : text;
}

/** How many of the chat's own messages read `text`. */
function sentCount(chat: OpenChat | null, text: string): number {
  if (!chat) return 0;
  let count = 0;
  for (const item of chat.transcript.items.values())
    if (item.kind === "user" && ownWords(item.text).trim() === text.trim()) count += 1;
  return count;
}

let sendingIds = 0;
const nextSending = () => {
  sendingIds += 1;
  return sendingIds;
};

function ChatScreen({ client, state }: { readonly client: RemoteClient; readonly state: State }) {
  const chat = state.chat;
  const summary =
    chat?.snapshot.chat ??
    state.overview?.chats.find((entry) => entry.chat.id === state.opening)?.chat ??
    null;
  const status = summary ? statusWords(summary) : null;
  // A turn is under way, whether the agent is working or waiting on an answer.
  const running = summary ? ["running", "starting", "cancelling"].includes(summary.status) : false;
  const working = running && !summary?.needs_attention;
  const [error, setError] = useState("");
  const [picking, setPicking] = useState(false);
  const [sending, setSending] = useState<readonly Sending[]>([]);
  // A message leaves "sending" once the chat shows it.
  useEffect(() => {
    setSending((current) => {
      const left = current.filter((entry) => sentCount(chat, entry.text) <= entry.before);
      return left.length === current.length ? current : left;
    });
  }, [chat]);
  const ready = state.phase.name === "ready";
  // Offered only by an o7 that answers it: an older one ends the session on the request.
  const pickers = state.features.includes("pickers") && ready;
  const send = async (text: string) => {
    if (!chat) return;
    setError("");
    const entry: Sending = {
      id: nextSending(),
      text,
      before: sentCount(chat, text),
      queued: false,
    };
    setSending((current) => [...current, entry]);
    try {
      const ack = await client.send(chat.id, text);
      if (ack.queued)
        setSending((current) =>
          current.map((item) => (item.id === entry.id ? { ...item, queued: true } : item)),
        );
    } catch (failure) {
      setSending((current) => current.filter((item) => item.id !== entry.id));
      throw failure;
    }
  };
  return (
    <div className="remote-screen remote-chat-screen">
      <header className="remote-bar">
        <button
          type="button"
          className="remote-icon"
          aria-label="All chats"
          onClick={() => client.close()}
        >
          <Back />
        </button>
        {summary ? <Mark chat={summary} size={24} /> : null}
        <div className="remote-bar-title">
          <h1>{summary?.title || "Untitled chat"}</h1>
          {ready ? (
            <p className="remote-link" data-tone={status?.tone ?? "idle"} role="status">
              {[summary?.agent_name, status?.text].filter(Boolean).join(" · ")}
            </p>
          ) : (
            <LinkState state={state} />
          )}
        </div>
        {chat && summary && pickers ? (
          <button
            type="button"
            className="remote-icon"
            aria-label="Model and agent"
            aria-haspopup="dialog"
            onClick={() => setPicking(true)}
          >
            <Sliders />
          </button>
        ) : null}
      </header>
      {picking && chat && summary ? (
        <ChatPickers
          client={client}
          chatId={chat.id}
          agentId={summary.agent_id}
          agentName={summary.agent_name}
          agents={state.overview?.agents ?? []}
          running={running}
          onClose={() => setPicking(false)}
        />
      ) : null}
      {chat ? (
        <ChatBody
          client={client}
          chat={chat}
          working={working}
          sending={sending}
          error={error}
          onError={setError}
        />
      ) : (
        <div className="remote-transcript">
          <div className="remote-column">
            <p className="remote-opening" role="status">
              <PixelLoader size={12} tempo="tool" />
              {ready ? "Opening the chat." : `Waiting for ${state.host?.name || "your computer"}.`}
            </p>
          </div>
        </div>
      )}
      {chat ? (
        <Composer
          key={chat.id}
          disabled={!ready}
          working={running}
          onSend={send}
          onStop={() => {
            setError("");
            client.interrupt(chat.id).catch((failure) => setError(message(failure)));
          }}
          onError={setError}
        />
      ) : null}
    </div>
  );
}

function ChatBody({
  client,
  chat,
  working,
  sending,
  error,
  onError,
}: {
  readonly client: RemoteClient;
  readonly chat: OpenChat;
  readonly working: boolean;
  readonly sending: readonly Sending[];
  readonly error: string;
  readonly onError: (message: string) => void;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  /** Whether the reader is at the newest words: only then does new text pull the view down. */
  const pinned = useRef(true);
  /** While older messages load: how far from the bottom the reader was, to keep their place. */
  const anchor = useRef<number | null>(null);
  const [below, setBelow] = useState(false);
  const alive = chat.snapshot.chat.status !== "stopped" && chat.snapshot.chat.status !== "failed";
  // Each turn keeps its rendering until it changes, so a streamed word redraws one turn.
  const [cache] = useState(renderCache);
  const segments = useMemo(() => cache(chat.transcript, alive), [cache, chat.transcript, alive]);
  useLayoutEffect(() => {
    const node = scroller.current;
    if (!node) return;
    if (anchor.current !== null && !chat.loadingEarlier) {
      node.scrollTop = node.scrollHeight - anchor.current;
      anchor.current = null;
      return;
    }
    if (pinned.current) node.scrollTop = node.scrollHeight;
    else setBelow(true);
  });
  // The keyboard opening makes the transcript shorter: a reader at the newest words stays there.
  useEffect(() => {
    const node = scroller.current;
    if (!node || typeof ResizeObserver === "undefined") return;
    const watch = new ResizeObserver(() => {
      if (pinned.current) node.scrollTop = node.scrollHeight;
    });
    watch.observe(node);
    return () => watch.disconnect();
  }, []);
  const onPermission = useCallback(
    (request: string, option: string | null) => {
      onError("");
      client.permission(chat.id, request, option).catch((failure) => onError(message(failure)));
    },
    [client, chat.id, onError],
  );
  const onAnswer = useCallback(
    (request: string, answer: UiAnswer | null) => {
      onError("");
      client.answer(chat.id, request, answer).catch((failure) => onError(message(failure)));
    },
    [client, chat.id, onError],
  );
  return (
    <div className="remote-transcript-wrap">
      <div
        className="remote-transcript"
        ref={scroller}
        onScroll={(event) => {
          const node = event.currentTarget;
          pinned.current = node.scrollHeight - node.scrollTop - node.clientHeight < 80;
          if (pinned.current) setBelow(false);
        }}
      >
        <div className="remote-column">
          {chat.hasMore ? (
            <button
              type="button"
              className="g-button remote-button remote-quiet remote-earlier"
              disabled={chat.loadingEarlier}
              onClick={() => {
                const node = scroller.current;
                pinned.current = false;
                anchor.current = node ? node.scrollHeight - node.scrollTop : null;
                onError("");
                client.loadEarlier().catch((failure) => {
                  anchor.current = null;
                  onError(message(failure));
                });
              }}
            >
              {chat.loadingEarlier ? "Loading" : "Earlier messages"}
            </button>
          ) : null}
          {segments.length === 0 && sending.length === 0 ? (
            <p className="remote-hint">Nothing here yet. Send a message.</p>
          ) : null}
          {segments.map((segment) => (
            <Turn
              key={segment.key}
              segment={segment}
              onPermission={onPermission}
              onAnswer={onAnswer}
            />
          ))}
          {sending.map((entry) => (
            <div
              key={entry.id}
              className="remote-sending"
              data-queued={entry.queued ? "" : undefined}
            >
              <p className="remote-pending">{entry.text}</p>
              <span>{entry.queued ? "Queued until this turn ends" : "Sending"}</span>
            </div>
          ))}
          {working ? <WorkingLine /> : null}
          {error ? (
            <p className="remote-error" role="alert">
              {error}
            </p>
          ) : null}
        </div>
      </div>
      {below ? (
        <button
          type="button"
          className="remote-latest"
          onClick={() => {
            const node = scroller.current;
            pinned.current = true;
            setBelow(false);
            node?.scrollTo({ top: node.scrollHeight, behavior: "smooth" });
          }}
        >
          <Down />
          Latest
        </button>
      ) : null}
    </div>
  );
}

/** What the desktop's working line says while a turn runs: one verb at a time, and the time. */
const VERBS = ["Working", "Thinking", "Reading", "Writing", "Checking"];
function WorkingLine() {
  const [started] = useState(() => Date.now());
  const [now, setNow] = useState(started);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const seconds = Math.floor((now - started) / 1000);
  const verb = VERBS[Math.floor(seconds / 4) % VERBS.length] ?? "Working";
  return (
    <p className="remote-working" role="status" aria-label="Working">
      <span className="remote-working-dot" aria-hidden="true" />
      <span className="remote-working-verb" key={verb}>
        {verb}
      </span>
      {seconds > 0 ? (
        <span className="remote-working-time">
          {seconds < 60 ? `${seconds}s` : `${Math.floor(seconds / 60)}m ${seconds % 60}s`}
        </span>
      ) : null}
    </p>
  );
}

type Tool = Extract<RenderedItem, { kind: "tool" }>;

/** One turn. Kept while its rendering is the same object, so other turns' words skip it. */
const Turn = memo(function Turn({
  segment,
  onPermission,
  onAnswer,
}: {
  readonly segment: RenderedSegment;
  readonly onPermission: (requestId: string, optionId: string | null) => void;
  readonly onAnswer: (requestId: string, answer: UiAnswer | null) => void;
}) {
  // A run of steps between two pieces of the agent's words folds into one line.
  const rows: (RenderedItem | readonly Tool[])[] = [];
  for (const item of segment.items) {
    const last = rows[rows.length - 1];
    if (item.kind === "tool" && Array.isArray(last)) rows[rows.length - 1] = [...last, item];
    else rows.push(item.kind === "tool" ? [item] : item);
  }
  const ended = segment.end && segment.end.stopReason !== "end_turn" ? segment.end : null;
  return (
    <div className="remote-turn">
      {rows.map((row) => {
        if (Array.isArray(row)) return <Steps key={row[0]?.key} tools={row} />;
        const item = row as Exclude<RenderedItem, Tool>;
        switch (item.kind) {
          case "user":
            return (
              <p key={item.key} className="remote-user">
                {ownWords(item.text)}
              </p>
            );
          case "message":
            return <Markdown key={item.key} text={item.text} />;
          case "permission":
            return (
              <PermissionCard
                key={item.key}
                title={item.request.title}
                options={item.request.options}
                chosen={
                  item.resolution === null
                    ? undefined
                    : item.resolution.decision.type === "selected"
                      ? item.resolution.decision.option_id
                      : null
                }
                onChoose={(option) => onPermission(item.request.request_id, option)}
              />
            );
          case "question":
            return (
              <QuestionCard
                key={item.key}
                prompt={item.request.prompt}
                resolved={item.resolution !== null}
                onAnswer={(answer) => onAnswer(item.request.request_id, answer)}
              />
            );
          case "error":
            return (
              <p key={item.key} className="remote-error">
                {item.message}
              </p>
            );
          case "notice":
            return (
              <p key={item.key} className="remote-notice">
                {item.message}
              </p>
            );
          case "gap":
            return (
              <p key={item.key} className="remote-notice">
                Part of this turn is still being saved on the computer.
              </p>
            );
        }
        return null;
      })}
      {ended ? <p className="remote-notice remote-ended">{stopReasonLabel(ended)}</p> : null}
    </div>
  );
});

function Steps({ tools }: { readonly tools: readonly Tool[] }) {
  const last = tools[tools.length - 1];
  const running = tools.some(
    (tool) => tool.display === "pending" || tool.display === "in_progress",
  );
  const failed = tools.filter((tool) => tool.display === "failed").length;
  const name = (tool: Tool) => tool.title || tool.toolName || "Step";
  return (
    <details className="remote-steps" data-running={running ? "" : undefined}>
      <summary>
        <Chevron />
        <span className="remote-steps-name">
          {tools.length === 1 ? name(tools[0] as Tool) : `${tools.length} steps`}
          {tools.length > 1 && running && last ? `: ${name(last)}` : ""}
        </span>
        <span className="remote-steps-state">
          {running ? (
            <PixelLoader size={10} tempo="tool" label="running" />
          ) : failed > 0 ? (
            `${failed} failed`
          ) : (
            ""
          )}
        </span>
      </summary>
      <ul>
        {tools.map((tool) => (
          <li key={tool.key} data-state={tool.display}>
            {name(tool)}
          </li>
        ))}
      </ul>
    </details>
  );
}

function PermissionCard({
  title,
  options,
  chosen,
  onChoose,
}: {
  readonly title: string;
  readonly options: readonly PermissionOption[];
  /** `undefined` while it waits; the chosen option, or `null` when it was declined. */
  readonly chosen: string | null | undefined;
  readonly onChoose: (optionId: string | null) => void;
}) {
  if (chosen !== undefined) {
    const option = options.find((entry) => entry.option_id === chosen);
    return (
      <p className="remote-notice">
        {title}: {option ? option.label : "declined"}
      </p>
    );
  }
  return (
    <section className="remote-card" aria-label="The agent asks for permission">
      <p className="remote-card-head">
        <StatusDot tone="volt" size={7} />
        Waiting on you
      </p>
      <p className="remote-card-title">{title}</p>
      <div className="remote-card-actions">
        {options.map((option) => (
          <button
            key={option.option_id}
            type="button"
            className={
              option.kind === "allow_once" || option.kind === "allow_always"
                ? "g-button g-button-primary remote-button"
                : "g-button remote-button remote-quiet"
            }
            onClick={() => onChoose(option.option_id)}
          >
            {option.label}
          </button>
        ))}
        {options.length === 0 ? (
          <button
            type="button"
            className="g-button remote-button remote-quiet"
            onClick={() => onChoose(null)}
          >
            Decline
          </button>
        ) : null}
      </div>
    </section>
  );
}

function QuestionCard({
  prompt,
  resolved,
  onAnswer,
}: {
  readonly prompt: UiPrompt;
  readonly resolved: boolean;
  readonly onAnswer: (answer: UiAnswer | null) => void;
}) {
  const [text, setText] = useState("");
  if (prompt.type === "notify") return <p className="remote-notice">{prompt.message}</p>;
  if (resolved) return <p className="remote-notice">{prompt.message}: answered</p>;
  const dismiss = (
    <button
      type="button"
      className="g-button remote-button remote-quiet"
      onClick={() => onAnswer(null)}
    >
      Dismiss
    </button>
  );
  return (
    <section className="remote-card" aria-label="The agent asks a question">
      <p className="remote-card-head">
        <StatusDot tone="volt" size={7} />
        Waiting on you
      </p>
      <p className="remote-card-title">{prompt.message}</p>
      {prompt.type === "confirm" ? (
        <div className="remote-card-actions">
          <button
            type="button"
            className="g-button g-button-primary remote-button"
            onClick={() => onAnswer({ type: "confirm", yes: true })}
          >
            Yes
          </button>
          <button
            type="button"
            className="g-button remote-button remote-quiet"
            onClick={() => onAnswer({ type: "confirm", yes: false })}
          >
            No
          </button>
        </div>
      ) : null}
      {prompt.type === "select" ? (
        <div className="remote-card-actions remote-card-choices">
          {prompt.options.map((option, index) => (
            <button
              // biome-ignore lint/suspicious/noArrayIndexKey: the answer is the option's index.
              key={index}
              type="button"
              className="g-button remote-button remote-quiet"
              onClick={() => onAnswer({ type: "select", index })}
            >
              {option}
            </button>
          ))}
          {dismiss}
        </div>
      ) : null}
      {prompt.type === "input" ? (
        <form
          className="remote-card-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (text.trim()) onAnswer({ type: "input", text });
          }}
        >
          <input
            className="g-input"
            aria-label="Your answer"
            value={text}
            placeholder={prompt.placeholder ?? ""}
            onChange={(event) => setText(event.currentTarget.value)}
          />
          <div className="remote-card-actions">
            <button
              type="submit"
              className="g-button g-button-primary remote-button"
              disabled={!text.trim()}
            >
              Answer
            </button>
            {dismiss}
          </div>
        </form>
      ) : null}
      {prompt.type === "questions" ? (
        <Questions questions={prompt.questions} onAnswer={onAnswer} dismiss={dismiss} />
      ) : null}
      {prompt.type === "form" || prompt.type === "link" ? (
        <>
          <p className="remote-notice">Answer this one on your computer, or dismiss it here.</p>
          <div className="remote-card-actions">{dismiss}</div>
        </>
      ) : null}
    </section>
  );
}

function Questions({
  questions,
  onAnswer,
  dismiss,
}: {
  readonly questions: readonly UiQuestion[];
  readonly onAnswer: (answer: UiAnswer) => void;
  readonly dismiss: ReactNode;
}) {
  const [selected, setSelected] = useState<Readonly<Record<string, readonly number[]>>>({});
  const [texts, setTexts] = useState<Readonly<Record<string, string>>>({});
  const answered = (question: UiQuestion) =>
    (selected[question.id]?.length ?? 0) > 0 || (texts[question.id] ?? "").trim() !== "";
  const toggle = (question: UiQuestion, index: number) => {
    const current = selected[question.id] ?? [];
    const next = question.multi_select
      ? current.includes(index)
        ? current.filter((value) => value !== index)
        : [...current, index].sort((a, b) => a - b)
      : [index];
    setSelected({ ...selected, [question.id]: next });
  };
  return (
    <form
      className="remote-card-form"
      onSubmit={(event) => {
        event.preventDefault();
        onAnswer({
          type: "questions",
          answers: questions.map((question) => ({
            question_id: question.id,
            selected: [...(selected[question.id] ?? [])],
            text: (texts[question.id] ?? "").trim() || null,
          })),
        });
      }}
    >
      {questions.map((question) => (
        <fieldset key={question.id}>
          <legend>{question.question}</legend>
          {question.options.map((option, index) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: the answer is the option's index.
            <label key={index} className="remote-choice">
              <input
                type={question.multi_select ? "checkbox" : "radio"}
                name={`question-${question.id}`}
                checked={(selected[question.id] ?? []).includes(index)}
                onChange={() => toggle(question, index)}
              />
              <span>
                <strong>{option.label}</strong>
                {option.description ? <span>{option.description}</span> : null}
              </span>
            </label>
          ))}
          {question.allow_free_text ? (
            <input
              className="g-input"
              aria-label={`Another answer to: ${question.question}`}
              placeholder="Something else"
              value={texts[question.id] ?? ""}
              onChange={(event) => setTexts({ ...texts, [question.id]: event.currentTarget.value })}
            />
          ) : null}
        </fieldset>
      ))}
      <div className="remote-card-actions">
        <button
          type="submit"
          className="g-button g-button-primary remote-button"
          disabled={!questions.every(answered)}
        >
          Answer
        </button>
        {dismiss}
      </div>
    </form>
  );
}

/**
 * The message box, as the desktop draws it: the raised layer with one faint edge, the words, and
 * Send as a disc. The box empties as soon as Send is pressed, and the message shows as sending
 * until the chat has it; a message that could not be sent comes back to the box.
 */
function Composer({
  disabled,
  working,
  onSend,
  onStop,
  onError,
}: {
  readonly disabled: boolean;
  readonly working: boolean;
  readonly onSend: (text: string) => Promise<void>;
  readonly onStop: () => void;
  readonly onError: (message: string) => void;
}) {
  const [text, setText] = useState("");
  const field = useRef<HTMLTextAreaElement>(null);
  useLayoutEffect(() => {
    const node = field.current;
    if (!node) return;
    // The box grows with what is typed, up to a third of the screen.
    node.style.height = "auto";
    node.style.height = `${Math.min(node.scrollHeight, window.innerHeight / 3)}px`;
  });
  const submit = () => {
    const words = text;
    if (!words.trim() || disabled) return;
    setText("");
    onSend(words).catch((failure) => {
      // What was typed comes back, to send again, unless something new was typed meanwhile.
      setText((current) => (current.trim() ? current : words));
      onError(message(failure));
    });
  };
  const empty = !text.trim();
  return (
    <form
      className="remote-composer"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      <div className="remote-composer-box">
        <textarea
          ref={field}
          className="g-textarea"
          rows={1}
          aria-label="Message"
          placeholder={working ? "Add to the queue" : "Message the agent"}
          value={text}
          enterKeyHint="send"
          onChange={(event) => setText(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              submit();
            }
          }}
        />
        <div className="remote-composer-controls">
          {working ? (
            <button type="button" className="remote-stop" aria-label="Stop" onClick={onStop}>
              <Square />
              <span>Stop</span>
            </button>
          ) : null}
          <button
            type="submit"
            className="remote-send"
            aria-label="Send"
            disabled={disabled || empty}
            data-quiet={empty ? "" : undefined}
          >
            <Up />
          </button>
        </div>
      </div>
    </form>
  );
}

function Dots() {
  return (
    <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
      <circle cx="4" cy="10" r="1.6" fill="currentColor" />
      <circle cx="10" cy="10" r="1.6" fill="currentColor" />
      <circle cx="16" cy="10" r="1.6" fill="currentColor" />
    </svg>
  );
}

/** Two sliders: what a chat runs on can be changed here. */
function Sliders() {
  return (
    <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
      <path
        d="M3 6.5h6.5M13.5 6.5H17M3 13.5h3.5M10.5 13.5H17"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
      />
      <circle cx="11.5" cy="6.5" r="2" fill="none" stroke="currentColor" strokeWidth="1.6" />
      <circle cx="8.5" cy="13.5" r="2" fill="none" stroke="currentColor" strokeWidth="1.6" />
    </svg>
  );
}

function Back() {
  return (
    <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
      <path
        d="M12.5 4.5 7 10l5.5 5.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function Up() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M8 13V3.5M3.5 8 8 3.5 12.5 8"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function Down() {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M8 3v9.5M3.5 8 8 12.5 12.5 8"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function Square() {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <rect x="1" y="1" width="8" height="8" rx="1.5" fill="currentColor" />
    </svg>
  );
}

function Plus() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
      <path
        d="M8 3v10M3 8h10"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
    </svg>
  );
}

function Chevron() {
  return (
    <svg className="remote-chevron" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <path
        d="M4.5 2.5 8 6l-3.5 3.5"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
