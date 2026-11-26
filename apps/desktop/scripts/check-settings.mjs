/** Synthetic command responses exercise production settings without touching native state. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";
import { checkAutomaticCheckpoints } from "./check-automatic-checkpoints.mjs";
import { checkGitReview } from "./check-git-review.mjs";
import { checkNotices } from "./check-notices.mjs";
import { checkProfiles } from "./check-profiles.mjs";
import { checkProjectTools } from "./check-project-tools.mjs";
import { checkQuestions } from "./check-questions.mjs";
import { checkRemote } from "./check-remote.mjs";
import { checkSessionFallback } from "./check-session-fallback.mjs";
import { checkSettingsNavigation } from "./check-settings-navigation.mjs";
import { checkSetupRecovery } from "./check-setup-recovery.mjs";
import { checkSteering } from "./check-steering.mjs";
import { checkThemes } from "./check-themes.mjs";
import { checkWorktreeSetup } from "./check-worktree-setup.mjs";
import { choose, chosen, labels } from "./choose.mjs";

// Sheets fade in and controls settle their colours; audit only once every finite animation ends.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// `[server URL] [--engines chromium,webkit]`: CI runs each engine as its own step.
const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: { engines: { type: "string", default: "chromium,webkit" } },
});
const launchers = Object.entries({ chromium, webkit }).filter(([engine]) =>
  values.engines.split(",").includes(engine),
);
assert.ok(launchers.length > 0, `No supported engine in --engines ${values.engines}`);
const url = positionals[0] ?? "http://127.0.0.1:1464";
const out = resolve(app, "../../target/settings-browser");
const require = createRequire(import.meta.url);
const server = positionals[0]
  ? null
  : spawn(
      process.execPath,
      [
        resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js"),
        "--host",
        "127.0.0.1",
        "--port",
        "1464",
        "--strictPort",
      ],
      { cwd: app, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
    );
let output = "";
if (server)
  for (const stream of [server.stdout, server.stderr])
    stream.on("data", (data) => {
      output = (output + data).slice(-8192);
    });
try {
  await mkdir(out, { recursive: true });
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (server?.exitCode != null) throw new Error(output);
    try {
      ready = (await fetch(url)).ok;
    } catch {
      /* The owned server is still starting. */
    }
    if (ready) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.ok(ready, output || "The preview server did not start.");
  for (const [engine, launcher] of launchers) {
    const browser = await launcher.launch({ headless: true });
    try {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 900 },
        reducedMotion: "reduce",
      });
      await context.route("**/*", (route) =>
        new URL(route.request().url()).origin === new URL(url).origin
          ? route.continue()
          : route.abort(),
      );
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      for (const theme of ["light", "dark"]) {
        await page.emulateMedia({ colorScheme: theme });
        await page.goto(`${url}/?preview=settings&theme=system&page=chats`);
        await page.getByRole("switch", { name: "Automatic checkpoints", exact: true }).waitFor();
        assert.equal(await page.locator("html").getAttribute("data-theme"), theme);
        await checkAutomaticCheckpoints(page, out, `${engine}-${theme}`);
      }
      // Files open in the first editor detected until another is chosen; the choice saves.
      const editor = page.getByLabel("Open files in");
      await editor.waitFor();
      assert.equal(await chosen(editor), "cursor");
      assert.deepEqual(await labels(editor), ["Cursor", "Zed"]);
      await choose(editor, "zed");
      await page.waitForFunction(() => window.settingsFixture.general().editor === "zed");
      assert.equal(await chosen(editor), "zed");
      await checkSettingsNavigation(page, url);
      // Appearance, motion and text size apply as they are chosen and save in turn.
      await page.goto(`${url}/?preview=settings&page=appearance`);
      await page.getByRole("radio", { name: "Reduced", exact: true }).check();
      assert.equal(await page.evaluate(() => window.settingsFixture.motion()), "reduced");
      await page.evaluate(() => sessionStorage.setItem("settings-fail-save", "yes"));
      await page.getByRole("radio", { name: "Off", exact: true }).click();
      await page.getByRole("alert").filter({ hasText: "Preferences could not be saved" }).waitFor();
      assert.equal(
        await page.getByRole("radio", { name: "Reduced", exact: true }).isChecked(),
        true,
      );
      await page.evaluate(() => sessionStorage.removeItem("settings-fail-save"));
      await page.getByRole("button", { name: "Reload preferences" }).click();
      await page.getByRole("alert").waitFor({ state: "hidden" });
      // Text size sets the chat at once, like a theme, keeps its focus through a slow save, saves
      // the latest choice and returns to the saved size when a save fails (the Rust onboarding
      // tests cover keeping it across restarts; this fixture holds preferences in memory).
      const textSize = () => page.evaluate(() => window.settingsFixture.textSize());
      const sizeAttribute = () =>
        page.evaluate(() => document.documentElement.dataset.textSize ?? "compact");
      const sizes = page.getByTestId("settings-text-size");
      assert.equal(await textSize(), "compact");
      await page.evaluate(() => sessionStorage.setItem("settings-slow-save", "yes"));
      await sizes.getByRole("radio", { name: "Comfortable", exact: true }).check();
      assert.equal(
        await sizeAttribute(),
        "comfortable",
        "Comfortable applies before its save ends",
      );
      // WebKit does not focus a radio it clicks, as Safari does not; arrow keys start from focus.
      await sizes.getByRole("radio", { name: "Comfortable", exact: true }).focus();
      await page.keyboard.press("ArrowLeft");
      assert.equal(await sizeAttribute(), "compact");
      assert.equal(
        await sizes
          .getByRole("radio", { name: "Compact", exact: true })
          .evaluate((node) => node === document.activeElement),
        true,
        "focus stays on the text size while it saves",
      );
      await page.keyboard.press("ArrowRight");
      await page.waitForFunction(() => window.settingsFixture.textSize() === "comfortable");
      await settled(page);
      assert.equal(await sizeAttribute(), "comfortable");
      await page.evaluate(() => sessionStorage.removeItem("settings-slow-save"));
      await page.evaluate(() => sessionStorage.setItem("settings-fail-save", "yes"));
      await sizes.getByRole("radio", { name: "Compact", exact: true }).click();
      await page.getByRole("alert").filter({ hasText: "Preferences could not be saved" }).waitFor();
      await page.waitForFunction(() => document.documentElement.dataset.textSize === "comfortable");
      assert.equal(await textSize(), "comfortable");
      // A failed save returns the radio to the saved size once React has rendered the rollback.
      await page.waitForFunction(
        () =>
          document.querySelector('[data-testid="settings-text-size"] input[value="comfortable"]')
            ?.checked === true,
      );
      await page.evaluate(() => sessionStorage.removeItem("settings-fail-save"));
      await page.getByRole("button", { name: "Reload preferences" }).click();
      await page.getByRole("alert").waitFor({ state: "hidden" });
      await sizes.getByRole("radio", { name: "Compact", exact: true }).check();
      await page.waitForFunction(() => window.settingsFixture.textSize() === "compact");
      assert.equal(await sizeAttribute(), "compact");
      // General's defaults for new chats and the background switches save at once.
      await page
        .getByRole("navigation", { name: "Settings sections" })
        .getByRole("button", { name: "General", exact: true })
        .click();
      const general = () => page.evaluate(() => window.settingsFixture.general());
      assert.equal(await page.getByLabel("Default model").count(), 0);
      await choose(page.getByLabel("Default agent"), "codex");
      // A thinking level for the agent's own default model; another model clears it.
      await choose(page.getByLabel("Default thinking level"), "high");
      await page.waitForFunction(() => window.settingsFixture.general().default_effort === "high");
      await choose(page.getByLabel("Default model"), "gpt-5.5-codex");
      await page
        .getByText("Codex lists this model's thinking levels once it has run it.")
        .waitFor();
      assert.equal(await page.getByLabel("Default thinking level").count(), 0);
      // Every control stays enabled while a save runs, so arrow keys keep their focus: the
      // owner's radio focus loss (4 November 2026). The latest choice is the one saved.
      await page.evaluate(() => sessionStorage.setItem("settings-slow-save", "yes"));
      const autonomy = (name) => page.getByRole("radio", { name, exact: true });
      await autonomy("Full auto").focus();
      await page.keyboard.press("ArrowLeft");
      await page.keyboard.press("ArrowLeft");
      assert.equal(
        await autonomy("Ask").evaluate((node) => node === document.activeElement && node.checked),
        true,
        "focus and the choice stay on the radio while it saves",
      );
      assert.equal(await autonomy("Ask").isDisabled(), false);
      await page.waitForFunction(() => window.settingsFixture.general().default_autonomy === "ask");
      await page.evaluate(() => sessionStorage.removeItem("settings-slow-save"));
      await page.getByRole("switch", { name: "Notifications" }).uncheck();
      await page.getByRole("switch", { name: "Keep awake" }).uncheck();
      await page.getByRole("switch", { name: "Keep running in the tray" }).uncheck();
      await page.waitForFunction(() => !window.settingsFixture.general().close_to_tray);
      // Looking for a newer Ghost is on until turned off, and the switch saves at once.
      const automatic = page.getByRole("switch", { name: "Check for updates automatically" });
      assert.equal(await automatic.isChecked(), true);
      await automatic.uncheck();
      await page.waitForFunction(() => !window.settingsFixture.general().automatic_updates);
      assert.deepEqual(await general(), {
        default_agent: "codex",
        default_model: "gpt-5.5-codex",
        default_effort: null,
        default_autonomy: "ask",
        automatic_checkpoints: false,
        notifications: false,
        sounds: false,
        keep_awake: false,
        close_to_tray: false,
        copy_instructions_to_worktrees: true,
        agent_memory_writes: true,
        keep_steps_open: false,
        editor: null,
        browser: true,
        browser_passwords: true,
        mobile: true,
        automatic_updates: false,
        coordinated_projects: false,
        speech_voice: null,
      });
      // Sounds are off until turned on; turning them on plays one and saves the choice.
      await page.getByRole("switch", { name: "Sounds" }).check();
      await page.waitForFunction(() => window.settingsFixture.general().sounds);
      // Another agent clears the model, which belonged to Codex; it has not listed its own.
      await choose(page.getByLabel("Default agent"), "claude");
      await page.getByText("Claude Code lists its models once one of its chats starts.").waitFor();
      assert.equal(await page.getByLabel("Default model").count(), 0);
      assert.equal((await general()).default_model, null);
      await page.getByRole("radio", { name: "Full auto", exact: true }).check();
      for (const name of ["Notifications", "Keep awake", "Keep running in the tray"])
        await page.getByRole("switch", { name }).check();
      await page.waitForFunction(() => window.settingsFixture.general().close_to_tray);
      await page.locator(".settings-main").evaluate((element) => {
        element.scrollTop = 0;
      });
      await settled(page);
      assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
      await page.screenshot({ path: resolve(out, `${engine}-general.png`) });
      await page
        .getByRole("navigation", { name: "Settings sections" })
        .getByRole("button", { name: "Shortcuts", exact: true })
        .click();
      // Settings moves focus to the new heading in the next animation frame. Wait for the heading
      // itself: in Chromium the clicked "Shortcuts" button holds focus until then.
      await page.waitForFunction(
        () =>
          /^H[1-6]$/.test(document.activeElement?.tagName ?? "") &&
          document.activeElement?.textContent === "Shortcuts",
        null,
        { timeout: 5000 },
      );
      assert.equal(
        await page
          .getByRole("heading", { name: "Shortcuts", exact: true })
          .evaluate((node) => node === document.activeElement),
        true,
      );
      await settled(page);
      assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
      await page
        .getByRole("navigation", { name: "Settings sections" })
        .getByRole("button", { name: "About", exact: true })
        .click();
      await page.getByRole("button", { name: "Check for updates" }).waitFor();
      assert.deepEqual(
        await page.evaluate(() =>
          window.settingsFixture
            .calls()
            .filter((call) => ["check", "download", "install"].includes(call)),
        ),
        [],
      );
      await page.getByRole("button", { name: "Check for updates" }).click();
      await page.getByRole("button", { name: "Download", exact: true }).click();
      await page.getByRole("button", { name: "Install update", exact: true }).waitFor();
      await page.getByRole("button", { name: "Install update", exact: true }).click();
      assert.equal(
        await page.evaluate(() => window.settingsFixture.calls().includes("install")),
        false,
      );
      await page.getByRole("button", { name: "Keep working", exact: true }).click();
      await page.getByRole("button", { name: "Install update", exact: true }).click();
      await page.getByRole("button", { name: "Install and restart", exact: true }).click();
      await page.getByText("Waiting for your work to finish", { exact: true }).waitFor();
      assert.deepEqual(
        await page.evaluate(() =>
          window.settingsFixture
            .calls()
            .filter((call) => ["check", "download", "install"].includes(call)),
        ),
        ["check", "download", "install"],
      );
      await settled(page);
      assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
      await page.screenshot({ path: resolve(out, `${engine}-update.png`) });
      await page.keyboard.press("Escape");
      await page.getByRole("heading", { name: "Back to workspace" }).waitFor();
      // Linux without GStreamer's plugins: Sounds says why, once, and its switch waits.
      await page.goto(`${url}/?preview=settings&media=off`);
      const sounds = page.getByRole("switch", { name: "Sounds" });
      await sounds.waitFor();
      assert.equal(await sounds.isDisabled(), true, `${engine}: Sounds waits where none can play`);
      const why =
        "Audio and video are off here: GStreamer's base and good plugins are missing. Install gst-plugins-base and gst-plugins-good, then restart Ghost.";
      await page.getByText(why, { exact: true }).waitFor();
      assert.equal(await page.getByText(why, { exact: true }).count(), 1, `${engine}: said once`);
      // Every switch waits while the saved preferences load; Notifications is free once they have.
      await page.waitForFunction(() => {
        const node = document.querySelector('[data-testid="settings-notifications"]');
        return node && !node.disabled && node.getAttribute("aria-disabled") !== "true";
      });
      assert.equal(await page.getByRole("switch", { name: "Notifications" }).isDisabled(), false);
      await settled(page);
      assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
      await page.screenshot({ path: resolve(out, `${engine}-sounds-off.png`) });
      await page.goto(`${url}/?preview=settings&unsigned`);
      await page.getByRole("button", { name: "About", exact: true }).click();
      await page.getByText("Manual installation", { exact: true }).waitFor();
      assert.equal(await page.getByRole("button", { name: "Check for updates" }).count(), 0);
      await page.goto(`${url}/?preview=settings&staging`);
      await page.getByRole("button", { name: "About", exact: true }).click();
      await page
        .getByText("Staging build. Updates are restricted to release qualification builds.", {
          exact: true,
        })
        .waitFor();
      await checkNotices(page, out, engine);
      await checkGitReview(page, url, out, engine);
      await checkProjectTools(page, url, out, engine);
      await checkProfiles(browser, url, out, engine);
      await checkWorktreeSetup(browser, url, out, engine);
      await checkSetupRecovery(browser, url);
      await checkSteering(browser, url, out, engine);
      await checkQuestions(browser, url, out, engine);
      await checkSessionFallback(browser, url, out, engine);
      await checkThemes(browser, url, out, engine);
      await checkRemote(browser, url, out, engine);
      assert.deepEqual(errors, []);
      console.log(
        `${engine}: settings persistence failures, focus, accessibility and explicit updater actions passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  server?.kill();
}
