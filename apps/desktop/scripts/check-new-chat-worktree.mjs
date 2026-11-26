/**
 * New chat in its own worktree, in Chromium and WebKit, against the development fixture that
 * imitates Rust's review (src/dev/newChatWorktreeFixture.ts). No Git command, folder or agent runs.
 *
 * It checks that a Git project folder defaults to a worktree that Create makes with no other click;
 * that the chip's popover switches to Local, starts from another branch, keeps the generated name,
 * refuses an existing branch name beside the field (and Create stops on it), and names a branch
 * with Enter without creating the chat; that a folder with no commit falls back to Local with its
 * reason and a plain folder shows no chip; that a stale review is reviewed again before the next
 * Create; that a saved setup command waits in the owned terminal and the agent starts after it;
 * and that every state has no axe violations, Escape returns focus to the chip, and reduced motion
 * stills the popover.
 *
 * Usage: node scripts/check-new-chat-worktree.mjs [--server URL] [--engines chromium,webkit]
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";
import { choose } from "./choose.mjs";

// Sheets fade in and controls settle their colours; audit only once every finite animation ends.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

const { values } = parseArgs({
  options: { server: { type: "string" }, engines: { type: "string", default: "chromium,webkit" } },
});
const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = resolve(app, "../../target/new-chat-browser");
const url = values.server ?? "http://127.0.0.1:1474";
const require = createRequire(import.meta.url);
let server;
let serverOutput = "";

async function ready() {
  for (let attempt = 0; attempt < 150; attempt++) {
    if (server?.exitCode !== null && server?.exitCode !== undefined) throw new Error(serverOutput);
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      /* Startup has not bound its port yet. */
    }
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`Preview did not start: ${serverOutput}`);
}

// The preview publishes its fixture from an effect after the first render, so wait for it.
const calls = async (page) => {
  await page.waitForFunction(() => Boolean(window.newChatFixture));
  return page.evaluate(() => window.newChatFixture.calls());
};

async function audit(page, name, engine, scans) {
  await settled(page);
  const { violations } = await new AxeBuilder({ page }).analyze();
  scans.push(name);
  assert.deepEqual(
    violations.map((violation) => ({
      rule: violation.id,
      nodes: violation.nodes.map((node) => node.target),
    })),
    [],
    `${engine}/${name}: accessibility violations`,
  );
  await page.screenshot({ path: resolve(out, `${engine}-${name}.png`) });
}

async function open(page, scenario) {
  await page.goto(`${url}/?preview=${scenario}`);
  await page.getByTestId("new-chat").waitFor();
}

async function chipReady(page, mode) {
  const chip = page.getByTestId("new-chat-worktree");
  await page.locator(`[data-testid="new-chat-worktree"][data-test-mode="${mode}"]`).waitFor();
  return chip;
}

async function openPopover(page) {
  await page.getByTestId("new-chat-worktree").locator("summary").click();
  await page.locator(".new-chat-worktree-panel").waitFor();
}

/** A change shows at once and is reviewed after it; this waits for the review to settle. */
async function reviewed(page) {
  await page.locator(".new-chat-worktree-pending").waitFor({ state: "detached" });
}

async function inspect(engine, launcher) {
  const browser = await launcher.launch({ headless: true });
  const scans = [];
  try {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      reducedMotion: "reduce",
      colorScheme: "dark",
    });
    const page = await context.newPage();
    page.setDefaultTimeout(12000);
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));

    // A Git project folder: Worktree by default, and Create makes it with no other click.
    await open(page, "new-worktree");
    const chip = await chipReady(page, "worktree");
    assert.equal(await chip.getAttribute("data-test-start"), "main");
    assert.equal(await chip.getAttribute("data-test-branch"), "ghost/7f3a9c2e");
    await page.getByText("Starts in a separate worktree", { exact: true }).waitFor();
    assert.equal(
      await chip.locator("summary").getAttribute("aria-label"),
      "Worktree from main on ghost/7f3a9c2e. Where this chat works",
    );
    await audit(page, "worktree-chip", engine, scans);
    await page.getByTestId("new-chat-create").click();
    await page.locator('[data-test-phase="live"]').waitFor();
    assert.deepEqual(await calls(page), ["review:current:generated:false:true", "apply:fixture-1"]);

    // The popover: another start keeps the generated name; Escape returns focus to the chip.
    await open(page, "new-worktree");
    await chipReady(page, "worktree");
    await openPopover(page);
    await audit(page, "worktree-popover", engine, scans);
    assert.equal(
      await page
        .locator(".new-chat-worktree-popover > .chat-popover-panel")
        .evaluate((panel) => getComputedStyle(panel).animationName),
      "none",
      "reduced motion stills the popover",
    );
    await choose(page.getByLabel("Start from", { exact: true }), "release/1.4");
    await page
      .locator('[data-testid="new-chat-worktree"][data-test-start="release/1.4"]')
      .waitFor();
    await reviewed(page);
    assert.equal(await chip.getAttribute("data-test-branch"), "ghost/7f3a9c2e");
    assert.equal(
      await page.getByRole("switch", { name: /Bring uncommitted changes/ }).isDisabled(),
      true,
      "changes come along only from the current branch",
    );
    await page.keyboard.press("Escape");
    assert.equal(await page.locator(".new-chat-worktree-popover").getAttribute("open"), null);
    assert.equal(
      await chip.locator("summary").evaluate((summary) => summary === document.activeElement),
      true,
      "Escape returns focus to the chip",
    );
    assert.deepEqual((await calls(page)).slice(-1), [
      "review:release/1.4:ghost/7f3a9c2e:false:true",
    ]);

    // An existing branch name is refused beside its field; Create stops and shows it again.
    await openPopover(page);
    const name = page.getByLabel("New branch", { exact: true });
    await name.fill("release/1.4");
    await page.getByRole("button", { name: "Apply", exact: true }).click();
    await page.getByRole("alert").filter({ hasText: "already exists" }).waitFor();
    assert.equal(await name.getAttribute("aria-invalid"), "true");
    assert.equal(await chip.getAttribute("data-test-branch"), "ghost/7f3a9c2e");
    await audit(page, "branch-error", engine, scans);
    await page.keyboard.press("Escape");
    await page.getByTestId("new-chat-create").click();
    await page.locator(".new-chat-worktree-popover[open]").waitFor();
    assert.equal(
      (await calls(page)).some((call) => call.startsWith("apply")),
      false,
    );
    // Enter names the branch and never submits New chat around the field.
    await name.fill("ghost/sidebar-scan");
    await name.press("Enter");
    await page
      .locator('[data-testid="new-chat-worktree"][data-test-branch="ghost/sidebar-scan"]')
      .waitFor();
    await reviewed(page);
    assert.equal(
      (await calls(page)).some((call) => call.startsWith("apply")),
      false,
    );
    await page.keyboard.press("Escape");
    await page.getByTestId("new-chat-create").click();
    await page.locator('[data-test-phase="live"]').waitFor();
    assert.equal((await calls(page)).at(-1), "apply:fixture-3");

    // Local, chosen in the popover, creates the chat in the project folder.
    await open(page, "new-worktree");
    await chipReady(page, "worktree");
    await openPopover(page);
    await page.locator('[data-test-mode-choice="local"]').click();
    await chipReady(page, "local");
    await page.keyboard.press("Escape");
    await page.getByText("Starts in this folder", { exact: true }).waitFor();
    await audit(page, "local", engine, scans);
    await page.getByTestId("new-chat-create").click();
    await page.locator('[data-test-phase="live"]').waitFor();
    assert.deepEqual(await calls(page), [
      "review:current:generated:false:true",
      "create-local:folder-auracode",
    ]);

    // A stale review is refused, reviewed again, and the next Create applies the fresh one.
    await open(page, "new-worktree");
    await chipReady(page, "worktree");
    await page.evaluate(() => sessionStorage.setItem("new-chat-stale", "yes"));
    await page.getByTestId("new-chat-create").click();
    await page.getByRole("alert").filter({ hasText: "expired or was already used" }).waitFor();
    await page.waitForFunction(() => window.newChatFixture?.calls().length === 3);
    await reviewed(page);
    await page.getByTestId("new-chat-create").click();
    await page.locator('[data-test-phase="live"]').waitFor();
    assert.deepEqual(
      (await calls(page)).filter((call) => call.startsWith("apply")),
      ["apply:fixture-1", "apply:fixture-2"],
    );

    // No commit yet: Local, with the reason under the chip. A plain folder shows no chip.
    await open(page, "new-unavailable");
    await chipReady(page, "local");
    await page
      .getByTestId("new-chat-worktree-reason")
      .filter({ hasText: "no commit yet" })
      .waitFor();
    await audit(page, "unavailable", engine, scans);
    await page.getByTestId("new-chat-create").click();
    await page.locator('[data-test-phase="live"]').waitFor();
    assert.equal((await calls(page)).at(-1), "create-local:folder-auracode");
    await open(page, "new-plain");
    await page.waitForFunction(() => window.newChatFixture?.calls().length === 1);
    // "Checking Git" shows while the review runs, then nothing: the chat works in the folder.
    await page.getByTestId("new-chat-worktree").waitFor({ state: "detached" });
    await page.getByText("Starts in this folder", { exact: true }).waitFor();
    await audit(page, "plain", engine, scans);

    // A saved setup: files copied, the command waits in the owned terminal, then the agent starts.
    await open(page, "new-setup");
    await chipReady(page, "worktree");
    await openPopover(page);
    await page.getByText("Copies 2 files, then runs pnpm install --frozen-lockfile.").waitFor();
    await page.keyboard.press("Escape");
    await audit(page, "setup-review", engine, scans);
    await page.getByTestId("new-chat-create").click();
    await page.locator('[data-test-phase="live"]').waitFor();
    assert.equal(
      (await calls(page)).includes("setup-start"),
      false,
      "New chat does not launch setup from a renderer callback",
    );
    assert.equal(
      (await calls(page)).some((call) => call.startsWith("start:")),
      false,
    );
    await page.getByTestId("composer-input").waitFor();

    // Nothing waits for the retained list: a list that never arrives still lets the chat start.
    await open(page, "new-worktree&retained=never");
    await chipReady(page, "worktree");
    await page.locator('[data-testid="new-chat-create"]:not([disabled])').waitFor();

    // Worktrees kept from chats that are gone: one quiet line, closed, that opens to their names
    // (each start of Ghost used to list every worktree as a box of two full paths).
    await open(page, "new-worktree&retained=6");
    await chipReady(page, "worktree");
    const retained = page.getByTestId("new-chat-retained");
    await retained.waitFor();
    assert.equal(await retained.evaluate((node) => node.open), false, "closed until opened");
    assert.match(await retained.locator("summary").textContent(), /Retained worktrees\s*6/);
    await retained.locator("summary").click();
    const kept = retained.locator(".git-worktrees > li");
    assert.equal(await kept.count(), 6);
    assert.match(await kept.first().textContent(), /auracode · 420476c6724f/);
    assert.equal(await retained.locator("code").count(), 0, "no spelled-out paths");
    await audit(page, "retained", engine, scans);
    // One whose creation stopped half way opens the line by itself.
    await open(page, "new-worktree&retained=broken");
    await chipReady(page, "worktree");
    await page.getByTestId("new-chat-retained").waitFor();
    assert.equal(
      await page.getByTestId("new-chat-retained").evaluate((node) => node.open),
      true,
      "an operation that needs recovery shows at once",
    );

    assert.deepEqual(errors, [], "browser errors");
    console.log(
      `${engine}: ${scans.length} accessibility scans (${scans.join(", ")}); zero-click worktree, popover, refused and typed names, Local, stale review, fallback, plain folder, setup terminal, no wait for retained worktrees and retained worktrees passed`,
    );
  } finally {
    await browser.close();
  }
}

try {
  await mkdir(out, { recursive: true });
  if (!values.server) {
    const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
    server = spawn(
      process.execPath,
      [vite, "--host", "127.0.0.1", "--port", "1474", "--strictPort"],
      { cwd: app, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
    );
    for (const stream of [server.stdout, server.stderr])
      stream.on("data", (data) => {
        serverOutput = (serverOutput + data).slice(-8192);
      });
  }
  await ready();
  for (const engine of values.engines.split(",")) {
    const launcher = { chromium, webkit }[engine];
    assert.ok(launcher, `Unsupported engine: ${engine}`);
    await inspect(engine, launcher);
  }
} finally {
  server?.kill();
}
