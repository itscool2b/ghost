import assert from "node:assert/strict";
import { resolve } from "node:path";

/** Settings > Chats, with synthetic persistence and failed-save responses. */
export async function checkAutomaticCheckpoints(page, out, label) {
  const checkpoints = page.getByRole("switch", { name: "Automatic checkpoints", exact: true });
  await checkpoints.waitFor();
  assert.equal(await checkpoints.isChecked(), false);
  assert.equal(
    await page.evaluate(() => window.settingsFixture.general().automatic_checkpoints),
    false,
  );
  await page
    .getByText("Ghost's per-turn file undo is unavailable for new messages.", { exact: false })
    .waitFor();

  await checkpoints.check();
  await page.waitForFunction(() => window.settingsFixture.general().automatic_checkpoints);
  await page.getByText("Saving delays sending.", { exact: false }).waitFor();

  // Saved state is what the page shows after visiting another.
  const sections = page.getByRole("navigation", { name: "Settings sections" });
  await sections.getByRole("button", { name: "Shortcuts", exact: true }).click();
  await sections.getByRole("button", { name: "Chats", exact: true }).click();
  await checkpoints.waitFor();
  assert.equal(await checkpoints.isChecked(), true);

  // A failed attempt to disable protection must leave its last saved state visible.
  await page.evaluate(() => sessionStorage.setItem("settings-fail-save", "yes"));
  await checkpoints.click();
  await page.getByRole("alert").filter({ hasText: "Preferences could not be saved" }).waitFor();
  assert.equal(await checkpoints.isChecked(), true);
  assert.equal(
    await page.evaluate(() => window.settingsFixture.general().automatic_checkpoints),
    true,
  );
  await page.evaluate(() => sessionStorage.removeItem("settings-fail-save"));
  await page.getByRole("button", { name: "Reload preferences" }).click();
  await page.getByRole("alert").waitFor({ state: "hidden" });

  await checkpoints.uncheck();
  await page.waitForFunction(() => !window.settingsFixture.general().automatic_checkpoints);
  await sections.getByRole("button", { name: "Shortcuts", exact: true }).click();
  await sections.getByRole("button", { name: "Chats", exact: true }).click();
  await checkpoints.waitFor();
  assert.equal(await checkpoints.isChecked(), false);
  await page
    .getByText("Saved checkpoints remain available in Changes.", { exact: false })
    .waitFor();
  await checkpoints.scrollIntoViewIfNeeded();
  await page.screenshot({ path: resolve(out, `${label}-automatic-checkpoints.png`) });
}
