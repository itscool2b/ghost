import type { LoginStart, RemoteStatus } from "../bindings";
import type { RemoteApi } from "../lib/remote";

/**
 * Remote access for development previews: nothing connects anywhere. `?remote=on` starts with it
 * on, a paired phone's browser, o7 for iPhone with notifications on and a linked bot;
 * `window.remoteFixture` plays the other side (a device opening the pairing link, the relay going
 * away or not sending notifications).
 */
export function remoteFixture(): RemoteApi {
  const on = new URLSearchParams(location.search).get("remote") === "on";
  const calls: string[] = [];
  const listeners = new Set<(status: RemoteStatus) => void>();
  let status: RemoteStatus = {
    enabled: on,
    link: on ? { state: "online" } : { state: "off" },
    host_name: "Studio PC",
    devices: on
      ? [
          {
            id: "device-phone",
            name: "iPhone (Safari)",
            created_at: Date.now() - 86_400_000 * 3,
            last_seen_at: Date.now() - 60_000 * 7,
            connected: false,
            kind: "browser",
            app_version: null,
          },
          // The iPhone app, under a name its person chose: its kind, not its name, says phone.
          // It turned notifications on, and the last one went five minutes ago.
          {
            id: "device-app",
            name: "Travel",
            created_at: Date.now() - 86_400_000,
            last_seen_at: Date.now(),
            connected: true,
            kind: "ios",
            app_version: "1.0",
            notifications: true,
            last_push: Date.now() - 60_000 * 5,
          },
        ]
      : [],
    // The relay sends notifications.
    notifications: on,
    bots: on
      ? [
          {
            id: "bot-grok",
            name: "Grok Bot",
            created_at: Date.now() - 3_600_000,
            last_used_at: null,
            full: false,
            read_only: false,
            approve: false,
            full_access: false,
            expires_at: Date.now() + 90 * 86_400_000,
            calls: 0,
            failed: 0,
            callback: null,
          },
        ]
      : [],
    pairing_expires_at: null,
    keep_awake: false,
    automations: on
      ? [
          {
            id: "auto-ci",
            name: "CI failures",
            project: "project-web",
            instruction:
              "Fix what the failed check reports, run the tests and commit on the branch.",
            created_at: Date.now() - 86_400_000 * 2,
            last_used_at: Date.now() - 3_600_000 * 5,
            expires_at: Date.now() + 85 * 86_400_000,
            calls: 4,
            failed: 0,
            last_error: null,
          },
        ]
      : [],
  };
  const projects = [
    { id: "project-web", name: "web" },
    { id: "project-api", name: "api" },
  ];
  let loginStart: LoginStart = { on: false, unavailable: null };
  const publish = (next: RemoteStatus): RemoteStatus => {
    status = next;
    for (const listener of listeners) listener(structuredClone(status));
    return structuredClone(status);
  };
  let pairingMs = 600_000;
  Object.assign(window, {
    remoteFixture: {
      calls: () => [...calls],
      /** A device opens the pairing link. */
      pairDevice: (name: string) =>
        publish({
          ...status,
          pairing_expires_at: null,
          devices: [
            ...status.devices,
            {
              id: `device-${status.devices.length + 1}`,
              name,
              created_at: Date.now(),
              last_seen_at: Date.now(),
              connected: true,
              kind: "browser",
              app_version: null,
            },
          ],
        }),
      offline: (reason: string) => publish({ ...status, link: { state: "offline", reason } }),
      /** Whether the relay says it sends notifications. */
      relayPushes: (on: boolean) => publish({ ...status, notifications: on }),
      /** How long the next pairing link opens for. */
      pairingLasts: (ms: number) => {
        pairingMs = ms;
      },
    },
  });
  return {
    status: async () => structuredClone(status),
    setEnabled: async (enabled) => {
      calls.push(`enabled:${enabled}`);
      return publish({
        ...status,
        enabled,
        link: enabled ? { state: "online" } : { state: "off" },
        pairing_expires_at: enabled ? status.pairing_expires_at : null,
        // Its relay sends notifications while it is connected.
        notifications: enabled,
      });
    },
    setKeepAwake: async (on) => {
      calls.push(`keep-awake:${on}`);
      return publish({ ...status, keep_awake: on });
    },
    loginStart: async () => ({ ...loginStart }),
    setLoginStart: async (on) => {
      calls.push(`login-start:${on}`);
      loginStart = { ...loginStart, on };
      return { ...loginStart };
    },
    projects: async () => projects,
    localProgram: async () => "C:\\Users\\you\\AppData\\Local\\o7\\o7.exe",
    createAutomation: async (name, project, instruction) => {
      calls.push(`create-automation:${name}:${project}`);
      if (!instruction.trim()) throw new Error("Say what its agent should do with what is sent.");
      const id = `auto-${status.automations.length + 1}`;
      publish({
        ...status,
        automations: [
          ...status.automations,
          {
            id,
            name: name.trim() || "Automation",
            project,
            instruction,
            created_at: Date.now(),
            last_used_at: null,
            expires_at: Date.now() + 90 * 86_400_000,
            calls: 0,
            failed: 0,
            last_error: null,
          },
        ],
      });
      return {
        id,
        name: name.trim() || "Automation",
        url: `https://o7.codes/api/remote/v1/hook/${"h".repeat(22)}/${"a".repeat(11)}.${"t".repeat(43)}`,
      };
    },
    pair: async () => {
      calls.push("pair");
      const expires_at = Date.now() + pairingMs;
      publish({ ...status, pairing_expires_at: expires_at });
      return {
        // The shape of a real link, with nothing secret in it.
        url: `https://o7.codes/remote#${"h".repeat(22)}.${"p".repeat(11)}.${"k".repeat(43)}`,
        expires_at,
      };
    },
    cancelPairing: async () => {
      calls.push("cancel-pairing");
      return publish({ ...status, pairing_expires_at: null });
    },
    removeDevice: async (id) => {
      calls.push(`remove-device:${id}`);
      return publish({ ...status, devices: status.devices.filter((device) => device.id !== id) });
    },
    createBot: async (name, rights) => {
      const full = rights.level === "full";
      calls.push(
        `create-bot:${name}${full ? ":full" : rights.level === "read" ? ":read" : ""}${rights.approve ? ":approve" : ""}${rights.fullAccess ? ":full-access" : ""}`,
      );
      if (name.trim() === "fail")
        throw new Error("This computer already has 16 bot links. Remove one first.");
      const id = `bot-${status.bots.length + 1}`;
      publish({
        ...status,
        bots: [
          ...status.bots,
          {
            id,
            name: name.trim() || "Bot",
            created_at: Date.now(),
            last_used_at: null,
            full,
            read_only: rights.level === "read",
            approve: rights.level !== "read" && rights.approve,
            full_access: full && rights.fullAccess,
            expires_at: Date.now() + 90 * 86_400_000,
            calls: 0,
            failed: 0,
            callback: null,
          },
        ],
      });
      return {
        id,
        name: name.trim() || "Bot",
        url: `https://o7.codes/api/remote/v1/mcp/${"h".repeat(22)}/${"b".repeat(11)}.${"t".repeat(43)}`,
      };
    },
    renewBot: async (id) => {
      calls.push(`renew-bot:${id}`);
      const automation = status.automations.find((item) => item.id === id);
      if (automation)
        return {
          id,
          name: automation.name,
          url: `https://o7.codes/api/remote/v1/hook/${"h".repeat(22)}/${"a".repeat(11)}.${"n".repeat(43)}`,
        };
      const bot = status.bots.find((item) => item.id === id);
      if (!bot) throw new Error("That bot link is gone.");
      publish({
        ...status,
        bots: status.bots.map((item) =>
          item.id === id
            ? {
                ...item,
                last_used_at: null,
                created_at: Date.now(),
                expires_at: Date.now() + 90 * 86_400_000,
              }
            : item,
        ),
      });
      return {
        id,
        name: bot.name,
        url: `https://o7.codes/api/remote/v1/mcp/${"h".repeat(22)}/${"b".repeat(11)}.${"n".repeat(43)}`,
      };
    },
    removeBot: async (id) => {
      calls.push(`remove-bot:${id}`);
      return publish({
        ...status,
        bots: status.bots.filter((bot) => bot.id !== id),
        automations: status.automations.filter((automation) => automation.id !== id),
      });
    },
    setBotCallback: async (id, callback) => {
      calls.push(`set-callback:${id}:${callback.kind}:${callback.key === null ? "kept" : "key"}`);
      if (!/^https:\/\/|^http:\/\/(127\.0\.0\.1|localhost)/.test(callback.url))
        throw new Error("The address starts with https://.");
      return publish({
        ...status,
        bots: status.bots.map((bot) =>
          bot.id === id
            ? {
                ...bot,
                callback: {
                  kind: callback.kind,
                  address: new URL(callback.url).origin,
                  channel: callback.channel,
                  to: callback.to,
                  last: null,
                },
              }
            : bot,
        ),
      });
    },
    clearBotCallback: async (id) => {
      calls.push(`clear-callback:${id}`);
      return publish({
        ...status,
        bots: status.bots.map((bot) => (bot.id === id ? { ...bot, callback: null } : bot)),
      });
    },
    testBotCallback: async (id) => {
      calls.push(`test-callback:${id}`);
      return publish({
        ...status,
        bots: status.bots.map((bot) =>
          bot.id === id && bot.callback
            ? {
                ...bot,
                callback: {
                  ...bot.callback,
                  last: { at: Date.now(), ok: true, detail: "Delivered." },
                },
              }
            : bot,
        ),
      });
    },
    testNotification: async (id) => {
      calls.push(`test-notification:${id}`);
      const device = status.devices.find((item) => item.id === id);
      if (!device?.notifications)
        throw new Error(
          "Notifications are not on for this device here. Turn them on in o7 for iPhone.",
        );
      if (!status.notifications)
        throw new Error("The relay this computer uses does not send notifications yet.");
      // Apple takes it at once.
      return publish({
        ...status,
        devices: status.devices.map((item) =>
          item.id === id ? { ...item, last_push: Date.now() } : item,
        ),
      });
    },
    subscribe: (changed) => {
      listeners.add(changed);
      return () => listeners.delete(changed);
    },
  };
}
