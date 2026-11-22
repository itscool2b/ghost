/**
 * Browser fixture evidence for reading aloud (docs/read-aloud.md). No native window, no voice
 * model and no sound: `dev/speechFixture.ts` stands in for the engine and records what it was
 * asked to read.
 *
 * Under an answer the speaker reads it: the answer's words without its marks, in the saved
 * voice, with the same button stopping it; another answer's button takes over; a failure says
 * why beside the button. A build without the voice model, or a processor it cannot run on, shows
 * no button at all. Settings > Voice names the model, offers the voices in their groups, plays a
 * sample of a choice and saves it.
 * Usage: node scripts/check-read-aloud.mjs [--server URL] [--engines chromium,webkit]
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
const url = given ?? "http://127.0.0.1:1466";
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

const fixture = (page) =>
  page.evaluate(() => ({
    said: window.speechFixture.said.map((entry) => ({ ...entry })),
    stopped: [...window.speechFixture.stopped],
    prepared: [...window.speechFixture.prepared],
  }));

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

      // Under an answer: one speaker per finished answer, after Copy.
      await page.goto(`${url}/?preview=settled`);
      const buttons = page.getByTestId("read-aloud");
      await buttons.first().waitFor();
      const footers = page.getByTestId("turn-footer");
      const count = await buttons.count();
      assert.ok(count >= 1, what("an answer has a speaker"));
      assert.equal(
        await footers.evaluateAll(
          (nodes) =>
            nodes.filter(
              (node) =>
                node.querySelector('[aria-label="Copy response as Markdown"]') !== null &&
                node.querySelector('[data-testid="read-aloud"]') === null,
            ).length,
        ),
        0,
        what("every answer that can be copied can be read"),
      );
      const last = buttons.last();
      assert.equal(await last.getAttribute("aria-label"), "Read aloud");
      assert.equal(await last.getAttribute("aria-pressed"), "false");
      const size = await last.evaluate((node) => {
        const box = node.getBoundingClientRect();
        const copy = node.parentElement
          .querySelector('[aria-label="Copy response as Markdown"]')
          .getBoundingClientRect();
        return {
          width: box.width,
          height: box.height,
          after: box.left >= copy.right,
          top: box.top === copy.top,
        };
      });
      assert.deepEqual(
        size,
        { width: 28, height: 28, after: true, top: true },
        what("a quiet 28px action beside Copy"),
      );

      // Reaching it readies the voice; pressing it reads the answer's words, not its marks.
      await last.hover();
      await page.waitForFunction(() => window.speechFixture.prepared.length === 1);
      await last.click();
      await page.waitForFunction(() => window.speechFixture.said.length === 1);
      await page.locator('[data-testid="read-aloud"][data-state="speaking"]').waitFor();
      assert.equal(await last.getAttribute("aria-label"), "Stop reading");
      assert.equal(await last.getAttribute("aria-pressed"), "true");
      let record = await fixture(page);
      assert.equal(record.said[0].voice, null, what("the model's own voice until one is chosen"));
      assert.ok(record.said[0].text.length > 0);
      assert.doesNotMatch(
        record.said[0].text,
        /[`#]|\*\*|\]\(/,
        what("no Markdown marks are read"),
      );
      assert.equal(
        await page.locator('[data-testid="read-aloud"]:not([data-state="idle"])').count(),
        1,
        what("only the answer being read shows it"),
      );
      assert.deepEqual(
        (await new AxeBuilder({ page }).include('[data-testid="turn-footer"]').analyze())
          .violations,
        [],
        what("the row under an answer, speaking"),
      );

      // The same button stops it.
      await last.click();
      await page.waitForFunction(() => window.speechFixture.stopped.length === 1);
      assert.equal(await last.getAttribute("data-state"), "idle");
      record = await fixture(page);
      assert.equal(
        record.stopped[0],
        record.said[0].id,
        what("the request that was reading is the one stopped"),
      );

      // Reaching the end frees the button by itself.
      await last.click();
      await page.locator('[data-testid="read-aloud"][data-state="speaking"]').waitFor();
      await page.evaluate(() => window.speechFixture.finish());
      await page
        .locator('[data-testid="read-aloud"][data-state="speaking"]')
        .waitFor({ state: "detached" });
      assert.equal(await last.getAttribute("aria-label"), "Read aloud");

      // The keyboard: Enter starts, Enter stops.
      await last.focus();
      await page.keyboard.press("Enter");
      await page.locator('[data-testid="read-aloud"][data-state="speaking"]').waitFor();
      await page.keyboard.press("Enter");
      await page.waitForFunction(
        () =>
          document.querySelector('[data-testid="read-aloud"]:not([data-state="idle"])') === null,
      );

      // Another answer's button takes over: one voice in the window.
      if (count > 1) {
        await last.click();
        await page.locator('[data-testid="read-aloud"][data-state="speaking"]').waitFor();
        const before = (await fixture(page)).said.length;
        await buttons.first().click();
        await page.waitForFunction((n) => window.speechFixture.said.length === n + 1, before);
        await page.waitForFunction(
          () =>
            document.querySelectorAll('[data-testid="read-aloud"]:not([data-state="idle"])')
              .length === 1,
        );
        assert.equal(
          await last.getAttribute("data-state"),
          "idle",
          what("the first answer's button is free"),
        );
        assert.notEqual(await buttons.first().getAttribute("data-state"), "idle");
        await buttons.first().click();
      }

      // A failure says why, beside the button that asked.
      await last.click();
      await page.locator('[data-testid="read-aloud"][data-state="speaking"]').waitFor();
      await page.evaluate(() => window.speechFixture.fail("o7 found no speakers or headphones."));
      const alert = page.getByTestId("read-aloud-error");
      await alert.waitFor();
      assert.equal(await alert.textContent(), "o7 found no speakers or headphones.");
      assert.equal(await last.getAttribute("data-state"), "idle");
      assert.ok(
        (await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)) <= 0,
        what("no sideways overflow"),
      );

      // Opening another chat ends the reading: no voice is left without its stop button. The
      // real chats screen, over the tour's three chats.
      await page.goto(`${url}/?preview=tour&open`);
      await page.getByTestId("read-aloud").first().click();
      await page.locator('[data-testid="read-aloud"][data-state="speaking"]').waitFor();
      const ended = (await fixture(page)).stopped.length;
      const reading = (await fixture(page)).said.at(-1).id;
      await page.getByTestId("chat-row").filter({ hasText: "Add search to the catalogue" }).click();
      await page.waitForFunction((n) => window.speechFixture.stopped.length > n, ended);
      assert.equal(
        (await fixture(page)).stopped.at(-1),
        reading,
        what("leaving the chat stops what it was reading"),
      );

      // Where reading aloud cannot run there is no button, and nothing else changes.
      for (const state of ["missing", "unsupported"]) {
        await page.goto(`${url}/?preview=settled&speech=${state}`);
        await page.getByTestId("turn-footer").first().waitFor();
        await page.getByRole("button", { name: "Copy response as Markdown" }).first().waitFor();
        await page.waitForTimeout(150);
        assert.equal(
          await page.getByTestId("read-aloud").count(),
          0,
          what(`no speaker when ${state}`),
        );
      }

      // Settings > Voice: the model, the voices in their groups, a sample, and a saved choice.
      await page.goto(`${url}/?preview=settings&page=voice`);
      const group = page.getByTestId("read-aloud-settings");
      await group.getByText("Phonon 31M, built in and ready", { exact: false }).waitFor();
      const voices = group.getByRole("combobox", { name: "Voice" });
      // The app's own dropdown: the groups as headings in its list, each voice a row.
      await voices.click();
      const list = page.getByRole("listbox");
      await list.waitFor();
      assert.deepEqual(
        await list.evaluate((node) =>
          [...node.querySelectorAll('.ui-select-group, [role="option"]')].map((entry) =>
            entry.matches(".ui-select-group") ? `# ${entry.textContent}` : entry.textContent,
          ),
        ),
        ["# American", "Reuben", "Zoey", "# British", "Alistair", "# Characters", "The Captain"],
        what("the voices in their groups"),
      );
      assert.equal(
        await list.getByRole("option", { selected: true }).textContent(),
        "Reuben",
        what("the model's own voice at first"),
      );
      await page.keyboard.press("Escape");
      const sample = group.getByTestId("voice-sample");
      assert.equal((await sample.textContent()).trim(), "Hear it");
      await sample.click();
      await page.waitForFunction(() => window.speechFixture.said.length === 1);
      await group.getByRole("button", { name: "Stop" }).waitFor();
      await sample.click();
      await group.getByRole("button", { name: "Hear it" }).waitFor();
      // A choice is heard in the voice just picked, and its description takes the row.
      await voices.click();
      await page.getByRole("option", { name: "Alistair" }).click();
      await page.waitForFunction(() => window.speechFixture.said.length === 2);
      record = await fixture(page);
      assert.equal(
        record.said[1].voice,
        "Qm4wT-u1_cXb7RzA",
        what("the sample is in the chosen voice"),
      );
      assert.equal(record.said[1].text, record.said[0].text, what("the same sample sentence"));
      await group.getByText("Measured and warm, with a British accent.").waitFor();
      await page.waitForFunction(
        () => window.settingsFixture?.general?.().speech_voice === "Qm4wT-u1_cXb7RzA",
      );
      await page.evaluate(() => window.speechFixture.finish());
      await group.getByRole("button", { name: "Hear it" }).waitFor();
      assert.deepEqual(
        (await new AxeBuilder({ page }).include('[data-testid="read-aloud-settings"]').analyze())
          .violations,
        [],
        what("Settings > Voice > Read aloud"),
      );
      assert.ok(
        (await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)) <= 0,
        what("Settings: no sideways overflow"),
      );

      // Without the model the page says so and offers no voice.
      await page.goto(`${url}/?preview=settings&page=voice&speech=missing`);
      await page
        .getByTestId("read-aloud-settings")
        .getByText("This build of o7 is missing its voice model.", { exact: false })
        .waitFor();
      assert.equal(await page.getByRole("combobox", { name: "Voice" }).count(), 0);
      await page.goto(`${url}/?preview=settings&page=voice&speech=unsupported`);
      await page
        .getByTestId("read-aloud-settings")
        .getByText("Reading aloud needs a processor with AVX2", { exact: false })
        .waitFor();
      assert.equal(await page.getByRole("combobox", { name: "Voice" }).count(), 0);

      assert.deepEqual(errors, [], what("no page errors"));
      await context.close();
    }

    // Reduced motion: getting ready does not pulse.
    const still = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      reducedMotion: "reduce",
    });
    const calm = await still.newPage();
    calm.setDefaultTimeout(12000);
    await calm.goto(`${url}/?preview=settled`);
    const button = calm.getByTestId("read-aloud").last();
    await button.waitFor();
    // Held at "getting ready": the stand-in never reports speaking.
    await calm.evaluate(() => {
      const speak = window.speechFixture.api.speak;
      window.speechFixture.api.speak = (id, text, voice, emit) =>
        speak(id, text, voice, (event) => {
          if (event.state !== "speaking") emit(event);
        });
    });
    await button.click();
    await calm.locator('[data-testid="read-aloud"][data-state="preparing"]').waitFor();
    assert.equal(
      await button.locator("svg").evaluate((node) => getComputedStyle(node).animationName),
      "none",
      `${engine}: no pulse with reduced motion`,
    );
    await still.close();
    console.log(
      `${engine}: an answer is read and stopped, one voice at a time, and Settings chooses the voice`,
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
      [vite, "--host", "127.0.0.1", "--port", "1466", "--strictPort"],
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
