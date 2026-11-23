import assert from "node:assert/strict";
import { test } from "node:test";
import { checkVersion, inApp, newer, parse } from "./check-version.mjs";

test("a version is three plain numbers", () => {
  assert.deepEqual(parse("0.2.1\n"), [0, 2, 1]);
  assert.deepEqual(parse("12.0.65535"), [12, 0, 65535]);
  for (const bad of ["0.2", "0.2.1.4", "v0.2.1", "0.02.1", "0.2.1-beta", "0.2.65536", ""])
    assert.equal(parse(bad), null, bad);
  assert.match(checkVersion("0.2", "0.2.0", []).join(), /three plain numbers/);
});

test("newer compares number by number", () => {
  assert.equal(newer([0, 2, 0], [0, 1, 1787]), true, "0.2.0 is newer than the last 0.1 build");
  assert.equal(newer([0, 2, 10], [0, 2, 9]), true);
  assert.equal(newer([0, 3, 0], [0, 2, 9]), true);
  assert.equal(newer([0, 2, 1], [0, 2, 1]), false);
  assert.equal(newer([0, 2, 0], [0, 10, 0]), false);
});

test("a branch that changes the app raises the version", () => {
  assert.equal(inApp("apps/desktop/src/App.tsx"), true);
  assert.equal(inApp("Cargo.lock"), true);
  assert.equal(inApp("apps/web/app/root.tsx"), false);
  assert.equal(inApp("docs/releases.md"), false);
  assert.equal(inApp("Cargo.toml.bak"), false);

  const unraised = checkVersion("0.2.4", "0.2.4", ["crates/o7-core/src/lib.rs"]);
  assert.equal(unraised.length, 1);
  assert.match(unraised[0], /0\.2\.5 for a small change or a fix, 0\.3\.0 for a big feature/);
  assert.equal(checkVersion("0.2.3", "0.2.4", ["crates/a.rs"]).length, 1, "lower is refused too");
  assert.deepEqual(checkVersion("0.2.5", "0.2.4", ["crates/a.rs"]), []);
  assert.deepEqual(checkVersion("0.3.0", "0.2.4", ["crates/a.rs"]), []);
  // The website, the docs or the tests alone are not published.
  assert.deepEqual(checkVersion("0.2.4", "0.2.4", ["apps/web/app/x.tsx", "docs/a.md"]), []);
  // Main unreadable (a shallow checkout): the shape only.
  assert.deepEqual(checkVersion("0.2.4", null, null), []);
});
