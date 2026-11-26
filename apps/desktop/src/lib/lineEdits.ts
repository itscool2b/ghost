/**
 * The Dashboard's AI line edits: a year of days, each with the lines agents' edits changed, laid
 * out as a calendar of weeks (docs/design/visual-language.md, the Dashboard). The core counts by
 * quarter hour (`crates/ghost-core/src/chatstore/line_edits.rs`); this places each quarter hour in the
 * person's own day, so an edit at 11:50 pm counts on the day it was made here.
 */
import { getBrand } from "@ghost/ui";
import { commands, type LineEditBucket, type LineEdits } from "../bindings";
import { settle } from "./ipc";

export interface LineEditsApi {
  read(since: number): Promise<LineEdits>;
}

export const lineEditsApi: LineEditsApi = {
  read: (since) => settle(commands.dashboardLineEdits(since)),
};

/** The calendar's columns: a year of weeks, the last one this week. */
export const WEEKS = 53;

/** How strongly a day is lit: 0 for none, then the quarter of the active days it falls in. */
export type Level = 0 | 1 | 2 | 3 | 4;

export interface Day {
  /** Local midnight. */
  readonly date: Date;
  readonly lines: number;
  readonly level: Level;
  /** The column: 0 is the oldest week. */
  readonly week: number;
  /** The row: 0 is Sunday. */
  readonly weekday: number;
}

export interface MonthLabel {
  readonly name: string;
  readonly week: number;
}

export interface Calendar {
  /** From the first Sunday shown to today, in order. */
  readonly days: readonly Day[];
  readonly months: readonly MonthLabel[];
  readonly total: number;
  /** "August", or "November 2025" when the year shows two Novembers; null with no edits. */
  readonly mostActiveMonth: string | null;
  readonly mostActiveDay: Date | null;
  readonly longestStreak: number;
  /** Days in a row with edits up to today, or up to yesterday while today has none yet. */
  readonly currentStreak: number;
}

const LOCALE = "en-US";
const NUMBER = new Intl.NumberFormat(LOCALE);
const MONTH_SHORT = new Intl.DateTimeFormat(LOCALE, { month: "short" });
const MONTH_LONG = new Intl.DateTimeFormat(LOCALE, { month: "long" });
const DAY = new Intl.DateTimeFormat(LOCALE, { month: "short", day: "numeric", year: "numeric" });
const DAY_LONG = new Intl.DateTimeFormat(LOCALE, {
  weekday: "long",
  month: "long",
  day: "numeric",
  year: "numeric",
});

/** A month label closer than this many columns to the next one is left off. */
const MONTH_GAP = 3;

function midnight(date: Date): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate());
}

/** `days` later, by the calendar: a daylight saving change does not move it off midnight. */
function addDays(date: Date, days: number): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);
}

/** The first day the calendar shows: the Sunday 52 weeks before this week's. */
export function calendarStart(now: number): Date {
  const today = midnight(new Date(now));
  return addDays(today, -today.getDay() - (WEEKS - 1) * 7);
}

/** "2026-08-29": a day in local time. */
export function dayKey(date: Date): string {
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${date.getFullYear()}-${month}-${day}`;
}

/** The agents with line edits, the most first. */
export function agentsOf(buckets: readonly LineEditBucket[]): string[] {
  const totals = new Map<string, number>();
  for (const bucket of buckets)
    totals.set(bucket.agent, (totals.get(bucket.agent) ?? 0) + bucket.lines);
  return [...totals]
    .filter(([, lines]) => lines > 0)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .map(([agent]) => agent);
}

/** An agent's name: the one its chats show, its brand's, or its id. */
export function agentLabel(agent: string, names: ReadonlyMap<string, string>): string {
  if (agent === "") return "Other";
  const named = names.get(agent);
  if (named) return named;
  const brand = getBrand(agent);
  return brand.id === "generic" ? agent : brand.name;
}

/**
 * Lights each day by where it falls among the active days: the busiest is always full, the rest
 * by quarter, so one huge day does not leave every other one dim.
 */
function levelsOf(values: readonly number[]): (lines: number) => Level {
  const active = values.filter((lines) => lines > 0).sort((a, b) => a - b);
  const most = active[active.length - 1] ?? 0;
  const quarter = (share: number) => active[Math.max(0, Math.ceil(share * active.length) - 1)] ?? 0;
  const [first, second, third] = [quarter(0.25), quarter(0.5), quarter(0.75)];
  return (lines) => {
    if (lines <= 0) return 0;
    if (lines >= most) return 4;
    if (lines <= first) return 1;
    if (lines <= second) return 2;
    if (lines <= third) return 3;
    return 4;
  };
}

/** One agent's days (`null` for every agent's), from the calendar's first Sunday to today. */
export function calendarOf(
  buckets: readonly LineEditBucket[],
  agent: string | null,
  now: number,
): Calendar {
  const start = calendarStart(now);
  const today = midnight(new Date(now));
  const byDay = new Map<string, number>();
  for (const bucket of buckets) {
    if (agent !== null && bucket.agent !== agent) continue;
    const key = dayKey(new Date(bucket.at));
    byDay.set(key, (byDay.get(key) ?? 0) + bucket.lines);
  }
  const counted: { date: Date; lines: number }[] = [];
  for (let date = start; date <= today; date = addDays(date, 1))
    counted.push({ date, lines: byDay.get(dayKey(date)) ?? 0 });
  const level = levelsOf(counted.map((day) => day.lines));
  const days: Day[] = counted.map(({ date, lines }, index) => ({
    date,
    lines,
    level: level(lines),
    week: Math.floor(index / 7),
    weekday: date.getDay(),
  }));

  // A month is labelled over the first week that starts in it.
  const months: MonthLabel[] = [];
  for (let week = 0; week * 7 < days.length; week += 1) {
    const sunday = days[week * 7]?.date;
    const before = days[(week - 1) * 7]?.date;
    if (sunday !== undefined && (before === undefined || before.getMonth() !== sunday.getMonth()))
      months.push({ name: MONTH_SHORT.format(sunday), week });
  }
  const [firstMonth, secondMonth] = months;
  if (firstMonth && secondMonth && secondMonth.week - firstMonth.week < MONTH_GAP) months.shift();

  let total = 0;
  const byMonth = new Map<string, { first: Date; lines: number }>();
  let mostActiveDay: Day | null = null;
  let longestStreak = 0;
  let streak = 0;
  for (const day of days) {
    total += day.lines;
    const key = dayKey(day.date).slice(0, 7);
    const month = byMonth.get(key) ?? { first: day.date, lines: 0 };
    month.lines += day.lines;
    byMonth.set(key, month);
    if (day.lines > 0 && (mostActiveDay === null || day.lines >= mostActiveDay.lines))
      mostActiveDay = day;
    streak = day.lines > 0 ? streak + 1 : 0;
    longestStreak = Math.max(longestStreak, streak);
  }

  let busiest: { first: Date; lines: number } | null = null;
  for (const month of byMonth.values())
    if (month.lines > 0 && (busiest === null || month.lines > busiest.lines)) busiest = month;
  let mostActiveMonth: string | null = null;
  if (busiest !== null) {
    const name = MONTH_LONG.format(busiest.first);
    const sameName = [...byMonth.values()].filter(
      (month) => MONTH_LONG.format(month.first) === name,
    );
    mostActiveMonth = sameName.length > 1 ? `${name} ${busiest.first.getFullYear()}` : name;
  }

  // Today is not over: a streak that reached yesterday is still current.
  let currentStreak = 0;
  let index = days.length - 1;
  if ((days[index]?.lines ?? 0) === 0) index -= 1;
  while ((days[index]?.lines ?? 0) > 0) {
    currentStreak += 1;
    index -= 1;
  }

  return {
    days,
    months,
    total,
    mostActiveMonth,
    mostActiveDay: mostActiveDay?.date ?? null,
    longestStreak,
    currentStreak,
  };
}

export function formatLines(lines: number): string {
  return NUMBER.format(lines);
}

/** "Aug 29, 2026". */
export function formatDay(date: Date): string {
  return DAY.format(date);
}

/** "24 days", "1 day". */
export function formatStreak(days: number): string {
  return `${NUMBER.format(days)} ${days === 1 ? "day" : "days"}`;
}

/** What a day's square says under the pointer: "1,204 line edits on Friday, August 29, 2026". */
export function dayLabel(day: Pick<Day, "date" | "lines">): string {
  const when = DAY_LONG.format(day.date);
  if (day.lines === 0) return `No line edits on ${when}`;
  return `${NUMBER.format(day.lines)} line ${day.lines === 1 ? "edit" : "edits"} on ${when}`;
}
