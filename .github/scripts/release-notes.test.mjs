import assert from "node:assert/strict";
import test from "node:test";
import { entries, FALLBACK, headline, MAX_LINES, notes } from "./release-notes.mjs";

const pr = (n) => `([#${n}](https://github.com/itscool2b/ghost/pull/${n}))`;
const changelog = (...days) =>
  [
    "# Changelog",
    "",
    "Intro.",
    "",
    ...days.flatMap(([day, ...lines]) => [`## ${day}`, "", ...lines, ""]),
  ].join("\n");
const OLD = changelog([
  "2026-11-18",
  `- The download page lists installers. They are unsigned. ${pr(146)}`,
]);
const NEW = changelog(
  [
    "2026-11-19",
    `- Ghost updates itself. An Update button shows when a newer version is out. ${pr(149)}`,
    `- Changelog dates: each change is under its real day. ${pr(147)}`,
  ],
  ["2026-11-18", `- The download page lists installers. They are unsigned. ${pr(146)}`],
);

test("a line's headline ends at its first full stop or colon, not inside a version or a name", () => {
  assert.equal(headline("Ghost updates itself. An Update button shows."), "Ghost updates itself");
  assert.equal(headline("Changelog dates: each change is under its real day."), "Changelog dates");
  assert.equal(headline("Version 0.1.1570 is out. More."), "Version 0.1.1570 is out");
  assert.equal(headline("No stop here"), "No stop here");
});

test("the notes are the headlines of the lines added since the last published build", () => {
  assert.deepEqual(
    entries(NEW).map((entry) => [entry.number, entry.day]),
    [
      [149, "2026-11-19"],
      [147, "2026-11-19"],
      [146, "2026-11-18"],
    ],
  );
  assert.equal(notes(NEW, OLD), "Ghost updates itself.\nChangelog dates.");
  // Nothing new for the person using the app: a build of dependency or tooling changes.
  assert.equal(notes(NEW, NEW), FALLBACK);
  assert.equal(notes("# Changelog\n", NEW), FALLBACK);
});

test("the first published build takes the newest day's lines", () => {
  assert.equal(notes(NEW, null), "Ghost updates itself.\nChangelog dates.");
});

test("a long list is cut and counted", () => {
  const lines = Array.from(
    { length: MAX_LINES + 3 },
    (_, i) => `- Change ${200 - i}. Detail. ${pr(200 - i)}`,
  );
  const text = notes(changelog(["2026-11-20", ...lines]), OLD);
  assert.equal(text.split("\n").length, MAX_LINES + 1);
  assert.equal(text.split("\n")[0], "Change 200.");
  assert.equal(text.split("\n").at(-1), "And 3 more.");
});
