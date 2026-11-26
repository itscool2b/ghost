/**
 * Scripted theme checks (`?preview=settings`, `?preview=plan`, `?preview=permission`,
 * `?preview=new`, `?preview=settings&page=agents`). The gallery lists Aura and one theme per agent,
 * each named by its theme alone; a click or an arrow key applies a theme at once, a reload keeps
 * it, and Aura sets no palette. Every theme then passes axe, contrast included, over the chat's
 * busiest surfaces, New chat's stage and Settings > Agents, in both appearances.
 * Usage: node scripts/check-themes.mjs (CHECK_URL defaults to http://localhost:1448).
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

/** What Settings lists, in order: the saved name, then the name a person reads. */
const THEMES = [
  ["aura", "Main"],
  ["claude", "Claude Code"],
  ["codex", "Codex"],
  ["grok", "Grok Build"],
  ["pi", "Pi"],
  ["antigravity", "Antigravity"],
  ["cursor", "Cursor"],
  ["opencode", "OpenCode"],
  ["oh_my_pi", "oh-my-pi"],
  ["hermes", "Hermes"],
];

/** Where each chat preview has drawn its busiest surface. */
const VIEWS = {
  plan: (page) => page.getByRole("button", { name: "Accept plan" }).waitFor(),
  permission: (page) => page.getByRole("button", { name: /Allow/ }).first().waitFor(),
  // New chat's stage: the message box first, the agent, folder and Chat or Terminal chips under it.
  new: (page) => page.getByRole("textbox", { name: "First message" }).waitFor(),
  // Settings > Agents: the agents joining o7, above their rows.
  "settings&page=agents": (page) => page.getByTestId("agent-connect").first().waitFor(),
};

// A new palette repaints every control; audit once every finite transition has ended.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

export async function checkThemes(browser, url, out, engine) {
  for (const appearance of ["light", "dark"]) {
    const context = await browser.newContext({
      viewport: { width: 1280, height: 900 },
      colorScheme: appearance,
      reducedMotion: "reduce",
    });
    try {
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const palette = async (on = page) =>
        (await on.locator("html").getAttribute("data-palette")) ?? "aura";
      // Settings is a whole page; a chat preview is one pane, audited as check-chat does.
      const audit = async (label, tags, on = page) => {
        await settled(on);
        const builder = new AxeBuilder({ page: on });
        const { violations } = await (tags ? builder.withTags(tags) : builder).analyze();
        assert.deepEqual(
          violations.map(
            (violation) =>
              `${violation.id}: ${violation.nodes.map((node) => node.target.join(" ")).join(", ")}`,
          ),
          [],
          `${engine} ${appearance} ${label}`,
        );
      };

      await page.goto(`${url}/?preview=settings&theme=system&page=appearance`);
      const gallery = page.getByTestId("settings-themes");
      // Loaded: the gallery is disabled until the saved preferences arrive.
      const loaded = () =>
        page.waitForFunction(
          () => document.querySelector('[data-testid="settings-themes"]')?.disabled === false,
        );
      await loaded();
      assert.equal(await page.locator("html").getAttribute("data-theme"), appearance);
      const radio = (name) => gallery.getByRole("radio", { name, exact: true });
      // The gallery shows a choice once React has rendered it, which can be a frame later.
      const chosen = (id) =>
        page.waitForFunction(
          (want) =>
            document.querySelector(`[data-testid="settings-themes"] input[value="${want}"]`)
              ?.checked === true,
          id,
        );
      assert.deepEqual(
        await gallery.getByRole("radio").evaluateAll((items) => items.map((item) => item.value)),
        THEMES.map(([id]) => id),
      );
      // Each choice is named by its theme alone, never by its mark's "Claude Code, Idle".
      for (const [id, name] of THEMES) assert.equal(await radio(name).getAttribute("value"), id);
      // A fresh profile starts on the default, Main (owner, 25 November 2026): the base palette,
      // saved as "aura", which names no palette of its own.
      await chosen("aura");
      assert.equal(await page.locator("html").getAttribute("data-palette"), null);
      await gallery.scrollIntoViewIfNeeded();
      await settled(page);
      await gallery.screenshot({ path: resolve(out, `${engine}-${appearance}-gallery.png`) });

      // A click applies the theme at once, and the page stays accessible in every one.
      for (const [id] of THEMES) {
        await gallery.getByTestId(`theme-${id}`).click();
        await page.waitForFunction(
          (want) => (document.documentElement.dataset.palette ?? "aura") === want,
          id,
        );
        await chosen(id);
        await audit(`settings ${id}`);
      }
      await settled(page);
      await gallery.screenshot({
        path: resolve(out, `${engine}-${appearance}-gallery-hermes.png`),
      });

      // Arrow keys run through the themes, each painting at once with focus kept on it, even
      // while a slow save is in flight; the last one chosen is the one saved.
      const saved = () => page.evaluate(() => window.settingsFixture.theme());
      const focused = (name) => radio(name).evaluate((item) => item === document.activeElement);
      await page.evaluate(() => sessionStorage.setItem("settings-slow-save", "yes"));
      await radio("Hermes").focus();
      for (const [id, name] of [
        ["oh_my_pi", "oh-my-pi"],
        ["opencode", "OpenCode"],
        ["cursor", "Cursor"],
      ]) {
        await page.keyboard.press("ArrowLeft");
        assert.equal(await palette(), id);
        await chosen(id);
        assert.equal(await focused(name), true, `focus stays on ${name}`);
      }
      await page.waitForFunction(() => window.settingsFixture.theme() === "cursor");
      await settled(page);
      assert.equal(await palette(), "cursor");
      assert.equal(await focused("Cursor"), true);
      await page.evaluate(() => sessionStorage.removeItem("settings-slow-save"));

      // The choice survives a reload, as the appearance does.
      await page.reload();
      await loaded();
      assert.equal(await palette(), "cursor");
      await chosen("cursor");

      // A save that fails returns to the saved theme, beside the error.
      await page.evaluate(() => sessionStorage.setItem("settings-fail-save", "yes"));
      await gallery.getByTestId("theme-grok").click();
      await page.getByRole("alert").filter({ hasText: "Preferences could not be saved" }).waitFor();
      assert.equal(await palette(), "cursor");
      await chosen("cursor");
      assert.equal(await saved(), "cursor");
      await page.evaluate(() => sessionStorage.removeItem("settings-fail-save"));

      // Back to Main clears the palette and the error.
      await gallery.getByTestId("theme-aura").click();
      await page.waitForFunction(() => document.documentElement.dataset.palette === undefined);
      await chosen("aura");
      await page.getByRole("alert").waitFor({ state: "hidden" });
      assert.equal(await saved(), "aura");

      // Every theme over the chat's busiest surfaces (a plan to review and an approval), New chat's
      // stage and Settings > Agents. Each theme gets a fresh page: one page loading every view's
      // modules from the dev server 80 times ran Chromium out of resources
      // (net::ERR_INSUFFICIENT_RESOURCES) and drew nothing.
      await page.close();
      for (const [id] of THEMES) {
        const themed = await context.newPage();
        themed.on("pageerror", (error) => errors.push(error.message));
        try {
          for (const [view, ready] of Object.entries(VIEWS)) {
            await themed.goto(`${url}/?preview=${view}&theme=${appearance}&palette=${id}`);
            await ready(themed);
            assert.equal(await palette(themed), id, `${view} paints ${id}`);
            await audit(`${view} ${id}`, ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"], themed);
            if (view === "plan" && (id === "claude" || id === "grok"))
              await themed.screenshot({
                path: resolve(out, `${engine}-${appearance}-plan-${id}.png`),
              });
          }
        } finally {
          await themed.close();
        }
      }
      assert.deepEqual(errors, []);
    } finally {
      await context.close();
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/themes-browser/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkThemes(browser, process.env.CHECK_URL ?? "http://localhost:1448", out, engine);
      console.log(
        `${engine}: the theme gallery, keyboard, persistence and every theme's accessibility passed in both appearances`,
      );
    } finally {
      await browser.close();
    }
  }
}
