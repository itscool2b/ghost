import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  checkChangelog,
  checkDates,
  EM_DASH,
  easternDay,
  MAX_WORDS_LENGTH,
} from "./check-changelog.mjs";

const script = fileURLToPath(new URL("./check-changelog.mjs", import.meta.url));
const pr = (n) => `([#${n}](https://github.com/itscool2b/ghost/pull/${n}))`;
const valid = [
  "# Changelog",
  "",
  "Intro.",
  "",
  "## 2026-11-03",
  "",
  `- Dictation works. ${pr(68)}`,
  `- The working line shows the model maker. ${pr(66)}`,
  "",
  "## 2026-11-02",
  "",
  `- Images preview in chat. ${pr(61)}`,
  "",
].join("\n");

test("a valid changelog passes and counts its entries", () => {
  assert.deepEqual(checkChangelog(valid), { problems: [], count: 3 });
  assert.deepEqual(checkChangelog(valid.replace(/\n/g, "\r\n")).problems, []);
});

for (const [name, text, problem] of [
  ["a missing title", valid.replace("# Changelog", "# Changes"), /line 1/],
  [
    "dates out of order",
    valid.replace("## 2026-11-02", "## 2026-11-04"),
    /newest first, once each/,
  ],
  ["a repeated date", valid.replace("## 2026-11-02", "## 2026-11-03"), /once each/],
  ["a repeated pull request", valid.replace(pr(61), pr(66)), /already listed/],
  ["numbers out of order in a day", valid.replace(pr(66), pr(70)), /newest first within a day/],
  [
    "an entry without its pull request",
    valid.replace(` ${pr(61)}`, ""),
    /ends with its pull request/,
  ],
  [
    "a link to another pull request",
    valid.replace("pull/61", "pull/60"),
    /links to pull request 60/,
  ],
  ["an entry before any date", valid.replace("Intro.", `- Early. ${pr(1)}`), /not under a date/],
  ["an em dash", valid.replace("Dictation works.", `Dictation ${EM_DASH} works.`), /em dash/],
  [
    "a line longer than the limit",
    valid.replace("Dictation works.", `Dictation works. ${"a".repeat(MAX_WORDS_LENGTH)}`),
    /#68 runs to \d+ characters before its link/,
  ],
  ["no entries", "# Changelog\n", /no entries/],
]) {
  test(`rejects ${name}`, () => {
    const { problems } = checkChangelog(text);
    assert.ok(
      problems.some((p) => problem.test(p)),
      `expected ${problem} in ${JSON.stringify(problems)}`,
    );
  });
}

test("a line of exactly the limit passes, whatever the length of its link", () => {
  const words = `${"a".repeat(MAX_WORDS_LENGTH - 1)}.`;
  assert.deepEqual(checkChangelog(valid.replace("Dictation works.", words)).problems, []);
});

test("the command reads CHANGELOG.md from the given folder and sets the exit code", (t) => {
  const root = mkdtempSync(join(tmpdir(), "auracode-changelog-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  writeFileSync(join(root, "CHANGELOG.md"), valid);
  const ok = spawnSync(process.execPath, [script, root], { encoding: "utf8" });
  assert.equal(ok.status, 0, ok.stderr);
  assert.match(ok.stdout, /lists 3 pull requests/);
  writeFileSync(join(root, "CHANGELOG.md"), valid.replace(pr(61), pr(66)));
  const bad = spawnSync(process.execPath, [script, root], { encoding: "utf8" });
  assert.equal(bad.status, 1);
  assert.match(bad.stderr, /already listed/);
});

test("a day is the US Eastern day of the moment, in winter and in summer", () => {
  // The evening merge that showed under tomorrow: 11:34pm on the 18th in New York.
  assert.equal(easternDay("2026-11-19T04:34:28Z"), "2026-11-18");
  assert.equal(easternDay("2026-11-19T05:00:00Z"), "2026-11-19");
  // Daylight time is four hours behind UTC, not five.
  assert.equal(easternDay("2026-07-01T03:59:59Z"), "2026-06-30");
  assert.equal(easternDay("2026-07-01T04:00:00Z"), "2026-07-01");
});

test("each entry must be under the Eastern day its pull request merged", () => {
  const merged = new Map([
    [68, "2026-11-03T22:10:00Z"],
    [66, "2026-11-03T15:00:00Z"],
    [61, "2026-11-03T01:56:55Z"],
  ]);
  assert.deepEqual(checkDates(valid, merged), []);
  const wrong = checkDates(valid, new Map([...merged, [61, "2026-11-03T05:30:00Z"]]));
  assert.equal(wrong.length, 1);
  assert.match(wrong[0], /#61 is under 2026-11-02, but it merged on 2026-11-03 Eastern/);
});

test("a pull request that has not merged belongs under today's Eastern day", () => {
  const merged = new Map([
    [66, "2026-11-03T15:00:00Z"],
    [61, "2026-11-03T01:56:55Z"],
  ]);
  // #68 is still open. Late on the 3rd in New York it is under the right day.
  assert.deepEqual(checkDates(valid, merged, new Date("2026-11-04T03:00:00Z")), []);
  // After midnight there, it has to move to the 4th before it merges.
  const next = checkDates(valid, merged, new Date("2026-11-04T05:30:00Z"));
  assert.equal(next.length, 1);
  assert.match(
    next[0],
    /#68 has not merged, so it belongs under today, 2026-11-04, not 2026-11-03/,
  );
});
