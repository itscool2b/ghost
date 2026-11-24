/** Image flows on the production components through the owned browser fixture. */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";

const out = fileURLToPath(new URL("../../../target/images-browser/", import.meta.url));
await mkdir(out, { recursive: true });
for (const [engine, launcher] of Object.entries({ chromium, webkit })) {
  const browser = await launcher.launch();
  try {
    for (const theme of ["light", "dark"]) {
      const context = await browser.newContext({
        viewport: { width: 1280, height: 900 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(12000);
      const errors = [];
      const remote = [];
      page.on("pageerror", (error) => errors.push(error.message));
      page.on("request", (request) => {
        if (request.url().includes("example.invalid")) remote.push(request.url());
      });
      await page.goto(`${process.env.CHECK_URL ?? "http://127.0.0.1:1471"}/?preview=images`);
      const input = page.getByTestId("composer-input");
      await page
        .getByRole("button", { name: "View Draft screen.png", exact: true })
        .locator("img")
        .waitFor();
      await page
        .getByRole("button", { name: "View Attached image 1", exact: true })
        .locator("img")
        .waitFor();
      await page
        .getByRole("button", { name: "View Revised screen", exact: true })
        .locator("img")
        .waitFor();
      await page
        .getByRole("button", { name: "View Image unavailable Missing output", exact: true })
        .getByText("Image unavailable")
        .waitFor();
      assert.equal(await input.inputValue(), "Keep the controls compact");
      await page
        .getByRole("button", { name: "View Generated image", exact: true })
        .locator("img")
        .waitFor();
      assert.ok(
        await page.evaluate(() => window.imageCalls.some((call) => call.source.type === "stored")),
      );
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-draft.png`) });
      const draft = page.getByRole("button", { name: "View Draft screen.png", exact: true });
      await draft.focus();
      await page.keyboard.press("Enter");
      const dialog = page.getByRole("dialog", { name: "Draft screen.png" });
      await dialog.locator("img").waitFor();
      assert.ok(await dialog.locator("img").evaluate((image) => image.naturalWidth > 0));
      await page.waitForFunction(() =>
        document
          .getAnimations()
          .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
      );
      assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-enlarged.png`) });
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "hidden" });
      await page.waitForFunction(
        () => document.activeElement?.getAttribute("aria-label") === "View Draft screen.png",
      );
      assert.equal(await draft.evaluate((node) => node === document.activeElement), true);
      await page.getByRole("button", { name: "Remove Draft screen.png", exact: true }).click();
      assert.equal(await draft.count(), 0);
      assert.equal(await input.inputValue(), "Keep the controls compact");
      await page.locator('summary[aria-label="Add to message"]').click();
      await page.getByRole("button", { name: "Files and images", exact: true }).click();
      await draft.locator("img").waitFor();
      await page.getByRole("button", { name: "Reattach chat", exact: true }).click();
      await draft.locator("img").waitFor();
      assert.equal(await input.inputValue(), "Keep the controls compact");
      await page.getByRole("button", { name: "Fail image reads", exact: true }).click();
      await draft.click();
      await dialog.getByText("Image unavailable").waitFor();
      // Recovery is external to the dialog; close it, restore the fixture and reopen.
      await page.keyboard.press("Escape");
      await page.getByRole("button", { name: "Recover image reads", exact: true }).click();
      await draft.click();
      await dialog.locator("img").waitFor();
      await page.keyboard.press("Escape");
      await input.fill("Send the screen");
      await page.keyboard.press("Enter");
      await page.waitForFunction(() => window.sentImages.length === 1);
      assert.deepEqual(await page.evaluate(() => window.sentImages[0]), {
        text: "Send the screen",
        attachments: ["draft-image"],
        mentions: [],
      });
      await page.waitForFunction(() => !document.querySelector(".composer-context .chat-image"));
      await page.getByRole("button", { name: "Reattach chat", exact: true }).click();
      await page
        .getByRole("button", { name: "View Attached image 1", exact: true })
        .last()
        .locator("img")
        .waitFor();
      for (const width of [1280, 960, 390]) {
        await page.setViewportSize({ width, height: 900 });
        // The virtualizer measures wrapped rows over two frames after a width change.
        await page.evaluate(
          () => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))),
        );
        await page.waitForFunction(() =>
          document
            .getAnimations()
            .every(
              (a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity,
            ),
        );
        assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
        assert.equal(
          await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
          false,
        );
        await page.screenshot({ path: resolve(out, `${engine}-${theme}-${width}.png`) });
      }
      assert.deepEqual(errors, []);
      assert.deepEqual(remote, []);
      await context.close();
    }
    console.log(
      `${engine}: image drafts, persisted history, local agent images, missing paths, keyboard dialogs, retry, send and light/dark accessibility passed`,
    );
  } finally {
    await browser.close();
  }
}
