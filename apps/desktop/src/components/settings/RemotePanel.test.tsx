import { describe, expect, it } from "vitest";
import { deviceApp, deviceNotifications, told, usage } from "./RemotePanel";

describe("a bot link's use, as its row says it", () => {
  it("says nothing before the first call", () => {
    expect(usage({ calls: 0, failed: 0 })).toBe("");
  });
  it("counts calls, and failures only when there are some", () => {
    expect(usage({ calls: 1, failed: 0 })).toBe(" · 1 call");
    expect(usage({ calls: 14, failed: 0 })).toBe(" · 14 calls");
    expect(usage({ calls: 14, failed: 3 })).toBe(" · 14 calls, 3 failed");
  });
});

describe("a bot link's call-back, as its row says it", () => {
  const callback = { kind: "grok", address: "https://api.example.com", channel: null, to: null };
  it("says nothing when the link is told nowhere", () => {
    expect(told({ callback: null })).toBe("");
  });
  it("names the host it tells, never more of the address", () => {
    expect(told({ callback: { ...callback, last: null } })).toBe(" · tells api.example.com");
    expect(
      told({ callback: { ...callback, last: { at: 1, ok: true, detail: "Delivered." } } }),
    ).toBe(" · tells api.example.com");
  });
  it("says so when the last message was not delivered", () => {
    expect(
      told({ callback: { ...callback, last: { at: 1, ok: false, detail: "Refused." } } }),
    ).toBe(" · call back failed");
  });
});

describe("o7 for iPhone, as its row says it", () => {
  const now = 1_000_000_000;
  it("names the app and its version, and nothing for a browser", () => {
    expect(deviceApp({ kind: "ios", app_version: "1.0" })).toBe(" · o7 for iPhone 1.0");
    expect(deviceApp({ kind: "ios", app_version: null })).toBe(" · o7 for iPhone");
    expect(deviceApp({ kind: "browser", app_version: null })).toBe("");
  });
  it("says whether it hears from this computer, and when it last did", () => {
    expect(deviceNotifications({ kind: "ios", notifications: false }, now)).toBe(
      " · Notifications off",
    );
    expect(deviceNotifications({ kind: "ios", notifications: true, last_push: null }, now)).toBe(
      " · Notifications on",
    );
    expect(
      deviceNotifications({ kind: "ios", notifications: true, last_push: now - 3 * 60_000 }, now),
    ).toBe(" · Notifications on · Last notification 3 min ago");
    // A copy of the service from before notifications says nothing of them: off.
    expect(deviceNotifications({ kind: "ios" }, now)).toBe(" · Notifications off");
    expect(deviceNotifications({ kind: "browser", notifications: true }, now)).toBe("");
  });
});
