/** Verify the real final gate configuration without running hosted CI, agents or sign-in. */
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path) => readFileSync(resolve(root, path), "utf8");
const ci = read(".github/workflows/ci.yml");
const build = read(".github/workflows/build.yml");
const desktop = JSON.parse(read("apps/desktop/package.json"));
const scripts = "apps/desktop/scripts";
// Native evidence probes drive real installed agents, which hosted runners do not have: documented
// manual checks (docs/plugin-library.md), not CI gates.
const manual = new Set(["check-native-mcp.mjs"]);

test("every desktop acceptance script runs directly or through a checked aggregator", () => {
  const expected = readdirSync(resolve(root, scripts)).filter((file) =>
    /^check-.*\.mjs$/u.test(file),
  );
  const covered = new Set();
  function include(file) {
    if (covered.has(file)) return;
    covered.add(file);
    const source = read(`${scripts}/${file}`);
    for (const match of source.matchAll(/from\s+["']\.\/(check-[^"']+\.mjs)["']/gu)) {
      include(match[1]);
    }
    for (const match of source.matchAll(
      /spawn\(\s*process\.execPath,\s*\[\s*resolve\(app,\s*["']scripts\/(check-[^"']+\.mjs)["']\)/gu,
    )) {
      include(match[1]);
    }
    // A suite that runs a list of checks on its own server (check-customization-suite.mjs).
    if (/resolve\(app,\s*["']scripts["'],\s*script\)/u.test(source)) {
      for (const match of source.matchAll(/["'](check-[\w-]+\.mjs)["']/gu)) include(match[1]);
    }
  }
  for (const match of ci.matchAll(/@ghost\/desktop\s+exec node scripts\/(check-[^\s]+\.mjs)/gu))
    include(match[1]);
  for (const match of ci.matchAll(/@ghost\/desktop\s+(check:[\w-]+)/gu)) {
    const command = desktop.scripts[match[1]];
    assert.ok(command, `Missing package command ${match[1]}`);
    const file = /^node scripts\/(check-[\w-]+\.mjs)$/u.exec(command);
    assert.ok(file, `Review acceptance command ${match[1]}: ${command}`);
    include(file[1]);
  }
  assert.deepEqual(
    expected.filter((file) => !covered.has(file) && !manual.has(file)),
    [],
    "Acceptance scripts missing from final CI",
  );
});

test("browser gates have explicit deadlines and retain standalone evidence", () => {
  for (const step of ci.split(/\n {6}- name:/u).slice(1)) {
    // Match individual command lines: a step can include the next job's metadata,
    // which makes a nested multiline wildcard backtrack on unrelated run blocks.
    if (
      !step
        .split(/\r?\n/u)
        .some((line) =>
          /^[ \t]*(?:run:[ \t]*)?pnpm --filter @ghost\/(?:desktop|web) (?:check:|exec node scripts\/check-|test:browser)/u.test(
            line,
          ),
        )
    )
      continue;
    const deadline = Number(/timeout-minutes: (\d+)\b/u.exec(step)?.[1]);
    const maximum = step.includes("node scripts/check-layout.mjs") ? 20 : 15;
    assert.ok(
      deadline >= 1 && deadline <= maximum,
      `Browser step needs a bounded deadline: ${step.split("\n")[0]}`,
    );
  }
  for (const artifact of [
    "setup-browser",
    "setup-actions-browser",
    "sidebar-footer-browser",
    "chat-auth-browser",
  ]) {
    assert.ok(ci.includes(`target/${artifact}`), `Missing browser evidence: ${artifact}`);
  }
});

test("the app is local-only: no account crate, feature, encrypted store or API build remains", () => {
  const workspace = read("Cargo.toml");
  assert.doesNotMatch(workspace, /aura-sync|apps\/api/u);
  assert.doesNotMatch(read("crates/ghost-core/Cargo.toml"), /account-sync|sqlcipher|aura-sync/u);
  assert.doesNotMatch(
    read("apps/desktop/src-tauri/Cargo.toml"),
    /account-sync|aura-sync|deep-link/u,
  );
  assert.doesNotMatch(read("Cargo.lock"), /name = "openssl-src"/u);
  assert.doesNotMatch(ci, /sqlcipher|account-sync|@ghost\/api|@auracode\/api|encrypted_storage/u);
  for (const workflow of [ci, build, read(".github/workflows/publish.yml")])
    assert.doesNotMatch(workflow, /select-native-perl|vendored OpenSSL/u);
  // The checkpoint comparison keeps its own job, on the systems its baseline supports.
  assert.match(ci, /checkpoint-benchmarks:[\s\S]*?os: \[macos-26-intel, windows-2025\]/u);
});

test("CI uses supported Intel macOS runners without changing universal packaging", () => {
  const labels = [...ci.matchAll(/\bmacos-26(?:-intel)?\b/gu)].map((match) => match[0]);
  assert.ok(labels.length > 0);
  assert.ok(labels.every((label) => label === "macos-26-intel"));
  assert.match(build, /os: macos-26\s+target: universal-apple-darwin/u);
});

test("unsigned packages use exact bindings and need no release secret or updater artifact", () => {
  const bindings = build.indexOf('export-bindings.sh check "$RUNNER_TEMP/bindings-build"');
  const bundle = build.indexOf('pnpm --filter @ghost/desktop bundle --target "$BUNDLE_TARGET"');
  assert.ok(
    bindings >= 0 && bundle > bindings,
    "Check exact generated bindings before bundling the frontend",
  );
  assert.doesNotMatch(
    build,
    /--signed-updater|secrets\.|TAURI_SIGNING_PRIVATE_KEY|GHOST_UPDATER_PUBLIC_KEY/u,
  );
  assert.match(build, /APPLE_SIGNING_IDENTITY=-/u);
  for (const target of [
    "universal-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
  ]) {
    assert.ok(build.includes(`target: ${target}`), `Missing acceptance bundle ${target}`);
  }
  const config = JSON.parse(read("apps/desktop/src-tauri/tauri.conf.json"));
  assert.ok(
    !config.bundle.createUpdaterArtifacts,
    "Unsigned bundles must not require updater signing",
  );
  assert.equal(config.bundle.windows.signCommand, undefined);
  assert.equal(config.bundle.macOS.signingIdentity, undefined);
});

test("Windows uses a pinned external bridge, matching runtime and separate evidence", () => {
  assert.match(ci, /cargo install tauri-driver --version 2\.1\.0 --locked/u);
  assert.match(ci, /prepare-webdriver\.ps1 -OutputDirectory/u);
  assert.match(
    ci,
    /EXPECTED_WEBVIEW_VERSION: \$\{\{ steps\.webdriver-native\.outputs\.webview_version \}\}/u,
  );
  const external = ci
    .split(/\n {6}- name:/u)
    .find((step) => step.includes("node .github/scripts/webdriver-smoke.mjs"));
  assert.ok(external, "The full-app WebDriver gate must not be replaced by embedded smoke");
  assert.match(external, /timeout-minutes: 8/u);
  assert.match(external, /target\/debug\/ghost\.exe/u);
  assert.match(ci, /path: \|\s+\$\{\{ runner\.temp \}\}\/webdriver-evidence/u);
  assert.match(ci, /node --test [^\n]*webdriver-smoke\.test\.mjs/u);
});
