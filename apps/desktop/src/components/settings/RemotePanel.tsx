import { BOTS, type BotId, BotMark, botNamed } from "@o7/ui/graphite";
import { Bot, Monitor, MonitorSmartphone, Smartphone } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { encode } from "uqr";
import type { RemoteBotLink, RemoteDevice, RemotePairing, RemoteStatus } from "../../bindings";
import { ago, linkLabel, linkTone, type RemoteApi, remaining, remoteApi } from "../../lib/remote";
import { Modal } from "../chat/Modal";
import { Row, State, Switch } from "./SettingsRow";
import "./remote.css";

const message = (error: unknown): string =>
  error instanceof Error && error.message ? error.message : "Something went wrong.";

/**
 * Settings' Remote access page: the switch, the devices paired with this computer and the bots
 * linked to it (docs/remote-sessions.md). Pairing and linking each show their secret once, in a
 * dialog; the lists never repeat it.
 */
export function RemotePanel({ api = remoteApi }: { readonly api?: RemoteApi }) {
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pairing, setPairing] = useState<RemotePairing | null>(null);
  /** The devices that were paired when the pairing dialog opened: a new one ends it. */
  const [before, setBefore] = useState<readonly string[]>([]);
  /** The link dialog, opened for one assistant or with none chosen yet. */
  const [linking, setLinking] = useState<{ readonly bot: BotChoice } | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    let live = true;
    api.status().then(
      (value) => {
        if (live) setStatus(value);
      },
      (failure) => {
        if (live) setError(message(failure));
      },
    );
    const stop = api.subscribe((value) => {
      if (live) setStatus(value);
    });
    return () => {
      live = false;
      stop();
    };
  }, [api]);
  // "Last seen 3 min ago" keeps telling the truth while the page stays open.
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);

  const run = useCallback(async (action: () => Promise<RemoteStatus>) => {
    setError(null);
    try {
      setStatus(await action());
    } catch (failure) {
      setError(message(failure));
    }
  }, []);

  /**
   * Pairing and linking need remote access on. A pairing link exists from the moment its dialog
   * opens, so asking for one turns it on; a bot link turns it on only when the link is created,
   * so reading what a link allows and closing the dialog connects this computer to nothing.
   */
  const ensureOn = useCallback(async () => {
    if (status?.enabled) return;
    setStatus(await api.setEnabled(true));
  }, [api, status]);

  const pair = async () => {
    setError(null);
    try {
      await ensureOn();
      setBefore((status?.devices ?? []).map((device) => device.id));
      setPairing(await api.pair());
    } catch (failure) {
      setError(message(failure));
    }
  };

  const link = (bot: BotChoice) => {
    setError(null);
    setLinking({ bot });
  };

  const enabled = status?.enabled ?? false;
  const devices = status?.devices ?? [];
  const bots = status?.bots ?? [];
  const paired = pairing
    ? (status?.devices.find((device) => !before.includes(device.id)) ?? null)
    : null;

  return (
    <>
      <p className="settings-lead">
        See and steer this computer's chats from your phone, or let a bot work with them. o7
        connects out to its relay; nothing on this computer listens for connections.
      </p>
      {error ? (
        <p className="settings-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="settings-group">
        <Row
          title="Remote access"
          mark={<MonitorSmartphone size={18} />}
          detail={status ? <State tone={linkTone(status)}>{linkLabel(status)}</State> : "Checking…"}
        >
          <Switch
            label="Remote access"
            checked={enabled}
            disabled={status === null}
            testId="remote-enabled"
            onChange={(value) => void run(() => api.setEnabled(value))}
          />
        </Row>
      </div>

      <section className="settings-section" aria-labelledby="settings-remote-devices">
        <div className="settings-heading" data-setting="Pair a device">
          <h2 id="settings-remote-devices">Devices</h2>
          <button
            className="g-button g-button-outline g-button-28"
            type="button"
            data-testid="remote-pair"
            disabled={status === null}
            onClick={() => void pair()}
          >
            Pair a device
          </button>
        </div>
        <p className="settings-lead">
          Your phone, or any browser you open the link in. A pairing link opens once, within ten
          minutes.
        </p>
        {devices.length ? (
          <div className="settings-group">
            {devices.map((device) => (
              <Row
                key={device.id}
                title={device.name}
                setting="Devices"
                mark={handheld(device.name) ? <Smartphone size={18} /> : <Monitor size={18} />}
                detail={
                  device.connected ? (
                    <State tone="green">Connected now</State>
                  ) : (
                    <State tone="off">Last seen {ago(device.last_seen_at, now)}</State>
                  )
                }
              >
                <button
                  className="g-button g-button-ghost g-button-28"
                  type="button"
                  aria-label={`Remove ${device.name}`}
                  onClick={() => void run(() => api.removeDevice(device.id))}
                >
                  Remove
                </button>
              </Row>
            ))}
          </div>
        ) : (
          <p className="settings-empty" data-setting="Devices">
            <strong>No devices yet</strong>
            Pair your phone to read your chats, send messages and answer requests from it.
          </p>
        )}
      </section>

      <section className="settings-section" aria-labelledby="settings-remote-bots">
        <div className="settings-heading" data-setting="Link a bot">
          <h2 id="settings-remote-bots">Bots</h2>
          <button
            className="g-button g-button-outline g-button-28"
            type="button"
            data-testid="remote-link-bot"
            disabled={status === null}
            onClick={() => link("grok")}
          >
            Link a bot
          </button>
        </div>
        <p className="settings-lead">
          Hand your assistant a goal from wherever you are. It starts your coding agents here,
          answers what they ask and brings back the result.
        </p>
        {bots.length ? (
          <div className="settings-group">
            {bots.map((bot) => (
              <Row
                key={bot.id}
                title={bot.name}
                setting="Bots"
                mark={<LinkMark name={bot.name} />}
                badge={<span className="g-chip">{bot.full ? "Full control" : "Chats"}</span>}
                detail={
                  bot.last_used_at === null ? (
                    <State tone="off">Not used yet</State>
                  ) : (
                    <State tone={bot.failed > 0 && bot.failed === bot.calls ? "off" : "green"}>
                      Last used {ago(bot.last_used_at, now)}
                      {usage(bot)}
                    </State>
                  )
                }
              >
                <button
                  className="g-button g-button-ghost g-button-28"
                  type="button"
                  aria-label={`Remove ${bot.name}`}
                  onClick={() => void run(() => api.removeBot(bot.id))}
                >
                  Remove
                </button>
              </Row>
            ))}
          </div>
        ) : (
          <div className="settings-empty remote-bots" data-setting="Bots">
            <p>
              <strong>No bots linked</strong>
              Pick yours to link it. Any other assistant that takes an MCP server works too.
            </p>
            <ul className="remote-bot-picks">
              {BOTS.map((bot) => (
                <li key={bot.id}>
                  <button
                    className="remote-bot-pick"
                    type="button"
                    data-testid={`remote-link-${bot.id}`}
                    disabled={status === null}
                    onClick={() => link(bot.id)}
                  >
                    <BotMark bot={bot.id} size={36} />
                    <span>
                      <strong>{bot.name}</strong>
                      <span>{bot.maker}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </section>
      <details className="settings-more">
        <summary>What the relay can see</summary>
        <p>
          A paired browser's session is encrypted end to end: the relay passes it along and cannot
          read it. A bot's calls are not: the relay and the bot's own service read them, and either
          could act as that bot. Remote access works while o7 is open, including in the tray.
        </p>
      </details>

      {pairing ? (
        <PairingDialog
          pairing={pairing}
          paired={paired}
          onRenew={() => void pair()}
          onClose={() => {
            setPairing(null);
            if (!paired) void run(() => api.cancelPairing());
          }}
        />
      ) : null}
      {linking ? (
        <BotDialog
          initial={linking.bot}
          create={async (name, full) => {
            await ensureOn();
            return api.createBot(name, full);
          }}
          called={(id) => bots.some((bot) => bot.id === id && bot.last_used_at !== null)}
          onClose={() => {
            setLinking(null);
            void run(() => api.status());
          }}
        />
      ) : null}
    </>
  );
}

/** A link's calls so far, after when it was last used: " · 14 calls, 1 failed". */
export function usage(bot: { readonly calls: number; readonly failed: number }): string {
  if (bot.calls === 0) return "";
  const calls = `${bot.calls} ${bot.calls === 1 ? "call" : "calls"}`;
  return bot.failed === 0 ? ` · ${calls}` : ` · ${calls}, ${bot.failed} failed`;
}

/** Whether a paired browser's name reads as a phone or tablet: its row wears that glyph. */
const handheld = (name: string): boolean =>
  /iphone|ipad|android|pixel|galaxy|phone|mobile/i.test(name);

/** A QR code as one SVG path: dark modules on white, whatever the app's appearance. */
export function QrCode({ text, label }: { readonly text: string; readonly label: string }) {
  const { size, path } = useMemo(() => {
    // Four modules of quiet space on every side, as scanners expect.
    const code = encode(text, { ecc: "M", border: 4 });
    let d = "";
    code.data.forEach((row, y) => {
      row.forEach((dark, x) => {
        if (dark) d += `M${x} ${y}h1v1h-1z`;
      });
    });
    return { size: code.size, path: d };
  }, [text]);
  return (
    <svg
      className="remote-qr"
      viewBox={`0 0 ${size} ${size}`}
      role="img"
      aria-label={label}
      shapeRendering="crispEdges"
    >
      <rect width={size} height={size} fill="white" />
      <path d={path} fill="black" />
    </svg>
  );
}

/** A named assistant, or any other that takes an MCP server. */
export type BotChoice = BotId | "other";

/** A bot link's mark: its assistant's, when its name names one of the four. */
function LinkMark({ name }: { readonly name: string }) {
  const bot = botNamed(name);
  return bot ? <BotMark bot={bot} size={36} /> : <Bot size={18} />;
}

/**
 * How each assistant takes the address, in its own words. `paste` is what goes on the
 * clipboard: the address alone, or the lines or command that carry it. Hermes' and OpenClaw's
 * set a tool's timeout to five minutes whatever their defaults are: a bot that may wait longer
 * for a turn asks less often, and each time it asks costs it.
 */
const SETUP: Readonly<
  Record<
    BotChoice,
    {
      readonly label: string;
      readonly paste: (url: string) => string;
      readonly steps: readonly string[];
    }
  >
> = {
  grok: {
    label: "Bot address",
    paste: (url) => url,
    steps: [
      "In the Grok Bot app, open Connect apps in the sidebar and choose Add a custom connector.",
      "Name it o7, choose Remote HTTPS, paste this address as the Server URL, then choose Add plugin.",
      "Ask your bot to list your o7 chats.",
    ],
  },
  muse: {
    label: "Bot address",
    paste: (url) => url,
    steps: [
      "In Muse, ask it to add a custom connector.",
      "Paste this address when it asks for the server's URL.",
      "Ask it to list your o7 chats.",
    ],
  },
  hermes: {
    label: "Lines for Hermes' config",
    paste: (url) => `mcp_servers:\n  o7:\n    url: "${url}"\n    timeout: 300`,
    steps: [
      "Add these lines to ~/.hermes/config.yaml (under mcp_servers, if you have one already).",
      "Run /reload-mcp in Hermes.",
      "Ask it to list your o7 chats.",
    ],
  },
  openclaw: {
    label: "Command for OpenClaw",
    paste: (url) => `openclaw mcp add o7 --url ${url} --transport streamable-http --timeout 300`,
    steps: ["Run this command on the computer OpenClaw runs on.", "Ask it to list your o7 chats."],
  },
  other: {
    label: "Bot address",
    paste: (url) => url,
    steps: [
      "Add this address wherever your assistant takes a remote MCP server (streamable HTTP).",
      "Ask it to list your o7 chats.",
    ],
  },
};

function CopyField({ value, label }: { readonly value: string; readonly label: string }) {
  const [state, setState] = useState<"ready" | "copied" | "failed">("ready");
  const reset = useRef<ReturnType<typeof setTimeout> | null>(null);
  const field = useId();
  useEffect(
    () => () => {
      if (reset.current !== null) clearTimeout(reset.current);
    },
    [],
  );
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setState("copied");
    } catch {
      setState("failed");
    }
    if (reset.current !== null) clearTimeout(reset.current);
    reset.current = setTimeout(() => setState("ready"), 1600);
  };
  // Lines to paste (a config entry) are shown whole, with Copy under them.
  const lines = value.includes("\n");
  return (
    <div className="remote-copy" data-lines={lines ? "" : undefined}>
      <label className="sr-only" htmlFor={field}>
        {label}
      </label>
      {lines ? (
        <textarea
          id={field}
          className="g-input remote-secret"
          readOnly
          rows={value.split("\n").length}
          wrap="off"
          value={value}
          spellCheck={false}
          onFocus={(event) => event.currentTarget.select()}
        />
      ) : (
        <input
          id={field}
          className="g-input remote-secret"
          readOnly
          value={value}
          spellCheck={false}
          onFocus={(event) => event.currentTarget.select()}
        />
      )}
      <button className="g-button g-button-outline" type="button" onClick={() => void copy()}>
        <span aria-live="polite">
          {state === "copied" ? "Copied" : state === "failed" ? "Could not copy" : "Copy"}
        </span>
      </button>
    </div>
  );
}

function PairingDialog({
  pairing,
  paired,
  onRenew,
  onClose,
}: {
  readonly pairing: RemotePairing;
  readonly paired: RemoteDevice | null;
  readonly onRenew: () => void;
  readonly onClose: () => void;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  const expired = pairing.expires_at <= now;
  return (
    <Modal title="Pair a device" onClose={onClose}>
      {paired ? (
        <>
          <p className="remote-result" role="status">
            Paired with {paired.name}. It can open your chats from now on, without another link.
          </p>
          <div className="g-dialog-actions">
            <button className="g-button g-button-primary" type="button" onClick={onClose}>
              Done
            </button>
          </div>
        </>
      ) : expired ? (
        <>
          <p className="remote-result" role="status">
            This link expired before a device opened it.
          </p>
          <div className="g-dialog-actions">
            <button className="g-button" type="button" onClick={onClose}>
              Close
            </button>
            <button className="g-button g-button-primary" type="button" onClick={onRenew}>
              New link
            </button>
          </div>
        </>
      ) : (
        <>
          <QrCode text={pairing.url} label="Pairing code" />
          <p className="remote-note">
            Scan it with your phone's camera, or open the link in the browser of the device you want
            to pair.
          </p>
          <CopyField value={pairing.url} label="Pairing link" />
          <p className="remote-note" data-testid="remote-pairing-time">
            Opens once, for another {remaining(pairing.expires_at, now)}. Whoever opens it first can
            read and steer this computer's chats, so keep it to yourself.
          </p>
        </>
      )}
    </Modal>
  );
}

/**
 * The dialog that makes a bot link: which assistant, what it may do and its name, then the
 * address, shown once, the way that assistant takes it. Setup opens it too.
 */
export function BotDialog({
  initial,
  create,
  called,
  onClose,
}: {
  readonly initial: BotChoice;
  readonly create: (name: string, full: boolean) => Promise<RemoteBotLink>;
  /** Whether the link with this id has made a call yet: the dialog then says it is connected. */
  readonly called?: (id: string) => boolean;
  readonly onClose: () => void;
}) {
  const [bot, setBot] = useState<BotChoice>(initial);
  const named = (choice: BotChoice) =>
    BOTS.find((kind) => kind.id === choice)?.name ?? "My assistant";
  const [name, setName] = useState(() => named(initial));
  // What the link may do is chosen here and is the link's for good: a wider one is a new link.
  const [full, setFull] = useState(false);
  const [link, setLink] = useState<RemoteBotLink | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const field = useId();
  const choose = (choice: BotChoice) => {
    // The name follows the assistant until its owner writes one of their own.
    if (name === named(bot)) setName(named(choice));
    setBot(choice);
  };
  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      setLink(await create(name, full));
    } catch (failure) {
      setError(message(failure));
    } finally {
      setBusy(false);
    }
  };
  const setup = SETUP[bot];
  return (
    <Modal title="Link a bot" onClose={onClose}>
      {link ? (
        <>
          <p className="remote-note">
            This is {link.name}'s address. It is shown once and works like a password: anyone who
            has it can read your chats and direct your agents. Removing the link here stops it.
          </p>
          <CopyField value={setup.paste(link.url)} label={setup.label} />
          <ol className="remote-steps">
            {setup.steps.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ol>
          {called ? (
            <p className="remote-note" role="status" data-testid="remote-bot-called">
              {called(link.id)
                ? `Connected: ${link.name} just called this computer.`
                : `Waiting for ${link.name}'s first call. This line changes when it arrives.`}
            </p>
          ) : null}
          <div className="g-dialog-actions">
            <button className="g-button g-button-primary" type="button" onClick={onClose}>
              Done
            </button>
          </div>
        </>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <fieldset className="remote-kinds">
            <legend>Your assistant</legend>
            {[...BOTS.map((kind) => kind.id), "other" as const].map((choice) => (
              <label key={choice} className="remote-kind">
                <input
                  type="radio"
                  name="remote-bot-kind"
                  checked={bot === choice}
                  onChange={() => choose(choice)}
                />
                {choice === "other" ? (
                  <span className="remote-kind-glyph" aria-hidden="true">
                    <Bot size={18} />
                  </span>
                ) : (
                  <BotMark bot={choice} size={28} />
                )}
                <span>{choice === "other" ? "Another" : named(choice)}</span>
              </label>
            ))}
          </fieldset>
          <fieldset className="remote-levels">
            <legend>What it can do</legend>
            <label className="remote-level">
              <input
                type="radio"
                name="remote-bot-level"
                checked={!full}
                onChange={() => setFull(false)}
              />
              <span>
                <strong>Chats</strong>
                <span>
                  List and read your chats, send messages, start chats, stop turns and answer what
                  an agent asks.
                </span>
              </span>
            </label>
            <label className="remote-level">
              <input
                type="radio"
                name="remote-bot-level"
                checked={full}
                onChange={() => setFull(true)}
              />
              <span>
                <strong>Full control</strong>
                <span>
                  Everything in Chats, and what you do in this window: make a project from any
                  folder on this computer, choose a chat's model and thinking, set its access level
                  (full auto included), start a stopped agent, hand a chat to another agent, rename
                  and archive chats, read a turn's file changes, and read and write project memory,
                  which every later chat in that project starts with. It has no tool for Settings,
                  sign-in, installing software or deleting data.
                </span>
              </span>
            </label>
          </fieldset>
          <label className="settings-field" htmlFor={field}>
            <span>Name</span>
            <input
              id={field}
              className="g-input"
              value={name}
              maxLength={48}
              onChange={(event) => setName(event.currentTarget.value)}
            />
          </label>
          <p className="remote-note remote-fine">
            A linked bot directs your agents, and agents can edit files and run commands. So either
            level is as much trust as handing someone this computer: the level limits the bot's own
            tools, not what an agent it directs can do.
          </p>
          {error ? (
            <p className="settings-error" role="alert">
              {error}
            </p>
          ) : null}
          <div className="g-dialog-actions">
            <button className="g-button" type="button" onClick={onClose}>
              Cancel
            </button>
            <button className="g-button g-button-primary" type="submit" disabled={busy}>
              Create link
            </button>
          </div>
        </form>
      )}
    </Modal>
  );
}
