import { describe, expect, it } from "vitest";
import type { Salvage } from "../bindings";
import {
  agentVersion,
  fileManagerName,
  formatCost,
  formatCount,
  formatDuration,
  glassLabel,
  osName,
  outcomeLabel,
  startupTimings,
  stopReasonLabel,
} from "./format";

describe("labels", () => {
  it("names systems, file managers and glass", () => {
    expect(osName("macos")).toBe("macOS");
    expect(osName("freebsd")).toBe("freebsd");
    expect(fileManagerName("windows")).toBe("Explorer");
    expect(fileManagerName("linux")).toBe("file manager");
    expect(glassLabel("liquid_glass")).toBe("Liquid Glass");
    expect(glassLabel("acrylic")).toBe("Acrylic");
    expect(glassLabel("none")).toBe("None (opaque)");
  });

  it("says how long this launch's work on the database took, as far as it is known", () => {
    expect(startupTimings({ open_ms: 12, recovery_ms: null, indexes_ms: null })).toBe(
      "opened in 12 ms, still starting",
    );
    expect(startupTimings({ open_ms: 4630, recovery_ms: 8, indexes_ms: null })).toBe(
      "opened in 4,630 ms, ready 8 ms after that",
    );
    expect(startupTimings({ open_ms: 12, recovery_ms: 86, indexes_ms: 1080 })).toBe(
      "opened in 12 ms, ready 86 ms after that, indexes built in 1,080 ms",
    );
  });

  it("describes what opening the database did", () => {
    expect(outcomeLabel({ type: "created" })).toBe("created");
    expect(outcomeLabel({ type: "migrated", from: 1, to: 2, backup: null })).toBe(
      "upgraded from v1",
    );
    const recovered = (salvage: Salvage) =>
      outcomeLabel({ type: "recovered_from_corruption", quarantined: "/x", salvage });
    expect(recovered({ type: "complete" })).toBe("repaired, nothing lost");
    expect(recovered({ type: "partial", tables: ["chats"] })).toBe("repaired, some records lost");
    expect(recovered({ type: "nothing" })).toBe("started fresh (damaged copy kept)");
  });

  it("names stop reasons", () => {
    expect(stopReasonLabel("end_turn")).toBe("Done");
    expect(stopReasonLabel("cancelled")).toBe("Stopped");
  });
});

describe("numbers", () => {
  it("formats durations at a readable precision", () => {
    expect(formatDuration(950)).toBe("950 ms");
    expect(formatDuration(1240)).toBe("1.2 s");
    expect(formatDuration(12_400)).toBe("12 s");
    expect(formatDuration(125_000)).toBe("2 min 5 s");
    expect(formatDuration(Number.NaN)).toBe("0 ms");
    expect(formatDuration(-5)).toBe("0 ms");
    expect(formatDuration(Number.POSITIVE_INFINITY)).toBe("0 ms");
  });

  it("moves up a unit when rounding reaches the next one", () => {
    // Each pair: the last value that stays in a unit, then the first that rounds into the next.
    expect(formatDuration(999.4)).toBe("999 ms");
    expect(formatDuration(999.6)).toBe("1.0 s");
    expect(formatDuration(9_949)).toBe("9.9 s");
    expect(formatDuration(9_960)).toBe("10 s");
    expect(formatDuration(59_400)).toBe("59 s");
    expect(formatDuration(59_600)).toBe("1 min 0 s");
    expect(formatDuration(119_400)).toBe("1 min 59 s");
    expect(formatDuration(119_600)).toBe("2 min 0 s");
    expect(formatDuration(3_599_600)).toBe("60 min 0 s");
  });

  it("formats token counts compactly", () => {
    expect(formatCount(950)).toBe("950");
    expect(formatCount(12_480)).toBe("12.5k");
    expect(formatCount(12_000)).toBe("12k");
    expect(formatCount(1_250_000)).toBe("1.3M");
    expect(formatCount(Number.NaN)).toBe("0");
    expect(formatCount(0)).toBe("0");
    expect(formatCount(-0.4)).toBe("0");
  });

  it("moves counts up a unit when rounding reaches 1000", () => {
    expect(formatCount(999.4)).toBe("999");
    expect(formatCount(999.5)).toBe("1k");
    expect(formatCount(12_450)).toBe("12.5k");
    expect(formatCount(999_949)).toBe("999.9k");
    expect(formatCount(999_950)).toBe("1M");
    expect(formatCount(999_999)).toBe("1M");
    expect(formatCount(1_049_999)).toBe("1M");
    expect(formatCount(-999_950)).toBe("-1M");
    expect(formatCount(-1_500)).toBe("-1.5k");
  });

  it("formats costs, or nothing when unknown", () => {
    expect(formatCost(0.0184, "USD")).toBe("$0.0184");
    expect(formatCost(0.5, "EUR")).toBe("0.5000 EUR");
    expect(formatCost(null, "USD")).toBeNull();
  });
});

describe("agent versions", () => {
  it("drops the label that only repeats the agent's name", () => {
    expect(agentVersion("Claude Code", "Claude Code 2.1.312")).toBe("2.1.312");
    expect(agentVersion("Grok Build", "Grok Build 1.0.70 (unverified)")).toBe(
      "1.0.70 (unverified)",
    );
    expect(agentVersion("Codex", "codex 0.162.1")).toBe("0.162.1");
  });

  it("keeps a version led by another label or by nothing", () => {
    expect(agentVersion("Claude Code", "0.1.0")).toBe("0.1.0");
    expect(agentVersion("OpenCode", "Node.js 22.1.0")).toBe("Node.js 22.1.0");
    expect(agentVersion("Pi", "Pi ")).toBe("Pi ");
  });
});
