/**
 * The docked terminal panel, in Chromium and WebKit, against the development fixture that imitates
 * Rust's project tools review and an owned shell (src/dev/terminalPanelFixture.ts). No process runs.
 *
 * It checks that nothing is reviewed or reserved until the person asks; that Ctrl+` opens the
 * panel with one reviewed shell in the chat's folder and focus in it, typing reaches that shell,
 * and Ctrl+` from inside the shell hides the panel without stopping it and returns focus to the
 * composer; that the header menu and the command palette offer the same toggle; that New terminal
 * and the shell menu add numbered tabs, arrows move between tabs, F2 renames, Delete and the close
 * button stop a shell; that the top edge resizes on the 4px grid from the keyboard and the
 * pointer; that an exited shell says so; that the eighth started shell is the last (a tab that
 * never showed holds no shell and does not count); that a refused start shows Rust's words and
 * Try again; and that every state has no axe violations.
 *
 * Usage: node scripts/check-terminal-panel.mjs [--server URL] [--engines chromium,webkit]
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
const out = resolve(app, "../../target/terminal-panel-browser");
const port = "1426";
const url = values.server ?? `http://127.0.0.1:${port}`;
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

const calls = (page) => page.evaluate(() => window.terminalPanelFixture.calls());
const panel = (page) => page.getByTestId("terminal-panel");
const tabs = (page) => page.getByRole("tab").and(page.getByTestId("terminal-panel-tab"));
/** Waits for text on the shell's screen as the screen reader tree has it (the renderer may be a canvas). */
const shows = (page, text) =>
  page.waitForFunction(
    (wanted) =>
      document
        .querySelector(".terminal-panel-viewport .xterm-accessibility-tree")
        ?.textContent?.includes(wanted),
    text,
  );
const focusInShell = (page) =>
  page.waitForFunction(() => document.activeElement?.classList.contains("xterm-helper-textarea"));

async function audit(page, name, engine, scans) {
  await settled(page);
  // The WCAG rules, as check-chat scans the chat shell (which has no page heading of its own).
  const { violations } = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
    .analyze();
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

    // Nothing is reviewed or reserved until the person asks for a terminal.
    await page.goto(`${url}/?preview=settled`);
    await page.locator('[data-test-phase="live"]').waitFor();
    assert.equal(await panel(page).count(), 0);
    assert.deepEqual(await calls(page), []);

    // Ctrl+`: one reviewed shell in the chat's folder, focus in it, typing reaches it.
    await page.getByTestId("composer-input").click();
    await page.keyboard.press("Control+Backquote");
    await panel(page).waitFor();
    await shows(page, "sidebar-scan>");
    await focusInShell(page);
    assert.deepEqual(
      (await calls(page)).map((call) => call.replace(/:\d+x\d+$/, "")),
      ["review", "reserve", "start:power_shell"],
    );
    assert.equal(await tabs(page).count(), 1);
    assert.equal(await tabs(page).first().getAttribute("aria-selected"), "true");
    await page
      .locator(".terminal-panel-folder")
      .getByText("C:\\auracode\\worktrees\\sidebar-scan", { exact: true })
      .waitFor();
    await page.keyboard.type("pnpm test");
    await page.keyboard.press("Enter");
    await shows(page, "Nothing ran: pnpm test");
    await audit(page, "open", engine, scans);

    // Ctrl+` from inside the shell hides the panel, keeps the shell and returns to the composer.
    await page.keyboard.press("Control+Backquote");
    await panel(page).waitFor({ state: "detached" });
    await page.waitForFunction(
      () => document.activeElement?.getAttribute("data-testid") === "composer-input",
    );
    assert.equal((await calls(page)).includes("close"), false);
    // The header menu offers the same toggle, with its keys.
    await page.locator('summary[aria-label="Chat details and controls"]').click();
    const toggle = page.getByTestId("chat-terminal-toggle");
    assert.equal((await toggle.textContent())?.startsWith("Show terminal"), true);
    assert.equal(await toggle.getAttribute("aria-keyshortcuts"), "Control+` Meta+`");
    await toggle.click();
    await panel(page).waitFor();
    await shows(page, "Nothing ran: pnpm test");
    assert.equal((await calls(page)).filter((call) => call === "review").length, 1);
    // So does the command palette, by name (Ctrl+K inside the shell stays the shell's).
    await focusInShell(page);
    await page.getByTestId("composer-input").click();
    await page.keyboard.press("Control+k");
    await page.getByRole("dialog").getByText("Hide terminal", { exact: true }).waitFor();
    await page.getByRole("dialog").getByText("New terminal", { exact: true }).click();
    await page.getByRole("dialog").waitFor({ state: "detached" });
    await tabs(page).nth(1).waitFor();
    assert.equal(await tabs(page).nth(1).textContent(), "PowerShell 2");

    // The shell menu adds a Command Prompt; arrows move between tabs.
    await page.locator('.terminal-panel-shells > summary[aria-label="New terminal with"]').click();
    await page.getByRole("button", { name: "Command Prompt", exact: true }).click();
    await tabs(page).nth(2).waitFor();
    await shows(page, "Microsoft Windows");
    assert.equal(await tabs(page).nth(2).textContent(), "Command Prompt");
    assert.equal(await tabs(page).nth(2).getAttribute("aria-selected"), "true");
    await tabs(page).nth(2).focus();
    await page.keyboard.press("Home");
    assert.equal(await tabs(page).first().getAttribute("aria-selected"), "true");
    await shows(page, "Nothing ran: pnpm test");
    await page.keyboard.press("ArrowLeft");
    assert.equal(await tabs(page).nth(2).getAttribute("aria-selected"), "true");
    await audit(page, "tabs", engine, scans);

    // F2 renames; Enter keeps it, and an emptied name gives the shell's name back.
    await page.keyboard.press("F2");
    await page.getByLabel("Rename Command Prompt", { exact: true }).fill("dev server");
    await page.keyboard.press("Enter");
    await tabs(page).getByText("dev server", { exact: true }).waitFor();

    // The top edge resizes on the 4px grid, from the keyboard and the pointer.
    const resizer = page.getByRole("separator", { name: "Resize terminal panel" });
    assert.equal(await resizer.getAttribute("aria-valuenow"), "280");
    await resizer.focus();
    for (let step = 0; step < 4; step++) await page.keyboard.press("ArrowUp");
    assert.equal(await resizer.getAttribute("aria-valuenow"), "360");
    assert.equal(Math.round((await panel(page).boundingBox()).height), 360);
    const box = await resizer.boundingBox();
    await page.mouse.move(box.x + 200, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + 200, box.y - 61, { steps: 4 });
    await page.mouse.up();
    const dragged = Number(await resizer.getAttribute("aria-valuenow"));
    assert.equal(dragged % 4, 0, "dragged height on the grid");
    assert.ok(dragged > 400 && dragged <= 424, `dragged height ${dragged}`);
    await page.keyboard.press("End");
    const most = Number(await resizer.getAttribute("aria-valuenow"));
    assert.equal(most, Number(await resizer.getAttribute("aria-valuemax")));
    assert.ok(most <= 900 - 240, "the conversation keeps its room");
    await page.keyboard.press("Home");
    assert.equal(await resizer.getAttribute("aria-valuenow"), "120");
    for (let step = 0; step < 10; step++) await page.keyboard.press("ArrowUp");
    await audit(page, "resized", engine, scans);

    // Delete on a tab and the close button stop their shells; exit ends a shell visibly.
    await tabs(page).nth(2).focus();
    await page.keyboard.press("Delete");
    await page.waitForFunction(() => window.terminalPanelFixture.calls().includes("close"));
    assert.equal(await tabs(page).count(), 2);
    await tabs(page).nth(1).click();
    await page.getByRole("button", { name: "Close PowerShell 2", exact: true }).click();
    assert.equal(await tabs(page).count(), 1);
    await page.locator(".terminal-panel-viewport .xterm-helper-textarea").focus();
    await page.keyboard.type("exit");
    await page.keyboard.press("Enter");
    await page.getByText("PowerShell exited. Output is not saved.", { exact: true }).waitFor();
    assert.equal(await tabs(page).first().getAttribute("data-state"), "finished");
    await audit(page, "exited", engine, scans);

    // Escape on the panel's own controls hides it and never reaches the agent.
    await tabs(page).first().focus();
    await page.keyboard.press("Escape");
    await panel(page).waitFor({ state: "detached" });

    // The eighth started shell is the last; the ninth is refused before Rust is asked. The exited
    // shell keeps its tab but no longer counts. Each new tab shows, so its shell starts.
    await page.keyboard.press("Control+Backquote");
    await panel(page).waitFor();
    const reviews = async () => (await calls(page)).filter((call) => call === "review").length;
    const before = await reviews();
    const started = (await calls(page)).filter((call) => call.startsWith("start:")).length;
    for (let count = 1; count <= 8; count++) {
      await page.getByTestId("terminal-panel-new").click();
      await page.waitForFunction(
        (wanted) =>
          window.terminalPanelFixture.calls().filter((call) => call.startsWith("start:")).length ===
          wanted,
        started + count,
      );
    }
    await page.getByTestId("terminal-panel-new").click();
    await page
      .getByText("Eight terminals are open. Close one before opening another.", { exact: true })
      .waitFor();
    assert.equal(await reviews(), before + 8);
    assert.equal(await tabs(page).count(), 9);
    await audit(page, "limit", engine, scans);

    // A refused start shows Rust's own words, and Try again reviews again.
    const refusal =
      "This folder is being changed (a worktree removed, moved or restored, or changes reverted). Once that finishes, open the terminal again.";
    await page.goto(`${url}/?preview=terminal-panel&tabs=1&refuse=${encodeURIComponent(refusal)}`);
    await page.getByText(refusal, { exact: true }).waitFor();
    assert.equal(await tabs(page).first().getAttribute("data-state"), "failed");
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    await page.waitForFunction(
      () => window.terminalPanelFixture.calls().filter((call) => call === "review").length === 2,
    );
    await audit(page, "refused", engine, scans);

    // After a restart the panel offers back what the chat had open, by shell and name, and
    // starts nothing until Resume; Not now forgets it. Output from then is not kept.
    const remembered = JSON.stringify({
      tabs: [
        { kind: "shell", shell: "command_prompt", title: "Tests", named: true },
        { kind: "shell", shell: "power_shell", title: "PowerShell", named: false },
      ],
      open: true,
      savedAt: Date.now(),
    });
    const seed = (value) =>
      page.evaluate((value) => localStorage.setItem("auracode.terminals.preview", value), value);
    await page.goto(`${url}/?preview=settled`);
    await seed(remembered);
    await page.reload();
    const offer = page.getByTestId("terminal-resume");
    await offer
      .getByText(
        "Tests and PowerShell were open when Ghost last closed. Output from then was not kept.",
      )
      .waitFor();
    assert.deepEqual(await calls(page), [], "nothing started before Resume");
    await audit(page, "resume", engine, scans);
    await offer.getByRole("button", { name: "Resume", exact: true }).click();
    await offer.waitFor({ state: "detached" });
    await tabs(page).nth(1).waitFor();
    assert.deepEqual(await tabs(page).allTextContents(), ["Tests", "PowerShell"]);
    // The shown tab starts once its terminal has measured itself, a moment after the tabs show
    // (a slower machine, such as CI's Mac, reads the calls before it).
    await page.waitForFunction(() =>
      window.terminalPanelFixture.calls().some((call) => call.startsWith("start:")),
    );
    assert.deepEqual(
      (await calls(page))
        .filter((call) => call.startsWith("start:"))
        .map((call) => call.replace(/:\d+x\d+$/, "")),
      ["start:power_shell"],
    );
    // Each shell starts when its tab first shows, as any tab's does.
    await tabs(page).first().click();
    await page.waitForFunction(() =>
      window.terminalPanelFixture.calls().some((call) => call.startsWith("start:command_prompt")),
    );
    assert.deepEqual(
      JSON.parse(await page.evaluate(() => localStorage.getItem("auracode.terminals.preview")))
        .tabs,
      JSON.parse(remembered).tabs,
    );
    await page.goto(`${url}/?preview=settled`);
    await seed(remembered);
    await page.reload();
    await offer.getByRole("button", { name: "Not now", exact: true }).click();
    await offer.waitFor({ state: "detached" });
    assert.equal(await tabs(page).count(), 0);
    assert.equal(
      await page.evaluate(() => localStorage.getItem("auracode.terminals.preview")),
      null,
    );

    // Run, in a worktree chat whose folder setup saved a command: one tab of its own with the
    // worktree's address, and Stop in the same place while it runs.
    await page.goto(`${url}/?preview=terminal-panel&tabs=1&run=1`);
    const run = page.getByTestId("chat-run");
    await run.getByText("Run", { exact: true }).waitFor();
    assert.equal(
      await run.getAttribute("title"),
      "Run cmd /d /c pnpm dev in this worktree, on localhost:25040",
    );
    await run.click();
    await shows(page, "Local:   http://localhost:25040/");
    const runTab = tabs(page).filter({ hasText: "Run" });
    assert.equal(await runTab.getAttribute("aria-selected"), "true");
    const address = page.locator(".terminal-panel-url");
    assert.equal(await address.textContent(), "localhost:25040");
    assert.equal(await address.getAttribute("href"), "http://localhost:25040");
    assert.equal(await address.getAttribute("target"), "_blank");
    await run.getByText("Stop", { exact: true }).waitFor();
    await audit(page, "run", engine, scans);
    assert.equal(await tabs(page).count(), 2);
    assert.deepEqual(
      (await calls(page)).filter((call) => call.startsWith("run:")),
      ["run:reserve", "run:start"],
    );
    // Stop closes its tab and stops its process; Run is offered again.
    await run.getByText("Stop", { exact: true }).click();
    await run.getByText("Run", { exact: true }).waitFor();
    assert.equal(await tabs(page).count(), 1);
    await page.waitForFunction(() => window.terminalPanelFixture.calls().includes("run:close"));

    // The narrow window keeps the panel inside it.
    await page.setViewportSize({ width: 1024, height: 700 });
    await page.goto(`${url}/?preview=terminal-panel&tabs=3`);
    await tabs(page).nth(2).waitFor();
    await shows(page, "sidebar-scan>");
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
      true,
    );
    await audit(page, "narrow", engine, scans);

    assert.deepEqual(errors, [], "browser errors");
    console.log(
      `${engine}: ${scans.length} accessibility scans (${scans.join(", ")}); toggle, focus, menu, palette, tabs, rename, resize, close, exit, limit, refusal, resume and Run passed`,
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
      [vite, "--host", "127.0.0.1", "--port", port, "--strictPort"],
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
