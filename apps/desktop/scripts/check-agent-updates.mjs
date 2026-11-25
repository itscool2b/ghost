/**
 * Agent update notices in the real chat screen's notifications panel (`?preview=agent-updates`),
 * behind the sidebar's bell. Installs nothing.
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

export async function checkAgentUpdates(browser, url, out, engine) {
  for (const theme of ["light", "dark"]) {
    const context = await browser.newContext({
      viewport: { width: 1280, height: 800 },
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    try {
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const notice = (agent) =>
        page.locator(`[data-testid="agent-update-banner"][data-agent="${agent}"]`);
      const bell = page.locator(".chat-inbox > summary");
      const attention = () => page.getByTestId("notifications-bell").getAttribute("data-attention");
      const panel = page.getByRole("region", { name: "Notifications", exact: true });
      const open = async (query = "") => {
        await page.goto(`${url}/?preview=agent-updates&theme=${theme}${query}`);
        await bell.waitFor();
        await bell.click();
        await notice("claude").waitFor();
      };

      // A copy that never accepted the Terms of Service and Privacy Policy is shown them once, with
      // both links, until OK; a reload after OK shows nothing.
      await page.goto(`${url}/?preview=agent-updates&theme=${theme}&legal=new`);
      const legal = page.getByTestId("legal-banner");
      await legal.waitFor();
      assert.match(await legal.textContent(), /o7 has Terms of Service and a Privacy Policy/);
      assert.equal(
        await legal.getByRole("link", { name: "Privacy Policy" }).getAttribute("href"),
        "https://o7.codes/privacy",
      );
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-legal.png`) });
      await legal.getByRole("button", { name: "OK" }).click();
      await legal.waitFor({ state: "detached" });
      assert.equal(
        await page.evaluate(() => localStorage.getItem("o7.legal.accepted")),
        "2026-11-26",
      );

      // New releases wait behind the bell: nothing floats over the pane, the bell has its dot.
      await page.goto(`${url}/?preview=agent-updates&theme=${theme}`);
      await bell.waitFor();
      await page.waitForFunction(
        () => document.querySelectorAll('[data-testid="agent-update-banner"]').length === 3,
      );
      assert.equal(await page.locator(".ui-toasts").count(), 0, "no update floats over the pane");
      assert.equal(await notice("claude").isVisible(), false, "the notices wait in the panel");
      assert.equal(await attention(), "new");
      assert.equal(await bell.getAttribute("aria-label"), "Notifications, 3, 3 new");
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-bell.png`) });
      await bell.click();
      await notice("claude").waitFor();
      // Opening the panel counts them as seen.
      await page.waitForFunction(
        () =>
          !document
            .querySelector('[data-testid="notifications-bell"]')
            ?.hasAttribute("data-attention"),
      );
      assert.equal(await bell.getAttribute("aria-label"), "Notifications, 3");
      await page.keyboard.press("Escape");
      await panel.waitFor({ state: "hidden" });
      await open();
      assert.equal(
        await page.getByTestId("agent-update-banner").count(),
        3,
        "up-to-date agents stay quiet",
      );
      await notice("claude")
        .getByText("Claude Code 2.1.400 is available", { exact: true })
        .waitFor();
      // Another tool owns Codex's installation: its reason, and the way to Settings > Agents.
      // The reason arrives with the installation's check, after the notice itself.
      await notice("codex")
        .getByText(/Homebrew manages this installation/)
        .waitFor();
      assert.equal(
        await notice("codex").getByRole("button", { name: "Update", exact: true }).count(),
        0,
      );
      assert.match(await notice("codex").textContent(), /Homebrew manages this installation/);
      await notice("codex").getByRole("button", { name: "Agents settings" }).click();
      assert.equal(await page.evaluate(() => "openedAgents" in document.body.dataset), true);
      // Cursor's chat is working: its update waits rather than replacing files under a turn.
      assert.match(await notice("cursor").textContent(), /Update once its working chat finishes\./);
      assert.equal(
        await notice("cursor").getByRole("button", { name: "Update", exact: true }).count(),
        0,
      );
      assert.deepEqual(
        (await new AxeBuilder({ page }).include(".chat-inbox").analyze()).violations,
        [],
      );
      // The panel rises from the bell at the foot of the sidebar and stays inside the window.
      const box = await page.locator(".chat-inbox > .chat-popover-panel").boundingBox();
      const trigger = await bell.boundingBox();
      assert.ok(box && trigger, "the panel and the bell are on screen");
      assert.ok(box.y + box.height <= trigger.y, "the panel opens above the bell");
      assert.ok(box.y >= 0 && box.x >= 0 && box.x + box.width <= 1280, "the panel fits the window");
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-notices.png`) });

      // X hides a notice for this run of o7 only.
      await notice("cursor")
        .getByRole("button", { name: "Dismiss the Cursor update notice" })
        .click();
      assert.equal(await notice("cursor").count(), 0);
      // Update: it reads as updating, then goes once detection finds the new version.
      await notice("claude").getByRole("button", { name: "Update", exact: true }).click();
      await notice("claude").getByRole("button", { name: "Updating…" }).waitFor();
      await notice("claude").waitFor({ state: "detached" });
      // A restart shows the dismissed notice again, as the agent is still not updated.
      await open();
      await notice("cursor").waitFor();

      // A failed update says why and offers its command in o7's terminal, where its
      // output shows; once that passes and the sheet closes, the notice goes.
      await open("&case=fail");
      await notice("claude").getByRole("button", { name: "Update", exact: true }).click();
      await notice("claude").getByRole("alert").filter({ hasText: "The update failed" }).waitFor();
      // A failed update turns the bell red until it is settled, open panel or not.
      await page.waitForFunction(
        () =>
          document.querySelector('[data-testid="notifications-bell"]')?.dataset.attention ===
          "error",
      );
      await notice("claude").getByRole("button", { name: "Run in terminal" }).click();
      const sheet = page.getByRole("dialog", { name: "Update Claude Code" });
      await sheet.waitFor();
      await sheet
        .getByText("The update finished. o7 checks the new version when you close this.")
        .waitFor();
      assert.equal(await notice("claude").count(), 1, "the notice waits while the output is read");
      await sheet.getByRole("button", { name: "Close update terminal" }).click();
      await notice("claude").waitFor({ state: "detached" });
      assert.notEqual(await attention(), "error", "a settled failure leaves the bell");
      // Try again can succeed too.
      await open("&case=fail");
      await notice("claude").getByRole("button", { name: "Update", exact: true }).click();
      await notice("claude").getByRole("alert").filter({ hasText: "The update failed" }).waitFor();
      await notice("claude").getByRole("button", { name: "Try again" }).click();
      await notice("claude").waitFor({ state: "detached" });

      // An update that finished while detection still finds the old version says so.
      await open("&case=unchanged");
      await notice("claude").getByRole("button", { name: "Update", exact: true }).click();
      await notice("claude")
        .getByText(/still finds 2\.1\.310 at .*claude\.cmd/)
        .waitFor();
      assert.equal(
        await notice("claude")
          .getByRole("button", { name: /^(Update|Updating…|Try again)$/ })
          .count(),
        0,
      );
      await notice("claude").getByRole("button", { name: "Agents settings" }).waitFor();
      assert.deepEqual(errors, []);
    } finally {
      await context.close();
    }
  }
}

/**
 * The same update from the Agents page's row (`?preview=agents`): it waits for the agent's working
 * chats, says what the version did, and keeps what the updater printed one line away.
 */
export async function checkAgentRows(browser, url, out, engine) {
  for (const theme of ["light", "dark"]) {
    const context = await browser.newContext({
      viewport: { width: 1280, height: 800 },
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    try {
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const row = page.locator(".agent-row", { hasText: "Claude Code" });
      const update = row.getByRole("button", { name: "Update Claude Code" });
      const printed = row.locator("details.agent-row-output");

      // Two of its chats have a turn under way: Update is there, off, and the line says why.
      await page.goto(`${url}/?preview=agents&working=2&theme=${theme}`);
      await row.getByText("Update once its 2 working chats finish.", { exact: false }).waitFor();
      assert.equal(await update.isDisabled(), true);
      await row.getByRole("button", { name: "Claude Code actions" }).click();
      assert.equal(await page.getByRole("menuitem", { name: "Update", exact: true }).count(), 0);
      await page.keyboard.press("Escape");

      // A failed update: the reason in red, and the command's own words folded under it.
      await page.goto(`${url}/?preview=agents&update=fail&theme=${theme}`);
      await update.click();
      await row
        .getByRole("alert")
        .getByText("The vendor's command failed with exit code 1.")
        .waitFor();
      assert.equal(await printed.evaluate((node) => node.open), false);
      await printed.getByText("What the updater printed").click();
      const text = await printed.locator("pre").textContent();
      assert.match(text, /^npm error code EBUSY\n/);
      assert.match(text, /resource busy or locked$/);
      assert.deepEqual(
        (await new AxeBuilder({ page }).include(".agent-list").analyze()).violations,
        [],
      );
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-row-failed.png`) });

      // An update that worked says the version it reached, with its output as available.
      await page.goto(`${url}/?preview=agents&theme=${theme}`);
      await update.click();
      await row.getByRole("status").getByText("Updated to 2.1.314.").waitFor();
      await printed.getByText("What the updater printed").click();
      assert.equal(await printed.locator("pre").textContent(), "changed 1 package in 4s");
      assert.equal(await update.count(), 0, "an updated agent still offered Update");
      assert.deepEqual(errors, []);
    } finally {
      await context.close();
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/agent-updates-browser/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkAgentUpdates(
        browser,
        process.env.CHECK_URL ?? "http://localhost:1448",
        out,
        engine,
      );
      console.log(
        `${engine}: agent update notices, their edge cases and dismissal passed in both appearances`,
      );
      await checkAgentRows(browser, process.env.CHECK_URL ?? "http://localhost:1448", out, engine);
      console.log(
        `${engine}: the Agents row waits for working chats and shows the updater's output`,
      );
    } finally {
      await browser.close();
    }
  }
}
