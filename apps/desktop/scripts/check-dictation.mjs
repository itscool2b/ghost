/**
 * Scripted dictation UI checks (src/dev/dictationPreview.tsx). No audio hardware or native
 * commands: the fixture plays a recording a word at a time. Covers the bar, the live ghost text,
 * finishing and cancelling, failures inside the bar, chat switching, the ghost's alignment with
 * the message box, and the finishing and cancelling animations with full motion.
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

/** Pixels that differ between two PNG screenshots, counted by the page's own canvas. */
async function differingPixels(page, a, b) {
  return page.evaluate(
    async ([a, b]) => {
      const load = (src) =>
        new Promise((resolve) => {
          const image = new Image();
          image.onload = () => resolve(image);
          image.src = `data:image/png;base64,${src}`;
        });
      const [x, y] = await Promise.all([load(a), load(b)]);
      const canvas = document.createElement("canvas");
      canvas.width = x.width;
      canvas.height = x.height;
      const context = canvas.getContext("2d");
      context.drawImage(x, 0, 0);
      const p = context.getImageData(0, 0, x.width, x.height).data;
      context.clearRect(0, 0, x.width, x.height);
      context.drawImage(y, 0, 0);
      const q = context.getImageData(0, 0, x.width, x.height).data;
      let count = 0;
      for (let i = 0; i < p.length; i += 4)
        if (p[i] !== q[i] || p[i + 1] !== q[i + 1] || p[i + 2] !== q[i + 2]) count++;
      return count;
    },
    [a.toString("base64"), b.toString("base64")],
  );
}

export async function checkDictation(browser, url, out, engine) {
  for (const theme of ["light", "dark"]) {
    const context = await browser.newContext({
      viewport: { width: 1024, height: 700 },
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    try {
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const input = page.getByTestId("composer-input");
      const mic = page.getByRole("button", { name: "Dictate a message", exact: true });
      const bar = page.getByTestId("dictation-bar");
      const ghost = page.locator(".dictation-ghost");
      const finish = page.getByRole("button", { name: "Finish dictation", exact: true });
      // The fixture's controls act without taking focus, as the engine does.
      const fixture = (name) => ({
        click: () =>
          page.getByRole("button", { name, exact: true }).evaluate((node) => node.click()),
      });
      const open = async (scenario = "", draft = "") => {
        await page.goto(
          `${url}/?preview=dictation&theme=${theme}&case=${scenario}&draft=${encodeURIComponent(draft)}`,
        );
        await input.waitFor();
        await page.waitForFunction(
          () => !document.querySelector('[data-testid="composer-input"]').disabled,
        );
      };
      const audit = async (name) => {
        assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
        const box = await bar.boundingBox();
        assert.ok(
          box && box.x >= 0 && box.y >= 0 && box.x + box.width <= 1024 && box.y + box.height <= 700,
        );
        await page.screenshot({ path: resolve(out, `${engine}-${theme}-${name}.png`) });
      };
      const focusedName = () =>
        page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? "");

      // Listening: the bar replaces the controls, Finish takes focus, and the draft is the ghost's.
      await open();
      await mic.click();
      await page.getByRole("meter", { name: "Microphone level" }).waitFor();
      assert.equal(await page.locator('summary[aria-label="Add to message"]').isVisible(), false);
      await page.waitForFunction(
        () => document.activeElement?.getAttribute("aria-label") === "Finish dictation",
      );
      assert.match(await ghost.textContent(), /Listening/);
      await fixture("Hear words").click();
      await page.waitForFunction(() =>
        document.querySelector(".dictation-ghost")?.textContent.includes("Add a regression"),
      );
      assert.equal(await input.inputValue(), "");
      assert.equal(await input.evaluate((field) => field.readOnly), true);
      await fixture("Pause").click();
      await fixture("Hear words").click();
      assert.equal(await page.locator('.dictation-word[data-kind="settled"]').count(), 3);
      assert.ok((await page.locator('.dictation-word[data-kind="tentative"]').count()) > 0);
      await audit("listening");
      // Enter in the read-only draft finishes rather than sending.
      await input.focus();
      await input.press("Enter");
      await page.getByRole("status").filter({ hasText: "Finishing" }).waitFor();
      assert.equal(await page.getByLabel("Messages sent").textContent(), "0");
      await audit("finishing");
      await fixture("Complete transcription").click();
      await bar.waitFor({ state: "detached" });
      assert.equal(await input.inputValue(), "Add a regression test for the.");
      assert.equal(await input.evaluate((field) => field.readOnly), false);
      assert.equal(await ghost.count(), 0);
      await page.waitForFunction(
        () => document.activeElement === document.querySelector('[data-testid="composer-input"]'),
      );
      assert.equal(await input.evaluate((field) => field.selectionStart), 30);
      assert.equal(await page.getByLabel("Messages sent").textContent(), "0");

      // Words go where the caret was, spaced from the draft around them.
      await input.evaluate((field) => field.setSelectionRange(5, 5));
      await mic.click();
      await fixture("Hear words").click();
      assert.match(await ghost.textContent(), /^Add a Add a regression regression test for the\.$/);
      await finish.click();
      await fixture("Complete transcription").click();
      await bar.waitFor({ state: "detached" });
      assert.equal(await input.inputValue(), "Add a Add a regression. regression test for the.");

      // Esc and Cancel discard the words, and a late result changes nothing.
      for (const how of ["Escape", "Cancel"]) {
        await mic.click();
        await fixture("Hear words").click();
        if (how === "Escape") await page.keyboard.press("Escape");
        else await page.getByRole("button", { name: "Cancel dictation", exact: true }).click();
        await bar.waitFor({ state: "detached" });
        await fixture("Complete transcription").click();
        assert.equal(await input.inputValue(), "Add a Add a regression. regression test for the.");
        assert.equal(await ghost.count(), 0);
      }

      // Leaving the chat cancels; its late result never reaches the draft.
      await mic.click();
      await fixture("Hear words").click();
      await fixture("Switch chat").click();
      await input.waitFor({ state: "detached" });
      await fixture("Complete transcription").click();
      await fixture("Switch chat").click();
      await input.waitFor();
      assert.equal(await input.inputValue(), "Add a Add a regression. regression test for the.");

      // A microphone that drops keeps what it heard, and says so in the bar.
      await open("", "Draft:");
      await mic.click();
      await fixture("Hear words").click();
      await fixture("Drop microphone").click();
      await page.getByRole("alert").filter({ hasText: "What it heard so far" }).waitFor();
      assert.equal(await input.inputValue(), "Draft: Add a regression");
      await audit("dropped");
      await page.getByRole("button", { name: "Close dictation", exact: true }).click();
      await bar.waitFor({ state: "detached" });
      assert.equal(await page.locator('summary[aria-label="Add to message"]').isVisible(), true);

      // Finishing with nothing heard explains, inside the bar, and offers another try.
      await mic.click();
      await page.getByRole("status").filter({ hasText: "Listening" }).waitFor();
      await finish.click();
      await fixture("Complete transcription").click();
      await page.getByRole("alert").filter({ hasText: "No speech was detected" }).waitFor();
      assert.equal(await input.inputValue(), "Draft: Add a regression");
      await page.keyboard.press("Escape");
      await bar.waitFor({ state: "detached" });

      const retry = page.getByRole("button", { name: "Try again", exact: true });
      await open("denied", "Keep this draft");
      await mic.click();
      await page.getByRole("alert").filter({ hasText: "Microphone access" }).waitFor();
      assert.equal(await focusedName(), "");
      assert.equal(
        await page.evaluate(() => document.activeElement?.textContent?.trim()),
        "Try again",
      );
      await audit("permission-error");
      assert.equal(await input.inputValue(), "Keep this draft");
      assert.equal(await retry.count(), 1);
      // Typing moves on from a failure: the controls come back.
      await input.press("End");
      await input.pressSequentially("!");
      await bar.waitFor({ state: "detached" });
      assert.equal(await input.inputValue(), "Keep this draft!");

      await open("missing");
      await mic.click();
      await page.getByRole("alert").filter({ hasText: "Reinstall Ghost" }).waitFor();
      assert.equal(await retry.count(), 1);

      await open("unsupported");
      await mic.click();
      await page.getByRole("alert").filter({ hasText: "AVX2" }).waitFor();
      await audit("unsupported");
      assert.equal(await retry.count(), 0);
      assert.equal(await focusedName(), "Close dictation");
      assert.deepEqual(errors, []);
    } finally {
      await context.close();
    }
  }

  // With full motion: the ghost draws the draft exactly as the message box does, the finished
  // words sweep into ink and leave the message box showing, and cancelled ones dissolve.
  const context = await browser.newContext({
    viewport: { width: 1024, height: 700 },
    colorScheme: "dark",
    reducedMotion: "no-preference",
  });
  try {
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const input = page.getByTestId("composer-input");
    const mic = page.getByRole("button", { name: "Dictate a message", exact: true });
    const ghost = page.locator(".dictation-ghost");
    // The fixture's controls act without taking focus, as the engine does.
    const fixture = (name) => ({
      click: () => page.getByRole("button", { name, exact: true }).evaluate((node) => node.click()),
    });
    const draft =
      "Line the ghost up with the message box so that nothing moves when the dictated words land in it";
    await page.goto(`${url}/?preview=dictation&theme=dark&draft=${encodeURIComponent(draft)}`);
    await input.waitFor();
    await page.waitForFunction(
      () => !document.querySelector('[data-testid="composer-input"]').disabled,
    );
    await page.evaluate(() => document.fonts.ready);
    // Refit after the font arrives, then compare the draft as each draws it.
    await input.press("End");
    await input.pressSequentially(".");
    await input.press("Backspace");
    await page.addStyleTag({
      content:
        ".dictation-caret::after { visibility: hidden; } .chat-composer { box-shadow: none !important; transition: none !important; }",
    });
    const box = await input.boundingBox();
    // The text lines, above the bottom padding, where the bar below can tint the glass a shade.
    const clip = { x: box.x, y: box.y, width: box.width, height: box.height - 8 };
    const drawn = await page.screenshot({ clip });
    await mic.click();
    await page.getByRole("meter", { name: "Microphone level" }).waitFor();
    await page.waitForTimeout(400);
    assert.deepEqual(await input.boundingBox(), box);
    assert.equal(await differingPixels(page, drawn, await page.screenshot({ clip })), 0);

    await fixture("Hear words").click();
    await page.getByRole("button", { name: "Finish dictation", exact: true }).click();
    await fixture("Complete transcription").click();
    await page.locator('.dictation-ghost[data-mode="set"]').waitFor();
    assert.equal(await input.inputValue(), `${draft} Add a regression.`);
    await ghost.waitFor({ state: "detached", timeout: 1400 });

    await mic.click();
    await fixture("Hear words").click();
    await page.keyboard.press("Escape");
    await page.locator('.dictation-ghost[data-mode="dissolve"]').waitFor();
    await ghost.waitFor({ state: "detached", timeout: 1400 });
    assert.equal(await input.inputValue(), `${draft} Add a regression.`);

    // Typing while the words set shows the message box at once.
    await mic.click();
    await fixture("Hear words").click();
    await page.getByRole("button", { name: "Finish dictation", exact: true }).click();
    await fixture("Complete transcription").click();
    await page.locator('.dictation-ghost[data-mode="set"]').waitFor();
    await input.pressSequentially("!");
    assert.equal(await ghost.count(), 0);
    assert.equal(await input.inputValue(), `${draft} Add a regression. Add a regression.!`);
    await page.screenshot({ path: resolve(out, `${engine}-motion.png`) });
    assert.deepEqual(errors, []);
  } finally {
    await context.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/dictation-browser/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkDictation(browser, process.env.CHECK_URL ?? "http://localhost:1448", out, engine);
      console.log(
        `${engine}: dictation bar, live words, finishing, cancelling, failures, alignment and motion passed in both appearances`,
      );
    } finally {
      await browser.close();
    }
  }
}
