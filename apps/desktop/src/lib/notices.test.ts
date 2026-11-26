import { describe, expect, it } from "vitest";
import type { AppInfo } from "../bindings";
import { withLateNotices } from "./notices";

const INFO: AppInfo = {
  app_version: "0.1.0",
  tauri_version: "2.12.5",
  os: "macos",
  os_version: "26.6.2",
  arch: "aarch64",
  data_dir: "/data",
  device: null,
  profile: null,
  profile_switching: false,
  db: null,
  glass: "none",
  launch: { type: "normal" },
  notices: ["Your data was upgraded from version 1 to 2."],
  recovery_notice: "Your database was damaged, so Ghost rebuilt it.",
  fatal: null,
  media: true,
  media_notice: null,
};

describe("notices that arrive after startup", () => {
  it("are added to what startup reported, and nothing else changes", () => {
    const sweep =
      "Ghost stopped 2 agent processes left running by an earlier session that ended unexpectedly.";
    const later = { ...INFO, notices: [...INFO.notices, sweep], recovery_notice: null };
    const merged = withLateNotices(INFO, later);
    expect(merged.notices).toEqual([...INFO.notices, sweep]);
    // The repair notice follows its own dismissal, not a later read.
    expect(merged.recovery_notice).toBe(INFO.recovery_notice);
    expect(withLateNotices(merged, later)).toBe(merged);
    expect(withLateNotices(INFO, INFO)).toBe(INFO);
  });
});
