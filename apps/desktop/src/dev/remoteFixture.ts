import type { RemoteStatus } from "../bindings";
import type { RemoteApi } from "../lib/remote";

/**
 * Remote access for development previews: nothing connects anywhere. `?remote=on` starts with it
 * on, a paired phone and a linked bot; `window.remoteFixture` plays the other side (a device
 * opening the pairing link, the relay going away).
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
          },
        ]
      : [],
    bots: on
      ? [
          {
            id: "bot-grok",
            name: "Grok Bot",
            created_at: Date.now() - 3_600_000,
            last_used_at: null,
            full: false,
            calls: 0,
            failed: 0,
          },
        ]
      : [],
    pairing_expires_at: null,
  };
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
            },
          ],
        }),
      offline: (reason: string) => publish({ ...status, link: { state: "offline", reason } }),
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
      });
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
    createBot: async (name, full) => {
      calls.push(`create-bot:${name}${full ? ":full" : ""}`);
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
            calls: 0,
            failed: 0,
          },
        ],
      });
      return {
        id,
        name: name.trim() || "Bot",
        url: `https://o7.codes/api/remote/v1/mcp/${"h".repeat(22)}/${"b".repeat(11)}.${"t".repeat(43)}`,
      };
    },
    removeBot: async (id) => {
      calls.push(`remove-bot:${id}`);
      return publish({ ...status, bots: status.bots.filter((bot) => bot.id !== id) });
    },
    subscribe: (changed) => {
      listeners.add(changed);
      return () => listeners.delete(changed);
    },
  };
}
