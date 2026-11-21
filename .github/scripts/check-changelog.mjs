// CHANGELOG.md lists the pull requests that changed the app (not the website, docs, tests or
// tooling alone; owner, 21 November 2026), newest first: `## YYYY-MM-DD` sections
// by the day it merged in US Eastern time (the owner's clock, 18 November 2026: an evening merge
// was showing under tomorrow's date, because the days were UTC), each line
// `- What changed. ([#N](https://github.com/itscool2b/o7/pull/N))`.
// Each such pull request adds its own line, in simple English about what changed for the person using
// o7 and short enough to read at a glance (docs/developing-in-o7.md, "Changelog").
//
//   node .github/scripts/check-changelog.mjs [folder]          the format, offline (pnpm lint)
//   node .github/scripts/check-changelog.mjs [folder] --dates  also each day against GitHub's
//                                                              merge time, through `gh`
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const ENTRY = /^- (\S.*\S) \(\[#(\d+)\]\(https:\/\/github\.com\/itscool2b\/o7\/pull\/(\d+)\)\)$/;
const DATE = /^## (\d{4}-\d{2}-\d{2})$/;
/** U+2014, written by code point so the character itself never appears in the source. */
export const EM_DASH = String.fromCharCode(0x2014);
/** The most characters a line's words may run to, not counting its pull request link. */
export const MAX_WORDS_LENGTH = 600;

export function checkChangelog(text) {
  const problems = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines[0] !== "# Changelog") problems.push("line 1 must be '# Changelog'");
  const seen = new Map();
  let date = null;
  let previousDate = null;
  let previousNumber = Infinity;
  lines.forEach((line, index) => {
    const at = `line ${index + 1}`;
    if (line.includes(EM_DASH))
      problems.push(`${at}: use a colon, comma or full stop, not an em dash`);
    const heading = line.match(DATE);
    if (heading) {
      date = heading[1];
      if (Number.isNaN(Date.parse(`${date}T00:00:00Z`)))
        problems.push(`${at}: ${date} is not a date`);
      if (previousDate !== null && date >= previousDate)
        problems.push(`${at}: ${date} must come before ${previousDate} (newest first, once each)`);
      previousDate = date;
      previousNumber = Infinity;
      return;
    }
    if (!line.startsWith("- ")) return;
    const entry = line.match(ENTRY);
    if (!entry) {
      problems.push(
        `${at}: an entry ends with its pull request, ([#N](https://github.com/itscool2b/o7/pull/N))`,
      );
      return;
    }
    const [, words, label, linked] = entry;
    const number = Number(label);
    if (words.length > MAX_WORDS_LENGTH)
      problems.push(
        `${at}: #${label} runs to ${words.length} characters before its link; keep a line to ${MAX_WORDS_LENGTH} (a headline, then one to three short sentences)`,
      );
    if (label !== linked) problems.push(`${at}: #${label} links to pull request ${linked}`);
    if (date === null) problems.push(`${at}: #${number} is not under a date`);
    if (seen.has(number))
      problems.push(`${at}: #${number} is already listed on line ${seen.get(number)}`);
    seen.set(number, index + 1);
    if (number >= previousNumber)
      problems.push(
        `${at}: #${number} must come before #${previousNumber} (newest first within a day)`,
      );
    previousNumber = number;
  });
  if (seen.size === 0) problems.push("no entries");
  return { problems, count: seen.size };
}

const EASTERN = new Intl.DateTimeFormat("en-CA", {
  timeZone: "America/New_York",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
});

/** The calendar day of a moment in US Eastern time, the changelog's clock, as YYYY-MM-DD. */
export function easternDay(moment) {
  return EASTERN.format(new Date(moment));
}

/**
 * Each entry's day against the day its pull request merged, in US Eastern time. `merged` maps a
 * pull request's number to the moment GitHub says it merged. A pull request that has not merged
 * yet (the one being written) must be under today's Eastern day: it moves if it merges tomorrow.
 */
export function checkDates(text, merged, now = new Date()) {
  const problems = [];
  const today = easternDay(now);
  let date = null;
  for (const line of text.replace(/\r\n/g, "\n").split("\n")) {
    const heading = line.match(DATE);
    if (heading) {
      date = heading[1];
      continue;
    }
    const entry = line.match(ENTRY);
    if (!entry || date === null) continue;
    const number = Number(entry[2]);
    const moment = merged.get(number);
    if (moment === undefined) {
      if (date !== today)
        problems.push(
          `#${number} has not merged, so it belongs under today, ${today}, not ${date}`,
        );
      continue;
    }
    const day = easternDay(moment);
    if (day !== date)
      problems.push(`#${number} is under ${date}, but it merged on ${day} Eastern (${moment})`);
  }
  return problems;
}

/** Every merged pull request's number and merge moment, asked of GitHub through `gh`. */
function mergedPullRequests(root) {
  const asked = spawnSync(
    "gh",
    ["pr", "list", "--state", "merged", "--limit", "1000", "--json", "number,mergedAt"],
    { cwd: root, encoding: "utf8", windowsHide: true },
  );
  if (asked.error || asked.status !== 0)
    throw new Error(
      `could not ask GitHub for merge times: ${asked.error?.message ?? asked.stderr}`,
    );
  return new Map(JSON.parse(asked.stdout).map((pull) => [pull.number, pull.mergedAt]));
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const args = process.argv.slice(2);
  const dates = args.includes("--dates");
  const root = resolve(args.find((arg) => !arg.startsWith("--")) ?? process.cwd());
  try {
    const text = readFileSync(resolve(root, "CHANGELOG.md"), "utf8");
    const { problems, count } = checkChangelog(text);
    if (dates) problems.push(...checkDates(text, mergedPullRequests(root)));
    if (problems.length) throw new Error(problems.join("\n  "));
    console.log(
      dates
        ? `CHANGELOG.md lists ${count} pull requests, newest first, each under the day it merged in US Eastern time.`
        : `CHANGELOG.md lists ${count} pull requests, newest first.`,
    );
  } catch (error) {
    console.error(`CHANGELOG.md needs fixing:\n  ${error.message}`);
    process.exitCode = 1;
  }
}
