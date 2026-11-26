import assert from "node:assert/strict";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

export async function checkSetupRecovery(browser, url) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.setDefaultTimeout(15000);
  try {
    await page.goto(`${url}/?preview=setup-recovery`);
    await page.getByRole("region", { name: "Review setup command" }).waitFor();
    assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
    const compose = page.getByTestId("composer-input");
    await compose.fill("first draft");
    await compose.press("Enter");
    await compose.fill("second");
    await compose.press("Enter");
    const rows = page.locator('[data-testid="queue"] [data-test-prompt-id]');
    await rows.nth(0).getByRole("button", { name: "Edit queued message" }).click();
    const edit = page.getByRole("textbox", { name: "Queued message", exact: true });
    await edit.fill("first edited");
    await edit.press("Enter");
    await rows.nth(1).getByRole("button", { name: "Move up in the queue" }).click();
    await page.getByRole("button", { name: "Run reviewed setup" }).click();
    await page.waitForFunction(() => window.setupRecoveryFixture.calls().includes("setup-start"));
    await compose.fill("third while running");
    await compose.press("Enter");
    assert.equal(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).some((call) =>
        call.startsWith("prompt:"),
      ),
      false,
    );
    if (process.env.CHECK_SCREENSHOT) await page.screenshot({ path: process.env.CHECK_SCREENSHOT });
    await page.evaluate(() => window.setupRecoveryFixture.complete(0));
    await page.waitForFunction(
      () =>
        window.setupRecoveryFixture.calls().filter((call) => call.startsWith("prompt:")).length ===
        3,
    );
    await page.evaluate(() => window.setupRecoveryFixture.complete(0));
    assert.deepEqual(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).filter((call) =>
        call.startsWith("prompt:"),
      ),
      ["prompt:second", "prompt:first edited", "prompt:third while running"],
    );

    // Recovery is reachable by a registry ID with no saved chat, and remains listed afterward.
    const retained = page.getByRole("region", { name: "Retained worktrees" });
    await retained.getByText("ghost/retained", { exact: true }).waitFor();
    await retained.getByRole("button", { name: "Open folder" }).click();
    await retained.getByRole("button", { name: "Review recovery" }).click();
    await retained.getByRole("button", { name: "Apply reviewed recovery" }).click();
    await retained
      .getByText("The worktree is reconciled and retained without a saved chat.")
      .waitFor();
    const recoveryCalls = await page.evaluate(() => window.setupRecoveryFixture.calls());
    for (const call of [
      "open-folder:orphan-tree",
      "recovery-review:orphan-tree",
      "recovery-apply:orphan-tree",
    ])
      assert.ok(recoveryCalls.includes(call));

    // Failure, an old generation and terminal close cannot release held messages.
    await page.reload();
    await page.getByRole("region", { name: "Review setup command" }).waitFor();
    await compose.fill("held through retries");
    await compose.press("Enter");
    await page.getByRole("button", { name: "Run reviewed setup" }).click();
    await page.waitForFunction(
      () =>
        window.setupRecoveryFixture.calls().filter((call) => call === "setup-start").length === 1,
    );
    await page.evaluate(() => window.setupRecoveryFixture.complete(2));
    const setupStatus = page.getByRole("region", { name: "Setup status" });
    await setupStatus.getByText("Setup exited with code 2. Retry setup.").waitFor();
    await setupStatus.getByRole("button", { name: "Retry setup" }).click();
    await page.getByRole("button", { name: "Run reviewed setup" }).click();
    await page.waitForFunction(
      () =>
        window.setupRecoveryFixture.calls().filter((call) => call === "setup-start").length === 2,
    );
    await page.evaluate(() => window.setupRecoveryFixture.complete(0, 2));
    assert.equal(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).some((call) =>
        call.startsWith("prompt:"),
      ),
      false,
    );
    await page.getByRole("button", { name: "Close Setup", exact: true }).click();
    await setupStatus.getByText("Setup exited with code 1. Retry setup.").waitFor();
    assert.equal((await page.evaluate(() => window.setupRecoveryFixture.queue()))[0].held, true);
    await setupStatus.getByRole("button", { name: "Retry setup" }).click();
    await page.getByRole("button", { name: "Run reviewed setup" }).click();
    await page.waitForFunction(
      () =>
        window.setupRecoveryFixture.calls().filter((call) => call === "setup-start").length === 3,
    );
    await page.evaluate(() => window.setupRecoveryFixture.complete(0));
    assert.equal(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).some((call) =>
        call.startsWith("prompt:"),
      ),
      false,
    );
    await page.getByTestId("queue-send-held").click();
    await page.waitForFunction(() =>
      window.setupRecoveryFixture.calls().includes("prompt:held through retries"),
    );

    // A restart holds queued messages and asks for review, without silently rerunning setup.
    await page.reload();
    await page.getByRole("region", { name: "Review setup command" }).waitFor();
    await compose.fill("held after restart");
    await compose.press("Enter");
    await page.evaluate(() => window.setupRecoveryFixture.restart());
    await page
      .getByRole("region", { name: "Setup status" })
      .getByRole("button", { name: "Retry setup" })
      .waitFor();
    assert.equal(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).includes("setup-start"),
      false,
    );
    assert.equal((await page.evaluate(() => window.setupRecoveryFixture.queue()))[0].held, true);

    // Sending one held row preserves other holds; new messages wait behind the held head.
    await compose.fill("selected held after restart");
    await compose.press("Enter");
    await setupStatus.getByRole("button", { name: "Retry setup" }).click();
    await page.getByRole("button", { name: "Run reviewed setup" }).click();
    await page.waitForFunction(() => window.setupRecoveryFixture.calls().includes("setup-start"));
    await page.evaluate(() => window.setupRecoveryFixture.complete(0));
    await compose.fill("new behind held head");
    await compose.press("Enter");
    assert.equal(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).some((call) =>
        call.startsWith("prompt:"),
      ),
      false,
    );
    await rows.nth(1).getByTestId("queued-send").click();
    await page.waitForFunction(() =>
      window.setupRecoveryFixture.calls().includes("prompt:selected held after restart"),
    );
    assert.deepEqual(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).filter((call) =>
        call.startsWith("prompt:"),
      ),
      ["prompt:selected held after restart"],
    );
    assert.deepEqual(
      (await page.evaluate(() => window.setupRecoveryFixture.queue())).map((row) => ({
        preview: row.preview,
        held: row.held,
      })),
      [
        { preview: "held after restart", held: true },
        { preview: "new behind held head", held: false },
      ],
    );
    await page.getByTestId("queue-send-held").click();
    await page.waitForFunction(() =>
      window.setupRecoveryFixture.calls().includes("prompt:new behind held head"),
    );
    assert.deepEqual(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).filter((call) =>
        call.startsWith("prompt:"),
      ),
      [
        "prompt:selected held after restart",
        "prompt:held after restart",
        "prompt:new behind held head",
      ],
    );

    // Another setup failure holds new retry messages even when an older held head already exists.
    await page.reload();
    await page.getByRole("region", { name: "Review setup command" }).waitFor();
    await compose.fill("held before retry");
    await compose.press("Enter");
    await page.getByRole("button", { name: "Run reviewed setup" }).click();
    await page.waitForFunction(() => window.setupRecoveryFixture.calls().includes("setup-start"));
    await page.evaluate(() => window.setupRecoveryFixture.complete(2));
    await setupStatus.getByRole("button", { name: "Retry setup" }).click();
    await page.getByRole("button", { name: "Run reviewed setup" }).click();
    await page.waitForFunction(
      () =>
        window.setupRecoveryFixture.calls().filter((call) => call === "setup-start").length === 2,
    );
    await compose.fill("unheld during retry");
    await compose.press("Enter");
    assert.equal(await rows.nth(1).getAttribute("data-test-held"), "false");
    await page.evaluate(() => window.setupRecoveryFixture.complete(2));
    await setupStatus.getByText("Setup exited with code 2. Retry setup.").waitFor();
    assert.equal(await rows.nth(0).getAttribute("data-test-held"), "true");
    assert.equal(await rows.nth(1).getAttribute("data-test-held"), "true");
    await rows.nth(1).getByTestId("queued-send").waitFor();
    assert.deepEqual(
      (await page.evaluate(() => window.setupRecoveryFixture.calls())).filter((call) =>
        call.startsWith("prompt:"),
      ),
      [],
    );
    assert.deepEqual(errors, []);
  } finally {
    await context.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium } = await import("playwright");
  const browser = await chromium.launch();
  try {
    await checkSetupRecovery(browser, process.env.CHECK_URL ?? "http://localhost:1478");
    console.log(
      "Chromium: setup compose/edit/reorder, owner completion, restart and chatless recovery passed",
    );
  } finally {
    await browser.close();
  }
}
