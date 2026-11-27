/** Production composer and new-chat components with synthetic session data. No agents run. */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";
import { choose, chosen } from "./choose.mjs";

const url = process.argv[2] ?? "http://127.0.0.1:1474";
const out = resolve("target/customization-browser");
await mkdir(out, { recursive: true });
for (const [engine, launcher] of Object.entries({ chromium, webkit })) {
  const browser = await launcher.launch({ headless: true });
  try {
    for (const theme of ["light", "dark"]) {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 900 },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      const page = await context.newPage();
      page.setDefaultTimeout(15000);
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const capture = async (name) => {
        await page.evaluate(() => document.fonts.ready);
        const violations = (
          await new AxeBuilder({ page })
            .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
            .analyze()
        ).violations;
        assert.deepEqual(
          violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
          [],
          `${engine}/${theme}/${name}`,
        );
        assert.equal(
          await page.evaluate(() => document.documentElement.scrollWidth > innerWidth),
          false,
          "Horizontal overflow",
        );
        await page.screenshot({ path: resolve(out, `${engine}-${theme}-${name}.png`) });
      };
      await page.goto(`${url}/?preview=settled`);
      await page.getByTestId("composer-input").waitFor();
      const access = page.locator(".chat-access-popover > summary");
      const model = page.locator(".chat-model-popover > summary");
      assert.ok(
        (await access.boundingBox()).x < (await model.boundingBox()).x,
        "Access must precede model",
      );
      await capture("composer");
      await access.click();
      await page.getByTestId("picker-access-choice").filter({ hasText: "Full access" }).focus();
      await page.keyboard.press("Enter");
      await page.getByLabel("Full access. Access level", { exact: true }).waitFor();
      assert.equal(
        await access.evaluate((node) => node === document.activeElement),
        true,
        "Keyboard access selection must return focus to its trigger",
      );
      await page.locator(".chat-workflow-popover > summary").click();
      await page
        .getByRole("button", { name: "Plan Review the work before implementation." })
        .focus();
      await page.keyboard.press("Enter");
      await page.getByLabel("Plan. Working mode", { exact: true }).waitFor();
      assert.equal(
        await page
          .locator(".chat-workflow-popover > summary")
          .evaluate((node) => node === document.activeElement),
        true,
        "Keyboard mode selection must return focus to its trigger",
      );
      await page.locator(".chat-workflow-popover > summary").click();
      await capture("plan");
      await page.getByRole("button", { name: /Approve and execute/ }).click();
      await page.getByLabel("Execute. Working mode", { exact: true }).waitFor();
      await model.click();
      await page.getByRole("searchbox", { name: "Search agents and models" }).fill("Haiku");
      await page.locator('[data-picker-value="haiku"]').waitFor();
      await capture("model-search");
      await page.keyboard.press("Escape");
      assert.equal(await model.evaluate((node) => node === document.activeElement), true);
      await page.goto(`${url}/?preview=new-worktree`);
      await page.getByTestId("new-chat-create").waitFor();
      await capture("new-chat");
      await page.locator(".new-chat-agent-picker > summary").click();
      await page.getByRole("searchbox", { name: "Search agents" }).fill("Claude");
      await capture("agent-search");
      await page.keyboard.press("Escape");
      await page.setViewportSize({ width: 960, height: 640 });
      await capture("new-chat-960");
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(`${url}/?preview=project-chat`);
      await page.getByRole("button", { name: "New chat in auracode", exact: true }).click();
      await page.getByRole("heading", { name: "New chat in auracode", level: 1 }).waitFor();
      await page.getByLabel("Claude Code. Choose agent", { exact: true }).waitFor();
      await capture("project-new-chat");
      await page.getByTestId("new-chat-pick-folder").click();
      await page.getByRole("button", { name: "Choose folder", exact: true }).waitFor();
      assert.equal(
        await page.getByTestId("new-chat-create").isEnabled(),
        true,
        "Cancelling the folder picker must retain the project prefill",
      );
      await page.getByTestId("new-chat-create").click();
      await page
        .getByRole("status")
        .filter({ hasText: "Chat created in the project folder." })
        .waitFor();
      // Customize is a shortcut into Settings, on Plugins & MCP.
      await page.getByRole("button", { name: "Customize", exact: true }).click();
      await page.getByRole("heading", { name: "Plugins & MCP", exact: true, level: 1 }).waitFor();
      await page.locator(".plugins-agent").waitFor();
      await capture("customize-plugins");
      const tabs = page.getByRole("navigation", { name: "Settings sections" });
      await tabs.getByRole("button", { name: "Project memory", exact: true }).click();
      const memory = page.getByRole("textbox", { name: "Markdown", exact: true });
      await memory.fill("# Unsaved project note\n\nKeep this draft when changing Settings pages.");
      await tabs.getByRole("button", { name: "Instructions", exact: true }).click();
      await page
        .getByRole("heading", { name: "How instructions are shared", exact: true })
        .waitFor();
      const scope = page.getByLabel("Applies to", { exact: true });
      await choose(scope, "global");
      await page.evaluate(() => window.customizeFixture.refresh());
      assert.equal(await chosen(scope), "global", "Workspace refresh must retain the chosen scope");
      await capture("customize-instructions");
      await tabs.getByRole("button", { name: "Project memory", exact: true }).click();
      assert.match(await memory.inputValue(), /Keep this draft/);
      await capture("customize-memory");
      await page.setViewportSize({ width: 960, height: 640 });
      await capture("customize-memory-960");
      assert.deepEqual(errors, []);
      await context.close();
    }
    console.log(
      `${engine}: access, native Plan approval, model search, new chat, both themes and narrow-window accessibility passed`,
    );
  } finally {
    await browser.close();
  }
}
