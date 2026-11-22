/**
 * Browser fixture evidence for the sidebar while the app opens (`?preview=opening`). No native
 * window and no database: the lists are invented and answer when the fixture says.
 *
 * The window's first frame shows the projects and chats the last launch kept, at rest, and the
 * database's own lists take their place without the rows that stay moving. With nothing kept the
 * sidebar draws placeholders, but never for a list that answers at once; a wait long enough to
 * notice is named in one quiet line, never as an error; a real fault stays an error with Retry.
 * Usage: node scripts/check-opening.mjs [--server URL] [--engines chromium,webkit]
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
  options: { server: { type: "string" }, engines: { type: "string", default: "chromium,webkit" } },
});
const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = resolve(app, "../../target/opening-browser");
// The fixture suite hands its own server over in CHECK_URL; alone, this check starts one.
const given = values.server ?? process.env.CHECK_URL;
const url = given ?? "http://127.0.0.1:1463";
const require = createRequire(import.meta.url);
let server;
let serverOutput = "";
await mkdir(out, { recursive: true });

async function ready() {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // Not up yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`The preview server did not start.\n${serverOutput}`);
}

async function inspect(engine, launcher) {
  const browser = await launcher.launch();
  try {
    for (const theme of ["light", "dark"]) {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 900 },
        colorScheme: theme,
      });
      const page = await context.newPage();
      page.setDefaultTimeout(12000);
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const skeleton = page.getByTestId("chats-skeleton");
      const note = page.getByTestId("chats-opening");
      const fault = page.getByTestId("chats-error");
      const row = (title) => page.getByRole("button", { name: title, exact: false });
      const audit = async (what) =>
        assert.deepEqual(
          // The sidebar is what this fixture shows; the pane beside it is only the start picture.
          (await new AxeBuilder({ page }).include(".chat-sidebar").analyze()).violations,
          [],
          `${engine} ${theme} ${what}`,
        );

      // Nothing kept: placeholders after a beat, then the lists, and never a word of alarm.
      await page.goto(`${url}/?preview=opening&late=1500`);
      await page.getByRole("heading", { name: "Projects" }).waitFor();
      assert.equal(await skeleton.count(), 0, "no placeholder on the first frame");
      assert.equal(await page.getByText("will appear here").count(), 0, "not empty: not read yet");
      assert.equal(
        await page.locator('.chat-project-section[aria-busy="true"]').count(),
        1,
        "the section says it is busy",
      );
      await skeleton.waitFor();
      assert.equal(await skeleton.locator("li").count(), 6);
      assert.equal(await skeleton.getAttribute("aria-hidden"), "true");
      // Bars of the rows' own height, in ink, with no blur and no glow.
      const bar = await skeleton.locator("li").first().evaluate((item) => {
        const style = getComputedStyle(item, "::before");
        return {
          row: item.getBoundingClientRect().height,
          filter: style.filter,
          backdrop: style.backdropFilter ?? style.webkitBackdropFilter ?? "none",
          shadow: style.boxShadow,
          animation: style.animationName,
        };
      });
      assert.equal(bar.row, 32);
      assert.equal(bar.filter, "none");
      assert.equal(bar.backdrop, "none");
      assert.equal(bar.shadow, "none");
      assert.equal(bar.animation, "shimmer");
      assert.equal(await fault.count(), 0);
      assert.equal(await note.count(), 0, "a short wait is not mentioned");
      await audit("placeholders");
      await page.screenshot({ path: resolve(out, `${engine}-placeholders-${theme}.png`) });
      await row("Tidy the sidebar").waitFor();
      assert.equal(await skeleton.count(), 0, "the lists take the placeholders' place");
      assert.equal(await page.locator(".chat-projects[data-settled]").count(), 1);
      assert.equal(await page.locator(".chat-project-section[aria-busy]").count(), 0);
      assert.equal(await note.count(), 0);

      // A wait long enough to notice is named, quietly, and the name goes with the wait.
      await page.goto(`${url}/?preview=opening&late=3200`);
      await note.waitFor();
      assert.equal(await note.textContent(), "Finishing the last session");
      assert.equal(await note.getAttribute("role"), "status");
      assert.equal(await fault.count(), 0, "starting is not a fault");
      await page.screenshot({ path: resolve(out, `${engine}-late-${theme}.png`) });
      await row("Tidy the sidebar").waitFor();
      assert.equal(await note.count(), 0);

      // Lists kept from the last launch: on the first frame, at rest, with no placeholder at all.
      await page.goto(`${url}/?preview=opening&kept&late=1500`);
      await row("Fix the login flow").waitFor();
      assert.equal(await row("A chat deleted since").count(), 1);
      assert.equal(await row("Tidy the sidebar").count(), 0);
      // The chat that was working when the list was kept is not working now, and waits on no one.
      assert.equal(await page.getByRole("region", { name: "Needs you" }).count(), 0);
      assert.equal(await page.locator(".chat-project-section[aria-busy]").count(), 0);
      await audit("kept lists");
      await page.screenshot({ path: resolve(out, `${engine}-kept-${theme}.png`) });
      // A kept row can be opened before the database answers.
      await row("Ship the release notes").click();
      assert.equal(await page.evaluate(() => document.body.dataset.openedChat), "chat-notes");
      const kept = await row("Fix the login flow").boundingBox();
      await page.waitForTimeout(400);
      assert.equal(await skeleton.count(), 0, "kept lists never show placeholders");
      // The database's answer: the deleted chat leaves, the new one rises in, the rest stay put.
      await row("Tidy the sidebar").waitFor();
      assert.equal(await row("A chat deleted since").count(), 0);
      const arrived = await page.locator(".chat-row-container[data-arrived]").allTextContents();
      assert.equal(arrived.length, 1, "only the new chat arrives");
      assert.match(arrived[0], /Tidy the sidebar/);
      assert.equal(await page.locator(".chat-projects[data-settled]").count(), 0);
      const after = await row("Fix the login flow").boundingBox();
      assert.deepEqual(after, kept, "a row that stays does not move");
      assert.equal(await note.count(), 0);

      // A real fault is an error with the way to read again; reading again clears it.
      await page.goto(`${url}/?preview=opening&fail&late=150`);
      await fault.waitFor();
      assert.match(await fault.textContent(), /The disk is full\./);
      assert.equal(await skeleton.count(), 0);
      assert.equal(await note.count(), 0);
      await audit("fault");
      await page.screenshot({ path: resolve(out, `${engine}-fault-${theme}.png`) });
      await fault.getByRole("button", { name: "Retry", exact: true }).click();
      await row("Tidy the sidebar").waitFor();
      assert.equal(await fault.count(), 0);

      assert.deepEqual(errors, [], `${engine} ${theme} page errors`);
      await context.close();
    }

    // Reduced motion: the placeholders are still, and the lists take their place at once.
    const still = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      reducedMotion: "reduce",
    });
    const page = await still.newPage();
    page.setDefaultTimeout(12000);
    await page.goto(`${url}/?preview=opening&late=1500`);
    const skeleton = page.getByTestId("chats-skeleton");
    await skeleton.waitFor();
    const motion = await skeleton
      .locator("li")
      .first()
      .evaluate((item) => getComputedStyle(item, "::before").animationDuration);
    assert.equal(motion, "0s", "no sheen with reduced motion");
    await page.getByRole("button", { name: "Tidy the sidebar", exact: false }).waitFor();
    const settle = await page
      .locator(".chat-projects")
      .evaluate((list) => getComputedStyle(list).animationDuration);
    assert.equal(settle, "0s", "no fade with reduced motion");
    await still.close();
    console.log(`${engine}: the sidebar opens from kept lists, placeholders, a late answer and a fault`);
  } finally {
    await browser.close();
  }
}

try {
  if (!given) {
    const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
    server = spawn(
      process.execPath,
      [vite, "--host", "127.0.0.1", "--port", "1463", "--strictPort"],
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
