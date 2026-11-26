/**
 * The app's own update on the real chat screen, with a synthetic updater
 * (`?preview=agent-updates&app-update=<case>`, src/dev/appUpdateFixture.ts): nothing shows while
 * the app is current; a newer version shows Update at the sidebar's foot; one button downloads,
 * installs and ends on the restart; a failed signature and a wait for work each say so and pass on
 * the next press; the pill folds to its arrow in the rail; and the panel is accessible in both
 * appearances.
 *
 * Usage: node scripts/check-app-update.mjs [--engines chromium,webkit]
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

const { values } = parseArgs({
  options: { engines: { type: "string", default: "chromium,webkit" } },
});

// Panels rise in and controls settle their colours; audit only once every finite animation ends.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const origin = "http://127.0.0.1:1481";
const out = resolve(app, "../../target/app-update-browser");
const server = spawn(
  process.execPath,
  [
    resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js"),
    "--host",
    "127.0.0.1",
    "--port",
    "1481",
    "--strictPort",
  ],
  { cwd: app, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
);
let output = "";
for (const stream of [server.stdout, server.stderr])
  stream.on("data", (part) => {
    output = (output + part).slice(-8192);
  });
const calls = (page) => page.evaluate(() => document.body.dataset.appUpdateCalls ?? "");
const axe = async (page, what) =>
  assert.deepEqual(
    (
      await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()
    ).violations.map((v) => ({
      id: v.id,
      nodes: v.nodes.map((n) => n.target),
    })),
    [],
    what,
  );
try {
  await mkdir(out, { recursive: true });
  let ready = false;
  for (let i = 0; i < 150; i++) {
    if (server.exitCode !== null) throw new Error(output);
    try {
      ready = (await fetch(origin)).ok;
    } catch {}
    if (ready) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.ok(ready, output);
  const launchers = { chromium, webkit };
  for (const engine of values.engines.split(",")) {
    const launcher = launchers[engine];
    assert.ok(launcher, `Unsupported engine: ${engine}`);
    const browser = await launcher.launch({ headless: true });
    try {
      for (const scheme of ["dark", "light"]) {
        const context = await browser.newContext({
          viewport: { width: 1200, height: 760 },
          colorScheme: scheme,
        });
        const page = await context.newPage();
        const errors = [];
        page.on("pageerror", (error) => errors.push(error.message));
        await page.route("**/*", (route) =>
          new URL(route.request().url()).hostname === "127.0.0.1"
            ? route.continue()
            : route.abort(),
        );
        const open = (kind) =>
          page.goto(`${origin}/?preview=agent-updates&theme=${scheme}&app-update=${kind}`);
        const pill = page.getByTestId("app-update");
        const start = page.getByTestId("app-update-start");
        const panel = page.getByRole("region", { name: "Update Ghost" });

        // Up to date: the footer is as it always was.
        await open("current");
        await page.getByTestId("notifications-bell").waitFor();
        await page.waitForTimeout(300);
        assert.equal(await pill.count(), 0, "no Update while the app is current");

        // A newer version: the pill, then one button for the whole update.
        await open("available");
        await pill.waitFor();
        assert.equal((await pill.textContent())?.trim(), "Update");
        // The word sits inside the pill with room each side, clear of the bell: a token that did
        // not exist once dropped the pill's padding, and the word ran to its edge.
        const room = await page.evaluate(() => {
          const edges = (selector) => document.querySelector(selector).getBoundingClientRect();
          const [around, arrow, word, bell] = [
            edges(".app-update-pill"),
            edges(".app-update-pill > svg"),
            edges(".app-update-word"),
            edges('[data-testid="notifications-bell"]'),
          ];
          return {
            left: arrow.left - around.left,
            right: around.right - word.right,
            clear: bell.left - around.right,
          };
        });
        assert.ok(room.left >= 8 && room.right >= 8, `the pill's padding: ${JSON.stringify(room)}`);
        assert.ok(room.clear >= 0, `the pill runs under the bell: ${JSON.stringify(room)}`);
        assert.equal(await calls(page), "", "showing the pill asks for nothing");
        await pill.click();
        await panel.getByRole("heading", { name: "Ghost 0.1.1571" }).waitFor();
        assert.match(await panel.textContent(), /You have 0\.1\.1570/);
        assert.match(await panel.textContent(), /The Update button\./);
        // One Cursor chat is working in this fixture: the panel says what the restart does to it.
        assert.match(await panel.textContent(), /1 chat is working\. The restart stops it/);
        await settled(page);
        await axe(page, `${engine} ${scheme}: the update panel`);
        await page.screenshot({ path: resolve(out, `${engine}-${scheme}-available.png`) });
        await start.click();
        await panel.getByRole("progressbar", { name: "Update download" }).waitFor();
        assert.equal(await start.isDisabled(), true, "no second start while it downloads");
        await panel.getByText("Preview: nothing was installed.").waitFor();
        assert.equal(await calls(page), "download,install");
        assert.equal((await pill.textContent())?.trim(), "Restart");
        assert.equal(await start.count(), 0, "nothing left to press once it is installed");

        // A signature that does not verify: it says so, installs nothing, and the next press passes.
        await open("fail");
        await pill.click();
        await start.click();
        await panel.getByRole("alert").waitFor();
        assert.match(await panel.getByRole("alert").textContent(), /signature .* is invalid/);
        assert.equal(await calls(page), "download", "a failed download is never installed");
        assert.equal((await start.textContent())?.trim(), "Try again");
        await settled(page);
        await axe(page, `${engine} ${scheme}: a failed update`);
        await start.click();
        await panel.getByText("Preview: nothing was installed.").waitFor();
        assert.equal(await calls(page), "download,check,download,install");

        // Work still finishing: it waits and says why; pressing again installs.
        await open("busy");
        await pill.click();
        await start.click();
        await panel.getByText(/Wait for Git changes/).waitFor();
        assert.equal((await start.textContent())?.trim(), "Try again");
        await start.click();
        await panel.getByText("Preview: nothing was installed.").waitFor();
        assert.equal(await calls(page), "download,install,install");

        // The rail: the arrow alone, still named for a screen reader.
        await open("available");
        await pill.waitFor();
        await page.getByRole("button", { name: /Collapse sidebar|Hide sidebar/ }).click();
        await page.waitForTimeout(300);
        assert.equal(await pill.locator(".app-update-word").isVisible(), false);
        assert.ok((await pill.boundingBox()).width <= 30, "the pill folds to its arrow");
        assert.match(
          await page.locator(".app-update > summary").getAttribute("aria-label"),
          /Update: Ghost 0\.1\.1571 is available/,
        );
        await settled(page);
        await axe(page, `${engine} ${scheme}: the rail`);
        assert.deepEqual(errors, []);
        await context.close();
      }
      console.log(
        `${engine}: nothing while current, the Update pill, one-button update, a failed signature, a wait for work, the rail and accessibility in both appearances passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  server.kill();
}
