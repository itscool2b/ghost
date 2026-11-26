// Build glue for the website's changelog: the repository's CHANGELOG.md, parsed when the site is
// built and written into app/changelog-data.ts as that module is loaded. Every pull request merged
// into main adds its line to the file (docs/developing-in-ghost.md, "Changelog"), and every push to
// main rebuilds the site, so the page is current without anyone editing it.
// `.github/scripts/check-changelog.mjs` keeps the file in the shape this reads. Only each line's
// words and its pull request number reach the page; the links stay out, since they lead to a
// private repository.
//
// The data replaces a real module rather than a virtual one on purpose: Vite names a shared chunk
// after the module it holds, a virtual module's name starts with an underscore, and the Worker that
// serves the site refuses such a file (apps/web-worker/src/manifest.rs).
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { Plugin } from "vite";
import type { ChangelogDay, ChangelogEntry } from "../app/changelog-types.ts";

const FILE = fileURLToPath(new URL("../../../CHANGELOG.md", import.meta.url));
const DATA = "/app/changelog-data.ts";

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

const DAY = /^## (\d{4})-(\d{2})-(\d{2})\s*$/;
const ENTRY = /^- (.+) \(\[#(\d+)\]\(https:\/\/github\.com\/[^)\s]+\/pull\/\d+\)\)\s*$/;

/**
 * Where a line's opening statement ends: its first colon or full stop that a space follows. Text
 * inside backticks is skipped, so `src/app.ts:42` does not split a line.
 */
function split(text: string): { head: string; rest: string } {
  let code = false;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === "`") code = !code;
    if (code) continue;
    if ((char === ":" || char === ".") && text[i + 1] === " ") {
      return { head: text.slice(0, i + 1), rest: text.slice(i + 1).trimStart() };
    }
  }
  return { head: text, rest: "" };
}

/** The days in the file, newest first as it lists them. Lines it does not recognise are skipped. */
export function parseChangelog(source: string): ChangelogDay[] {
  const days: { date: string; label: string; entries: ChangelogEntry[] }[] = [];
  for (const line of source.split(/\r?\n/)) {
    const day = DAY.exec(line);
    if (day) {
      const [, year = "", month = "", date = ""] = day;
      const name = MONTHS[Number(month) - 1] ?? month;
      days.push({
        date: `${year}-${month}-${date}`,
        label: `${Number(date)} ${name} ${year}`,
        entries: [],
      });
      continue;
    }
    const entry = ENTRY.exec(line);
    const current = days.at(-1);
    if (!entry || !current) continue;
    const [, text = "", pull = ""] = entry;
    current.entries.push({ ...split(text), pull: Number(pull) });
  }
  return days.filter((day) => day.entries.length > 0);
}

/**
 * Fills app/changelog-data.ts with the parsed days as it is loaded, and reloads it in development
 * when CHANGELOG.md changes.
 */
export function changelogPlugin(): Plugin {
  return {
    name: "auracode-changelog",
    enforce: "pre",
    transform(_code, id) {
      const path = (id.split("?")[0] ?? "").replaceAll("\\", "/");
      if (!path.endsWith(DATA)) return null;
      this.addWatchFile(FILE);
      const days = parseChangelog(readFileSync(FILE, "utf8"));
      if (days.length === 0) throw new Error(`No changelog entries could be read from ${FILE}.`);
      return { code: `export default ${JSON.stringify(days)};`, map: null };
    },
  };
}
