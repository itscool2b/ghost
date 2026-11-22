import type { GlassEffect, OpenOutcome, StopReason } from "../bindings";

/** Labels and number formats for the info card, the transcript footer and the stress view. */

export function osName(os: string): string {
  switch (os) {
    case "macos":
      return "macOS";
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    default:
      return os;
  }
}

/** The file manager's name, for "Show in ..." buttons. */
export function fileManagerName(os: string): string {
  switch (os) {
    case "macos":
      return "Finder";
    case "windows":
      return "Explorer";
    default:
      return "file manager";
  }
}

export function glassLabel(glass: GlassEffect): string {
  switch (glass) {
    case "liquid_glass":
      return "Liquid Glass";
    case "vibrancy":
      return "Vibrancy";
    case "mica":
      return "Mica";
    case "acrylic":
      return "Acrylic";
    case "none":
      return "None (opaque)";
  }
}

export function outcomeLabel(outcome: OpenOutcome): string {
  switch (outcome.type) {
    case "created":
      return "created";
    case "opened":
      return "opened";
    case "migrated":
      return `upgraded from v${outcome.from}`;
    case "recovered_from_corruption":
      switch (outcome.salvage.type) {
        case "complete":
          return "repaired, nothing lost";
        case "partial":
          return "repaired, some records lost";
        case "nothing":
          return "started fresh (damaged copy kept)";
      }
  }
}

/**
 * How long this launch's work on the database took, for the diagnostics card: opening it (before
 * the window exists), startup recovery (which the first lists wait behind), and building
 * recovery's indexes when this launch built them. The next time opening the app is slow, this
 * says where.
 */
export function startupTimings(db: {
  readonly open_ms: number;
  readonly recovery_ms: number | null;
  readonly indexes_ms: number | null;
}): string {
  const ms = (value: number) => `${value.toLocaleString("en-US")} ms`;
  const parts = [`opened in ${ms(db.open_ms)}`];
  parts.push(
    db.recovery_ms === null ? "still starting" : `ready ${ms(db.recovery_ms)} after that`,
  );
  if (db.indexes_ms !== null) parts.push(`indexes built in ${ms(db.indexes_ms)}`);
  return parts.join(", ");
}

export function stopReasonLabel(reason: StopReason): string {
  switch (reason) {
    case "end_turn":
      return "Done";
    case "cancelled":
      return "Stopped";
    case "max_tokens":
      return "Hit the token limit";
    case "max_turn_requests":
      return "Hit the request limit";
    case "refusal":
      return "Refused";
    case "interrupted":
      return "Interrupted";
    case "error":
      return "Failed";
    case "other":
      return "Ended";
  }
}

/**
 * 950 ms, 1.2 s, 12 s, 2 min 5 s. Each step rounds to the precision it shows before choosing the
 * unit, so a value just under a boundary moves up a unit instead of printing "1000 ms", "10.0 s",
 * "60 s" or "1 min 60 s".
 */
export function formatDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return "0 ms";
  const wholeMs = Math.round(ms);
  if (wholeMs < 1000) return `${wholeMs} ms`;
  const tenths = Math.round(ms / 100);
  if (tenths < 100) return `${(tenths / 10).toFixed(1)} s`;
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} s`;
  return `${Math.floor(seconds / 60)} min ${seconds % 60} s`;
}

/**
 * 950, 12.5k, 1.2M. Rounds before choosing the unit (as `formatDuration` does), so 999,950 is
 * "1M", not "1000k". Halves round away from zero, the same for negative counts.
 */
export function formatCount(count: number): string {
  if (!Number.isFinite(count)) return "0";
  const sign = count < 0 ? "-" : "";
  const abs = Math.abs(count);
  const whole = Math.round(abs);
  if (whole < 1000) return `${whole === 0 ? "" : sign}${whole}`;
  // Tenths of a thousand, then of a million: integers, so the rounding is exact.
  const thousands = Math.round(abs / 100);
  if (thousands < 10_000) return `${sign}${trimZero((thousands / 10).toFixed(1))}k`;
  const millions = Math.round(abs / 100_000);
  return `${sign}${trimZero((millions / 10).toFixed(1))}M`;
}

function trimZero(value: string): string {
  return value.endsWith(".0") ? value.slice(0, -2) : value;
}

/** $0.0184 or 0.0184 EUR: four decimals, since per-turn costs are cents. */
export function formatCost(amount: number | null, currency: string): string | null {
  if (amount === null || !Number.isFinite(amount)) return null;
  const value = amount.toFixed(4);
  return currency === "USD" ? `$${value}` : `${value} ${currency}`;
}

/**
 * An agent's version as a line under or beside its name. Detection reports what the binary
 * printed, led by its label ("Claude Code 2.1.312"); beside the name that label only repeats it,
 * so it goes ("2.1.312"). A label that differs from the name (the CLI behind an adapter) stays.
 */
export function agentVersion(name: string, version: string): string {
  const lead = `${name} `;
  return version.length > lead.length &&
    version.slice(0, lead.length).toLowerCase() === lead.toLowerCase()
    ? version.slice(lead.length)
    : version;
}

/** Where an agent runs from. The demo agent runs inside o7, which Rust reports as "built in". */
export function builtInPath(path: string): string {
  return path === "built in" ? "Built in" : path;
}
