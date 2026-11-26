/** The About page opens bundled third-party notices in a dialog without leaving settings. */
import assert from "node:assert/strict";
import { resolve } from "node:path";
import AxeBuilder from "@axe-core/playwright";

// Sheets fade in and controls settle their colours; audit only once every finite animation ends.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

export async function checkNotices(page, out, engine) {
  await page.getByRole("button", { name: "View", exact: true }).click();
  const dialog = page.getByRole("dialog", { name: "Third-party notices" });
  const text = dialog.getByRole("region", { name: "Notice text" });
  await text.waitFor();
  const content = await text.textContent();
  assert.match(content, /^Third-party software notices/);
  // A vendored source the generator adds by hand, so the dialog shows the whole catalogue.
  assert.match(content, /ggml \(transcribe-cpp-sys source\)/);
  await settled(page);
  assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
  await page.screenshot({ path: resolve(out, `${engine}-notices.png`) });
  await text.focus();
  await page.keyboard.press("End");
  // Browsers animate keyboard scrolling, so wait for movement rather than sampling once.
  await page.waitForFunction(
    () => document.querySelector(".settings-notices-text")?.scrollTop > 0,
    null,
    { timeout: 5000 },
  );
  await page.keyboard.press("Escape");
  await dialog.waitFor({ state: "hidden" });
  await page.getByRole("heading", { name: "About Ghost ADE", exact: true }).waitFor();
}
