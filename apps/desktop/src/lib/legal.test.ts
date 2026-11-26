import { describe, expect, it } from "vitest";
import { acceptedLegal, acceptLegal, LEGAL_VERSION, type LegalStorage, legalNotice } from "./legal";

function memory(
  initial: Record<string, string> = {},
): LegalStorage & { values: Record<string, string> } {
  const values = { ...initial };
  return {
    values,
    getItem: (key) => values[key] ?? null,
    setItem: (key, value) => {
      values[key] = value;
    },
  };
}

describe("the Terms of Service and Privacy Policy notice", () => {
  it("shows once to a copy that never accepted them, and not after OK", () => {
    const store = memory();
    expect(legalNotice(store)?.title).toBe("Ghost has Terms of Service and a Privacy Policy");
    expect(legalNotice(store)?.detail).toMatch(/BASH\.AI LLC.*Using Ghost means you agree to them/);
    acceptLegal(store);
    expect(acceptedLegal(store)).toBe(LEGAL_VERSION);
    expect(legalNotice(store)).toBeNull();
  });

  it("says they changed to a copy that accepted an older version", () => {
    const store = memory({ "o7.legal.accepted": "2026-01-01" });
    expect(legalNotice(store)?.title).toBe("Ghost's Terms of Service and Privacy Policy changed");
  });

  it("still shows when storage cannot be read, and accepting never throws", () => {
    const broken: LegalStorage = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("full");
      },
    };
    expect(legalNotice(broken)).not.toBeNull();
    expect(() => acceptLegal(broken)).not.toThrow();
  });
});
