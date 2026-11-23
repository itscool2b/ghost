import { describe, expect, it } from "vitest";
import { told, usage } from "./RemotePanel";

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
