/**
 * A synthetic app updater for previews (`&app-update=<case>` on `?preview=agent-updates`).
 * Nothing is downloaded or installed.
 *
 * - `available`: a newer version is on offer; Update and restart downloads it in a second and a
 *   half, installs and ends on "restart to finish".
 * - `fail`: the first download fails as an invalid signature does; trying again passes.
 * - `busy`: installing waits once for work to finish, then passes.
 * - `current`: the app is up to date, so nothing shows.
 *
 * `document.body.dataset.appUpdateCalls` lists the operations asked for, in order.
 */
import type { UpdateStatus } from "../bindings";
import type { UpdatesApi } from "../lib/updates";

const TOTAL = 130 * 1_048_576;

export function fakeUpdates(kind: string): UpdatesApi {
  let status: UpdateStatus = {
    channel: "stable",
    phase: kind === "current" ? "idle" : "available",
    current_version: "0.1.1570",
    automatic: true,
    version: kind === "current" ? null : "0.1.1571",
    notes:
      kind === "current"
        ? null
        : "The Update button. Ghost now looks for a newer version by itself and installs it when you choose.",
    downloaded_bytes: 0,
    total_bytes: null,
    message: null,
  };
  let failures = kind === "fail" ? 1 : 0;
  let waits = kind === "busy" ? 1 : 0;
  const calls: string[] = [];
  const called = (name: string) => {
    calls.push(name);
    document.body.dataset.appUpdateCalls = calls.join(",");
  };
  const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));
  return {
    status: async () => structuredClone(status),
    check: async () => {
      called("check");
      status = { ...status, phase: "available", message: null };
      return structuredClone(status);
    },
    download: async () => {
      called("download");
      status = { ...status, phase: "downloading", total_bytes: TOTAL, downloaded_bytes: 0 };
      for (let step = 1; step <= 6; step++) {
        await pause(250);
        status = { ...status, downloaded_bytes: Math.round((TOTAL * step) / 6) };
      }
      if (failures > 0) {
        failures--;
        const message = "The update signature or signed version is invalid. Nothing was installed.";
        status = { ...status, phase: "failed", message };
        throw new Error(message);
      }
      status = { ...status, phase: "ready", message: null };
      return structuredClone(status);
    },
    install: async () => {
      called("install");
      if (waits > 0) {
        waits--;
        status = {
          ...status,
          phase: "waiting_for_idle",
          message: "Wait for Git changes and agent updates to finish, then retry installation.",
        };
        return structuredClone(status);
      }
      status = { ...status, phase: "installing", message: null };
      await pause(900);
      status = {
        ...status,
        phase: "restart_required",
        message: "Preview: nothing was installed.",
      };
      return structuredClone(status);
    },
  };
}
