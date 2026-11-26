/** Real browser checks of the production setup UI with synthetic Rust command responses. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";
import { choose, chosen, labels, offered } from "./choose.mjs";

// Sheets fade in and controls settle their colours; audit only once every finite animation ends.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = resolve(app, "../../target/setup-browser");
const url = "http://127.0.0.1:1463";
const require = createRequire(import.meta.url);
const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
const server = spawn(
  process.execPath,
  [vite, "--host", "127.0.0.1", "--port", "1463", "--strictPort"],
  { cwd: app, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
);
let output = "";
for (const stream of [server.stdout, server.stderr])
  stream.on("data", (data) => {
    output = (output + data).slice(-8192);
  });
try {
  await mkdir(out, { recursive: true });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (server.exitCode !== null) throw new Error(output);
    try {
      ready = (await fetch(url)).ok;
    } catch {
      /* Wait for the owned preview server. */
    }
    if (ready) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.ok(ready, output);
  for (const [engine, launcher] of Object.entries({ chromium, webkit })) {
    const browser = await launcher.launch({ headless: true });
    try {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 900 },
        reducedMotion: "reduce",
      });
      await context.route("**/*", (route) =>
        new URL(route.request().url()).hostname === "127.0.0.1" ? route.continue() : route.abort(),
      );
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      for (const theme of ["light", "dark"]) {
        await page.emulateMedia({ colorScheme: theme });
        await page.goto(`${url}/?preview=setup&theme=system`);
        await page.getByRole("button", { name: "Get started" }).waitFor();
        assert.equal(await page.locator("html").getAttribute("data-theme"), theme);
        assert.equal(await page.locator(".g-swimmer, .g-creature, .g-shallows").count(), 0);
      }
      await page.goto(`${url}/?preview=setup`);
      await page.getByRole("button", { name: "Get started" }).waitFor();
      await page.waitForFunction(() => !document.querySelector("[data-cinematic]"));
      // Continuing accepts BASH.AI LLC's Terms of Service and Privacy Policy, linked in the line;
      // nothing is accepted by looking.
      const legal = page.getByTestId("setup-legal");
      assert.match(
        await legal.textContent(),
        /By continuing, you agree to Ghost’s Terms of Service and Privacy Policy\./,
      );
      assert.equal(
        await legal.getByRole("link", { name: "Terms of Service" }).getAttribute("href"),
        "https://onghost.dev/terms",
      );
      assert.equal(
        await legal.getByRole("link", { name: "Privacy Policy" }).getAttribute("href"),
        "https://onghost.dev/privacy",
      );
      assert.equal(await page.evaluate(() => localStorage.getItem("o7.legal.accepted")), null);
      await page.screenshot({ path: resolve(out, `${engine}-welcome.png`) });
      // The setup column, wordmark and Skip setup stay clear of each other at both sizes.
      const checkLayout = async () => {
        const layout = await page.evaluate(() => {
          const box = (node) => {
            const rect = node.getBoundingClientRect();
            return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
          };
          return {
            window: { width: innerWidth, height: innerHeight },
            column: box(document.querySelector(".setup-column")),
            heading: box(document.querySelector(".setup-heading")),
            top: box(document.querySelector(".setup-top")),
            skip: box(document.querySelector(".setup-skip")),
            creatures: document.querySelectorAll(".g-swimmer, .g-creature, .g-shallows").length,
            dither: document.querySelectorAll(".g-dither, canvas").length,
            overflow: document.documentElement.scrollWidth > innerWidth,
          };
        });
        const { window, column, heading, top, skip, creatures, dither, overflow } = layout;
        assert.equal(creatures, 0, `${engine}: setup must not render aquatic decoration`);
        assert.equal(dither, 0, `${engine}: setup still draws the dither field`);
        assert.ok(
          Math.abs(column.left + column.right - window.width) <= 1,
          `${engine}: column is not centred`,
        );
        assert.ok(
          heading.left >= column.left && heading.right <= column.right,
          `${engine}: heading leaves the column`,
        );
        assert.ok(heading.top >= top.bottom, `${engine}: heading runs under the top bar`);
        assert.ok(skip.right <= window.width && skip.left >= 0, `${engine}: Skip setup clipped`);
        assert.equal(overflow, false, `${engine}: setup scrolls sideways`);
      };
      await checkLayout();
      await page.setViewportSize({ width: 1024, height: 768 });
      await page.waitForFunction(() => innerWidth === 1024);
      await checkLayout();
      await page.screenshot({ path: resolve(out, `${engine}-compact.png`) });
      await page.setViewportSize({ width: 1440, height: 900 });
      const assertNoVendorCalls = async () =>
        assert.deepEqual(
          await page.evaluate(() =>
            window.setupFixture
              .methods()
              .filter((name) =>
                ["agentAuthStatus", "agentUpdate", "chatCreate", "send", "start"].includes(name),
              ),
          ),
          [],
        );
      await assertNoVendorCalls();
      // The newly announced heading is focused. Enter advances without stealing input/form keys.
      await page.keyboard.press("Enter");
      // The heading follows detection: three fixture agents are installed, six are not.
      await page
        .getByRole("heading", { name: /^Found 3 agents\.\s*6 more can be installed\.$/ })
        .waitFor();
      assert.equal(
        await page.evaluate(() => localStorage.getItem("o7.legal.accepted")),
        "2026-11-26",
        `${engine}: continuing accepted the Terms and Privacy`,
      );
      // What this computer has leads; every shown row is whole, inside the column and clear of the
      // next, and the six missing agents fold into one row.
      const rows = await page.evaluate(() => {
        const column = document.querySelector(".setup-column").getBoundingClientRect();
        return [...document.querySelectorAll(".setup-column .agent-row")].map((row) => {
          const rect = row.getBoundingClientRect();
          const name = row.querySelector(".agent-row-name strong");
          return {
            text: row.textContent,
            top: rect.top,
            bottom: rect.bottom,
            inside: rect.left >= column.left - 8 && rect.right <= column.right + 8,
            truncated: name.scrollWidth > name.clientWidth,
          };
        });
      });
      assert.equal(rows.length, 3, `${engine}: installed agents lead`);
      for (const [index, row] of rows.entries()) {
        assert.ok(row.inside && row.bottom > row.top, `${engine}: clipped ${row.text}`);
        assert.ok(!row.truncated, `${engine}: truncated ${row.text}`);
        for (const other of rows.slice(index + 1))
          assert.ok(row.bottom <= other.top, `${engine}: overlapping ${row.text} / ${other.text}`);
      }
      await page.getByRole("button", { name: "Show 6 not installed" }).waitFor();
      await page.getByRole("button", { name: /Claude Code.*Installed/ }).click();
      await assertNoVendorCalls();
      await page.screenshot({ path: resolve(out, `${engine}-discovery.png`) });
      await settled(page);
      assert.deepEqual(
        (await new AxeBuilder({ page }).analyze()).violations,
        [],
        `${engine} discovery accessibility`,
      );
      // Setup offers no account and no sign-in: the first project follows the agents.
      assert.equal(await page.getByRole("button", { name: "Open Account" }).count(), 0);
      await page.getByRole("button", { name: "Continue", exact: true }).click();
      // Linking an assistant follows the agents: the four are named with their marks, the dialog
      // opens for the one picked, and closing it links nothing and turns nothing on.
      await page.getByRole("heading", { name: /^Bring your assistant./ }).waitFor();
      assert.equal(await page.locator(".setup-bots .remote-bot-pick .g-agent-mark").count(), 4);
      await page.getByTestId("setup-link-openclaw").click();
      const linking = page.getByRole("dialog", { name: "Link a bot" });
      assert.equal(await linking.getByRole("radio", { name: "OpenClaw" }).isChecked(), true);
      await page.keyboard.press("Escape");
      await linking.waitFor({ state: "detached" });
      assert.deepEqual(await page.evaluate(() => window.remoteFixture.calls()), []);
      // A link made here shows as linked, on its button and in the picture.
      await page.getByTestId("setup-link-hermes").click();
      await linking.getByRole("button", { name: "Create link" }).click();
      await linking.getByRole("textbox", { name: "Lines for Hermes' config" }).waitFor();
      await linking.getByRole("button", { name: "Done" }).click();
      await page.getByRole("button", { name: /^Hermes\s*Linked/ }).waitFor();
      assert.deepEqual(await page.evaluate(() => window.remoteFixture.calls()), [
        "enabled:true",
        "create-bot:Hermes",
      ]);
      await settled(page);
      await page.screenshot({ path: resolve(out, `${engine}-assistant.png`) });
      assert.deepEqual(
        (await new AxeBuilder({ page }).analyze()).violations,
        [],
        `${engine} assistant accessibility`,
      );
      // The phone follows the assistant (owner, 24 November 2026): what it is for, Android's
      // setup with Google's offer and licence in full, and nothing fetched until it is accepted.
      await page.getByRole("button", { name: "Continue", exact: true }).click();
      await page.getByRole("heading", { name: /^Build phone apps./ }).waitFor();
      assert.match(
        await page.getByTestId("setup-phone-android").textContent(),
        /About 2\.5 GB from Google/,
      );
      await page.getByTestId("setup-phone-setup").click();
      await page.getByTestId("device-offer").waitFor();
      assert.match(await page.getByTestId("device-license").textContent(), /License Agreement/);
      await settled(page);
      await page.screenshot({ path: resolve(out, `${engine}-phone-offer.png`) });
      assert.deepEqual(
        (await new AxeBuilder({ page }).analyze()).violations,
        [],
        `${engine} phone offer accessibility`,
      );
      await page.getByRole("button", { name: "Not now" }).click();
      await page.getByTestId("setup-phone").waitFor();
      assert.deepEqual(
        await page.evaluate(() => window.deviceFixture.calls.filter((call) => call !== "refresh")),
        ["fetchOffer", "fetchCancel"],
      );
      await settled(page);
      await page.screenshot({ path: resolve(out, `${engine}-phone.png`) });
      assert.deepEqual(
        (await new AxeBuilder({ page }).analyze()).violations,
        [],
        `${engine} phone accessibility`,
      );
      // It is a page of the agents step: a setup left here resumes at the agents.
      await page.reload();
      await page.getByRole("heading", { name: /^Found 3 agents./ }).waitFor();
      await page.waitForFunction(() => !document.querySelector("[data-cinematic]"));
      await page.getByRole("button", { name: "Continue", exact: true }).click();
      await page.getByRole("heading", { name: /^Bring your assistant./ }).waitFor();
      await page.getByRole("button", { name: "Continue", exact: true }).click();
      await page.getByRole("heading", { name: /^Build phone apps./ }).waitFor();
      await page.getByRole("button", { name: "Continue", exact: true }).click();
      // The step survives a restart, and a setup saved at the retired motion step resumes at the
      // step that followed it.
      await page.getByRole("heading", { name: /^Add a first project\./ }).waitFor();
      await page.evaluate(() => {
        const saved = JSON.parse(sessionStorage.getItem("setup-fixture"));
        saved.preferences.step = "motion";
        sessionStorage.setItem("setup-fixture", JSON.stringify(saved));
      });
      await page.reload();
      await page.getByRole("heading", { name: /^Add a first project\./ }).waitFor();
      await page.waitForFunction(() => !document.querySelector("[data-cinematic]"));
      await page.getByRole("button", { name: /Choose a folder/ }).click();
      await page.getByRole("textbox", { name: "Project name" }).fill("A quiet project");
      await page.evaluate(() => sessionStorage.setItem("fail-setup", "yes"));
      await page.getByRole("button", { name: "Add project", exact: true }).click();
      await page.getByRole("alert").waitFor();
      assert.equal(await page.evaluate(() => window.setupFixture.projects()), 1);
      await page.evaluate(() => sessionStorage.removeItem("fail-setup"));
      await page.getByRole("button", { name: "Save selection", exact: true }).click();
      await page.getByText("A quiet project is ready.", { exact: true }).waitFor();
      assert.equal(
        await page.evaluate(() => window.setupFixture.projects()),
        1,
        "Retry duplicated the project",
      );
      await settled(page);
      assert.deepEqual(
        (await new AxeBuilder({ page }).analyze()).violations,
        [],
        `${engine} project accessibility`,
      );
      await page.getByRole("button", { name: "Open Ghost", exact: true }).click();
      await page.getByRole("heading", { name: "Workspace ready" }).waitFor();
      await page.reload();
      await page.getByRole("heading", { name: "Workspace ready" }).waitFor();
      await page.getByRole("button", { name: "Agents", exact: true }).click();
      // Agents: a row's menu owns the keyboard while open and hands focus back on Escape; the
      // page has no axe violations with it open, and opening it runs no vendor command.
      await page.getByRole("button", { name: "Claude Code actions" }).focus();
      await page.keyboard.press("ArrowDown");
      await page.getByRole("menu", { name: "Claude Code actions" }).waitFor();
      assert.equal(
        await page.evaluate(() => document.activeElement?.getAttribute("role")),
        "menuitem",
      );
      await settled(page);
      assert.deepEqual(
        (await new AxeBuilder({ page }).analyze()).violations,
        [],
        `${engine} Agents accessibility`,
      );
      await page.keyboard.press("Escape");
      assert.equal(await page.getByRole("menu").count(), 0);
      assert.equal(
        await page.evaluate(() => document.activeElement?.getAttribute("aria-label")),
        "Claude Code actions",
      );
      await assertNoVendorCalls();
      // One entry per agent: Claude Code's ACP adapter is a connection in its details, chosen
      // there (Automatic by default), never a second row.
      assert.deepEqual(
        await page.locator(".agent-row-name strong").allTextContents(),
        [
          "Hermes",
          "Claude Code",
          "Codex",
          "Antigravity",
          "Cursor",
          "Grok Build",
          "oh-my-pi",
          "OpenCode",
          "Pi",
        ],
        `${engine}: one row per agent`,
      );
      await page
        .locator(".agent-row", { has: page.locator("strong", { hasText: /^Claude Code$/ }) })
        .locator(".agent-row-toggle")
        .click();
      const connection = page.getByRole("combobox", { name: "Claude Code connection" });
      await connection.waitFor();
      assert.equal(await chosen(connection), "", `${engine}: Automatic by default`);
      assert.deepEqual(await labels(connection), ["Automatic (Direct)", "Direct", "ACP adapter"]);
      await choose(connection, "claude-acp");
      await page.waitForFunction(() =>
        window.setupFixture.methods().includes("agentSetConnection"),
      );
      await offered(
        page.getByRole("combobox", { name: "Claude Code connection" }),
        "Automatic (ACP adapter)",
      ).waitFor({ state: "attached" });
      assert.equal(await chosen(connection), "claude-acp");
      await page.screenshot({ path: resolve(out, `${engine}-agents-connection.png`) });
      await settled(page);
      assert.deepEqual(
        (await new AxeBuilder({ page }).analyze()).violations,
        [],
        `${engine} Agents connection accessibility`,
      );
      await choose(connection, "");
      await offered(
        page.getByRole("combobox", { name: "Claude Code connection" }),
        "Automatic (Direct)",
      ).waitFor({ state: "attached" });
      assert.equal(await chosen(connection), "");
      await assertNoVendorCalls();
      await page.getByRole("button", { name: "Run setup again" }).click();
      await page.getByRole("button", { name: "Get started" }).waitFor();
      await page.getByRole("button", { name: "Skip setup", exact: true }).click();
      await page.getByRole("heading", { name: "Workspace ready" }).waitFor();
      // Clone as the first project, by keyboard: the form shows the exact command first; Enter
      // clones, and the new folder becomes the project at once. Escape leaves the form.
      const cloning = await context.newPage();
      cloning.on("pageerror", (error) => errors.push(error.message));
      await cloning.goto(`${url}/?preview=clone`);
      await cloning.getByRole("heading", { name: /^Clone a repository\./ }).waitFor();
      await cloning
        .getByText(
          "git clone --progress -- https://github.com/auracode/orbit C:\\Users\\fixture\\code\\orbit",
          { exact: true },
        )
        .waitFor();
      assert.deepEqual(await cloning.evaluate(() => window.setupFixture.cloneCalls()), [
        "clone-check",
      ]);
      await settled(cloning);
      assert.deepEqual(
        (await new AxeBuilder({ page: cloning }).analyze()).violations,
        [],
        `${engine} clone form accessibility`,
      );
      await cloning.getByRole("textbox", { name: "Repository" }).press("Enter");
      await cloning.getByText("orbit is ready.", { exact: true }).waitFor();
      assert.deepEqual(await cloning.evaluate(() => window.setupFixture.folders()), [
        "C:\\Users\\fixture\\code\\orbit",
      ]);
      await cloning.getByRole("button", { name: "Clone a repository", exact: true }).click();
      await cloning.getByRole("textbox", { name: "Repository" }).press("Escape");
      await cloning.getByRole("heading", { name: /^Add a first project\./ }).waitFor();
      await cloning.close();
      const empty = await context.newPage();
      await empty.goto(`${url}/?preview=setup&missing`);
      await empty.getByRole("button", { name: "Get started" }).click();
      await empty.getByRole("button", { name: /Explore the local demo/ }).click();
      await empty.getByRole("heading", { name: "Local demo" }).waitFor();
      const small = await context.newPage();
      await small.setViewportSize({ width: 720, height: 600 });
      await small.goto(`${url}/?preview=setup`);
      await small.getByRole("button", { name: "Get started" }).waitFor();
      assert.equal(await small.evaluate(() => document.body.scrollWidth > innerWidth), false);
      await small.screenshot({ path: resolve(out, `${engine}-small.png`) });
      const intro = await browser.newContext({
        viewport: { width: 1440, height: 900 },
        reducedMotion: "no-preference",
      });
      const introPage = await intro.newPage();
      await introPage.goto(`${url}/?preview=setup`);
      await introPage.getByRole("button", { name: "Skip intro" }).waitFor();
      await introPage.keyboard.press("Escape");
      await introPage.waitForFunction(() => !document.querySelector("[data-cinematic]"));
      await introPage.getByRole("button", { name: "Skip setup", exact: true }).click();
      await introPage.getByRole("heading", { name: "Workspace ready" }).waitFor();
      // Skipped once, setup stays skipped: the reload holds the formed mark for a moment, never
      // the first launch's dissolve, and shows the ready workspace. Timing the heading cannot say
      // which played: on CI's macOS the page alone takes over 1.5 s to start.
      await introPage.addInitScript(() => {
        window.__dissolvePlayed = false;
        new MutationObserver(() => {
          if (document.querySelector('[data-mode="live"]')) window.__dissolvePlayed = true;
        }).observe(document, { childList: true, subtree: true, attributes: true });
      });
      await introPage.reload();
      await introPage.getByRole("heading", { name: "Workspace ready" }).waitFor();
      assert.equal(
        await introPage.evaluate(() => window.__dissolvePlayed),
        false,
        `${engine}: the first launch's dissolve played again after a reload`,
      );
      await intro.close();
      assert.deepEqual(errors, [], `${engine} page errors`);
      console.log(
        `${engine}: setup persistence, restart/skip, storage retry without duplicate project, retired-step resume, centred layout, clone by keyboard into a first project, Agents menu keyboard, one row per agent with its Connection setting, local demo, passive detection and accessibility passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  server.kill();
}
