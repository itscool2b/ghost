import { describe, expect, it } from "vitest";
import { versionName, versionWithBuild } from "./version";

describe("a version as people are shown it", () => {
  it("is its first two numbers, with the build where copies must be told apart", () => {
    expect(versionName("0.1.1787")).toBe("0.1");
    expect(versionWithBuild("0.1.1787")).toBe("0.1 (build 1787)");
    expect(versionName("1.0.2000")).toBe("1.0");
    expect(versionWithBuild(" 12.3.4 ")).toBe("12.3 (build 4)");
  });
  it("leaves anything else as it is", () => {
    for (const odd of ["", "0.1", "0.1.0-dev", "nightly"]) {
      expect(versionName(odd)).toBe(odd);
      expect(versionWithBuild(odd)).toBe(odd);
    }
  });
});
