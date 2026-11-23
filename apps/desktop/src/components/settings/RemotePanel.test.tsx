import { describe, expect, it } from "vitest";
import { usage } from "./RemotePanel";

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
