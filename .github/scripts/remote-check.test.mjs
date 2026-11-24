import assert from "node:assert/strict";
import test from "node:test";
import { dispatchFields, formatBill, parseOptions, summarizeBill } from "./remote-check.mjs";

test("with no options a run checks Linux with the whole workspace, Clippy and the frontend", () => {
  assert.deepEqual(parseOptions([]), {
    os: "linux",
    tests: "--workspace",
    clippy: true,
    frontend: true,
    smoke: "none",
    browser: "none",
    ref: undefined,
    watch: undefined,
    wait: true,
  });
});

test("--ref names a branch to check as GitHub has it, and only a plain branch name", () => {
  assert.equal(parseOptions(["--ref", "main"]).ref, "main");
  assert.equal(parseOptions(["--ref", "codex/check-ref"]).ref, "codex/check-ref");
  assert.throws(() => parseOptions(["--ref", "-main"]), /--ref takes a branch name/u);
  assert.throws(() => parseOptions(["--ref", "main; echo"]), /--ref takes a branch name/u);
  assert.throws(() => parseOptions(["--ref", "a..b"]), /--ref takes a branch name/u);
});

test("options narrow the run and pass through as workflow fields", () => {
  const options = parseOptions([
    "--os",
    "windows",
    "--tests",
    "-p o7-core --lib chatstore",
    "--no-clippy",
    "--no-frontend",
    "--smoke",
    "stream chat",
    "--browser",
    "check:chat --engines chromium; check:settings",
  ]);
  assert.deepEqual(dispatchFields(options), [
    "-f",
    "os=windows",
    "-f",
    "tests=-p o7-core --lib chatstore",
    "-f",
    "clippy=false",
    "-f",
    "frontend=false",
    "-f",
    "smoke=stream chat",
    "-f",
    "browser=check:chat --engines chromium; check:settings",
  ]);
});

test("unknown systems, smoke scenarios, run ids and stray arguments are refused", () => {
  assert.throws(() => parseOptions(["--os", "freebsd"]), /--os must be one of/u);
  assert.throws(() => parseOptions(["--smoke", "stream crash"]), /--smoke takes/u);
  assert.throws(() => parseOptions(["--watch", "12; rm -rf"]), /--watch takes a run id/u);
  assert.throws(() => parseOptions(["extra"]), /Unexpected argument/u);
  assert.throws(() => parseOptions(["--bogus"]), /Unknown option/u);
});

test("empty strings mean none, and a watch takes no new run", () => {
  const options = parseOptions([
    "--tests",
    " ",
    "--smoke",
    "",
    "--watch",
    "45468004911",
    "--no-wait",
  ]);
  assert.equal(options.tests, "none");
  assert.equal(options.smoke, "none");
  assert.equal(options.watch, "45468004911");
  assert.equal(options.wait, false);
});

test("the bill adds up the net amounts, largest part first, and leaves out free parts", () => {
  const bill = summarizeBill({
    usageItems: [
      { product: "Actions", sku: "actions_linux", netAmount: 1.974 },
      { product: "Actions", sku: "actions_macos", netAmount: 75.578 },
      { product: "Actions", sku: "actions_storage", netAmount: 0.095964841 },
      { product: "Actions", sku: "actions_windows", netAmount: 16.964 },
      { product: "Actions", sku: "actions_linux_arm", netAmount: 0 },
    ],
  });
  assert.equal(
    formatBill(bill),
    "$94.61 this month so far (Mac $75.58, Windows $16.96, Linux $1.97, storage $0.10)",
  );
  assert.equal(formatBill(summarizeBill({ usageItems: [] })), "$0.00 this month so far");
  assert.equal(formatBill(summarizeBill(undefined)), "$0.00 this month so far");
});
