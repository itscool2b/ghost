/**
 * Browser fixture evidence for the larger dictation model (docs/local-dictation.md, "The larger
 * model"): Settings > Voice > Larger model. No native window and no network: the settings fixture
 * stands in for the download and records what was asked of it.
 *
 * The row says what the model is, what it costs and where it comes from before anything is
 * fetched. Download shows what has arrived and can be cancelled; what arrived is kept and Continue
 * goes on from it. Once it is here dictation names it as the model in use, and Remove returns to
 * the built-in one. A failure says why. A copy with nowhere to keep a model offers nothing.
 * Usage: node scripts/check-dictation-larger.mjs [--server URL] [--engines chromium,webkit]
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
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
// The fixture suite hands its own server over in CHECK_URL; alone, this check starts one.
const given = values.server ?? process.env.CHECK_URL;
const url = given ?? "http://127.0.0.1:1467";
const require = createRequire(import.meta.url);
let server;
let serverOutput = "";

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
      const what = (text) => `${engine} ${theme}: ${text}`;
      const row = page.locator('[data-setting="Larger model"]');
      const control = page.getByTestId("larger-model");
      const dictation = page.locator('[data-setting="Dictation"]');
      const log = () => page.evaluate(() => window.largerFixture.log());

      // Before anything is fetched: what it is, what it costs, where it comes from.
      await page.goto(`${url}/?preview=settings&page=voice`);
      await row.waitFor();
      await dictation
        .getByText("Parakeet TDT-CTC 110M, built in and ready", { exact: false })
        .waitFor();
      const offer = await row.textContent();
      assert.match(offer, /Parakeet Ultra makes about 40% fewer mistakes/, what("what it is"));
      assert.match(offer, /A one-time 486 MB download from Hugging Face/, what("what it costs"));
      assert.deepEqual(await log(), [], what("nothing is fetched until it is asked for"));
      assert.deepEqual(
        (await new AxeBuilder({ page }).include('[data-setting="Larger model"]').analyze())
          .violations,
        [],
        what("the offer"),
      );

      // Download: what has arrived, and a way to stop.
      await control.getByRole("button", { name: "Download" }).click();
      await row.getByText("Downloading Parakeet Ultra: 365 of 486 MB.").waitFor();
      assert.equal(await control.getByRole("button", { name: "Download" }).count(), 0);
      assert.deepEqual(
        (await new AxeBuilder({ page }).include('[data-setting="Larger model"]').analyze())
          .violations,
        [],
        what("while it downloads"),
      );
      // Cancel keeps what arrived, and Continue goes on from it.
      await control.getByRole("button", { name: "Cancel" }).click();
      await row.getByText("What arrived so far is kept.", { exact: false }).waitFor();
      await control.getByRole("button", { name: "Continue" }).click();
      await row.getByText("Downloading Parakeet Ultra: 365 of 486 MB.").waitFor();
      assert.deepEqual(
        await log(),
        ["download from 0", "cancel", "download from 0.75"],
        what("the second download continued from what was kept"),
      );
      // It arrives and is checked: dictation names it, and the row offers to remove it.
      await page.evaluate(() => window.largerFixture.finish());
      await dictation
        .getByText(
          "Parakeet Ultra, downloaded and ready: English and 24 other European languages",
          {
            exact: false,
          },
        )
        .waitFor();
      await row.getByText("Parakeet Ultra is downloaded and in use.", { exact: false }).waitFor();
      assert.equal(await control.getByRole("button", { name: "Remove" }).count(), 1);
      assert.equal(
        await control.getByRole("button", { name: /Download|Continue|Cancel/ }).count(),
        0,
      );
      assert.ok(
        (await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)) <= 0,
        what("no sideways overflow"),
      );
      // Remove: the built-in model again, and the offer.
      await control.getByRole("button", { name: "Remove" }).click();
      await dictation
        .getByText("Parakeet TDT-CTC 110M, built in and ready", { exact: false })
        .waitFor();
      await control.getByRole("button", { name: "Download" }).waitFor();
      assert.equal((await log()).at(-1), "remove");

      // A failure says why, and the download can be asked for again.
      await control.getByRole("button", { name: "Download" }).click();
      await row.getByText("Downloading Parakeet Ultra", { exact: false }).waitFor();
      await page.evaluate(() =>
        window.largerFixture.fail("The download is not the file o7 expects, so it was discarded."),
      );
      const alert = page.getByTestId("larger-model-error");
      await alert.waitFor();
      assert.equal(
        await alert.textContent(),
        "The download is not the file o7 expects, so it was discarded.",
      );
      await dictation
        .getByText("Parakeet TDT-CTC 110M, built in and ready", { exact: false })
        .waitFor();
      await control.getByRole("button", { name: "Download" }).waitFor();

      // Already here when the page opens.
      await page.goto(`${url}/?preview=settings&page=voice&larger=ready`);
      await row.getByText("Parakeet Ultra is downloaded and in use.", { exact: false }).waitFor();
      await control.getByRole("button", { name: "Remove" }).waitFor();
      // A copy with nowhere to keep a model offers nothing.
      await page.goto(`${url}/?preview=settings&page=voice&larger=none`);
      await dictation
        .getByText("Parakeet TDT-CTC 110M, built in and ready", { exact: false })
        .waitFor();
      assert.equal(await row.count(), 0, what("no row without somewhere to keep it"));
      // Settings' search finds it.
      await page.goto(`${url}/?preview=settings`);
      await page.getByRole("searchbox", { name: "Search settings" }).fill("parakeet ultra");
      await page
        .getByRole("navigation", { name: "Matching settings" })
        .getByRole("button", { name: "Larger model", exact: false })
        .first()
        .waitFor();

      assert.deepEqual(errors, [], what("no page errors"));
      await context.close();
    }
    console.log(
      `${engine}: the larger dictation model is offered, downloaded, cancelled and continued, used and removed`,
    );
  } finally {
    await browser.close();
  }
}

try {
  if (!given) {
    const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
    server = spawn(
      process.execPath,
      [vite, "--host", "127.0.0.1", "--port", "1467", "--strictPort"],
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
