/**
 * The Instructions page in Chromium and WebKit with synthetic native replies (dev/instructionsFixture),
 * never real files: every flow reviews before it writes, Escape and Cancel discard a review, and
 * each state passes axe with no violations.
 *
 *   node scripts/check-instructions.mjs [--server URL] [--engines chromium,webkit]
 *
 * Without --server it starts its own Vite on 127.0.0.1:1426 (strictPort) and stops it after.
 * Screenshots go to target/instructions-browser/<engine>-<state>.png.
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";

const { values } = parseArgs({
  options: {
    server: { type: "string" },
    engines: { type: "string", default: "chromium,webkit" },
  },
});

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = resolve(app, "../../target/instructions-browser");
const require = createRequire(import.meta.url);

// Dialogs fade in and controls settle their colours; audit only once every finite animation ends.
const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

let server = null;
let output = "";
const url = values.server ?? "http://127.0.0.1:1426";
if (!values.server) {
  const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
  server = spawn(
    process.execPath,
    [vite, "--host", "127.0.0.1", "--port", "1426", "--strictPort"],
    {
      cwd: app,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  for (const stream of [server.stdout, server.stderr])
    stream.on("data", (data) => {
      output = (output + data).slice(-8192);
    });
}

let audits = 0;
try {
  await mkdir(out, { recursive: true });
  let ready = false;
  for (let attempt = 0; attempt < 150; attempt++) {
    // Our own server must be the one answering: another worktree's Vite may hold the port.
    if (server && server.exitCode !== null)
      throw new Error(
        /in use/i.test(output)
          ? "Port 1426 is in use by another dev server. Wait for it to stop and run again."
          : output,
      );
    if (server && !/ready in/i.test(output)) {
      await new Promise((done) => setTimeout(done, 100));
      continue;
    }
    try {
      ready = (await fetch(`${url}/?preview=instructions`)).ok;
    } catch {
      /* The server is starting. */
    }
    if (ready) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.ok(ready, `The dev server did not answer at ${url}. ${output}`);
  const launchers = { chromium, webkit };
  for (const engine of values.engines.split(",")) {
    const browser = await launchers[engine].launch({ headless: true });
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
      const calls = () => page.evaluate(() => window.instructionsFixture.calls());
      const audit = async (state) => {
        await settled(page);
        await page.screenshot({ path: resolve(out, `${engine}-${state}.png`) });
        const { violations } = await new AxeBuilder({ page }).analyze();
        assert.deepEqual(
          violations.map(
            (violation) =>
              `${violation.id}: ${violation.nodes.map((node) => node.target).join(", ")}`,
          ),
          [],
          `${engine} ${state} accessibility`,
        );
        audits++;
      };
      const open = async (mode) => {
        await page.goto(`${url}/?preview=${mode}`);
        await page.getByTestId("instructions").waitFor();
        await page.locator(".instructions-file, .instructions-empty").first().waitFor();
      };
      const dialog = (name) => page.getByRole("dialog", { name });

      await open("instructions-recovery");
      await page.getByText(/An interrupted Instructions change needs recovery/).waitFor();
      await page
        .getByText(/Its backup is kept at C:\/fixture\/auracode\/instructions\/backups\/recovered/)
        .waitFor();
      assert.ok((await calls()).every((call) => call === "snapshot"));
      await audit("recovery");
      await page.getByLabel("Text of AGENTS.md").fill("A new edit.");
      await page.getByRole("button", { name: "Review changes", exact: true }).click();
      await dialog("Save AGENTS.md").waitFor();
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      await page
        .getByRole("dialog")
        .getByText(
          /External edits were preserved.*Backups and recovery details are kept at C:\/fixture/,
        )
        .waitFor();
      await audit("recovery-blocked");
      await page.keyboard.press("Escape");

      // One file: an edit is reviewed with its diff, Escape discards the review, Apply saves it.
      await open("instructions");
      // Opening the page only reads (StrictMode reads twice in development).
      assert.ok(
        (await calls()).every((call) => call === "snapshot"),
        "opening only reads",
      );
      assert.equal(
        await page
          .getByLabel("Text of AGENTS.md")
          .inputValue()
          .then((v) => v.startsWith("# Ghost")),
        true,
      );
      const review = page.getByRole("button", { name: "Review changes", exact: true });
      assert.equal(await review.isDisabled(), true, "nothing to review before an edit");
      await audit("universal");
      await page.getByLabel("Text of AGENTS.md").fill("# auracode\n\nUse pnpm.\n");
      await review.click();
      await dialog("Save AGENTS.md").waitFor();
      await page.getByTestId("diff-block").first().waitFor();
      assert.equal((await calls()).includes("apply"), false, "a review never writes");
      await audit("review-save");
      await page.keyboard.press("Escape");
      await page.waitForFunction(() => window.instructionsFixture.calls().includes("cancel"));
      assert.equal(await page.getByRole("dialog").count(), 0);
      assert.equal(
        await page.getByLabel("Text of AGENTS.md").inputValue(),
        "# auracode\n\nUse pnpm.\n",
        "the draft survives a cancelled review",
      );
      await review.click();
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      await page.getByRole("status").filter({ hasText: "Saved AGENTS.md" }).waitFor();
      assert.equal((await calls()).filter((call) => call === "apply").length, 1);
      assert.equal(await page.getByRole("dialog").count(), 0);
      // Switching to a file per agent is reviewed too.
      await page.getByRole("radio", { name: "Agent-specific files" }).click();
      await dialog("Use a file per agent").waitFor();
      await page.getByRole("button", { name: "Cancel", exact: true }).click();
      assert.equal(
        await page.getByRole("radio", { name: "Shared instructions" }).isChecked(),
        true,
        "cancelling keeps the mode",
      );
      // New worktree chats get the files Git ignores unless the switch is turned off.
      const copies = page.getByRole("switch", {
        name: "Copy instruction files into new worktree chats",
      });
      await page.waitForFunction(
        () => !document.querySelector('[data-testid="instructions-worktree-copies"]')?.disabled,
      );
      assert.equal(await copies.isChecked(), true, "worktree copies are on by default");
      await page.getByText(/are copied into each new worktree chat/).waitFor();
      await copies.click();
      await page
        .getByText("New worktree chats get only the instruction files committed to Git.", {
          exact: true,
        })
        .waitFor();
      assert.equal(await copies.isChecked(), false, "the switch saves off");
      await copies.click();
      await page.getByText(/are copied into each new worktree chat/).waitFor();
      assert.equal(await copies.isChecked(), true, "and on again");

      // Per agent: files as tabs; an untracked file joins new worktree chats or is committed alone.
      await open("instructions-per-agent");
      await page.getByRole("tab", { name: "CLAUDE.md" }).click();
      await page.getByLabel("Text of CLAUDE.md").waitFor();
      await audit("per-agent");
      await page.getByRole("button", { name: "Copy into new worktree chats", exact: true }).click();
      await dialog("Copy CLAUDE.md into new worktree chats?").waitFor();
      assert.equal((await calls()).includes("include"), false);
      await audit("include");
      await page.getByRole("button", { name: "Copy into new chats", exact: true }).click();
      await page
        .getByRole("status")
        .filter({ hasText: "New worktree chats copy CLAUDE.md" })
        .waitFor();
      await page.getByRole("button", { name: "Commit", exact: true }).click();
      await dialog("Commit CLAUDE.md?").waitFor();
      await page.getByRole("button", { name: "Commit", exact: true }).last().click();
      await page
        .getByRole("status")
        .filter({ hasText: "Committed CLAUDE.md as 4f2a91c." })
        .waitFor();
      assert.deepEqual(
        (await calls()).filter((call) => call === "include" || call === "commit"),
        ["include", "commit"],
      );

      // Different texts: nothing is reviewed until a choice; overrides go only when ticked.
      await open("instructions-conflict");
      await dialog("Choose the shared text").waitFor();
      assert.equal(
        (await calls()).some((call) => call.startsWith("review")),
        false,
      );
      assert.equal(
        await page.getByRole("button", { name: "Apply", exact: true }).isDisabled(),
        true,
      );
      await audit("conflict");
      await page.getByText("Combine them", { exact: true }).click();
      await dialog("Use one file for every agent").waitFor();
      await page.getByText("AGENTS.override.md", { exact: true }).last().waitFor();
      assert.equal(
        await page.locator('[data-action="remove"]').count(),
        0,
        "keeping the override is the default",
      );
      await audit("conflict-review");
      await page.getByRole("checkbox", { name: "AGENTS.override.md" }).check();
      await page.locator('[data-action="remove"]').waitFor();
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      await page.getByRole("status").filter({ hasText: "Saved" }).waitFor();
      await page.getByRole("radio", { name: "Shared instructions" }).isChecked();
      assert.equal(
        await page.getByRole("radio", { name: "Shared instructions" }).isChecked(),
        true,
      );

      // A hand-edited managed copy: restore it after an explicit choice.
      await open("instructions-drift");
      await page.getByText("Edited copy for Pi").waitFor();
      await audit("drift");
      await page.getByRole("button", { name: "Resolve", exact: true }).click();
      await dialog("~/.pi/agent/AGENTS.md was edited by hand").waitFor();
      await page.getByText("Restore the text of ~/.codex/AGENTS.md", { exact: true }).click();
      await dialog("Restore ~/.pi/agent/AGENTS.md").waitFor();
      await page.getByRole("button", { name: "Apply", exact: true }).click();
      await page.getByRole("status").filter({ hasText: "Saved" }).waitFor();
      assert.equal(await page.getByText("Edited copy for Pi").count(), 0);
      // An agent installed later reads nothing yet; Set up reviews one file again, nothing more.
      const before = (await calls()).length;
      await page.getByRole("button", { name: "Set up Grok Build", exact: true }).click();
      await dialog("Use one file for every agent").waitFor();
      // StrictMode asks twice in development and cancels the stale review; nothing is applied.
      const since = (await calls()).slice(before);
      assert.ok(since.includes("review:universal:source"), since.join(", "));
      assert.equal(since.includes("apply"), false);
      await page.getByRole("button", { name: "Cancel", exact: true }).click();
      await dialog("Use one file for every agent").waitFor({ state: "detached" });
      const after = (await calls()).slice(before);
      assert.ok(after.includes("cancel") && !after.includes("apply"), after.join(", "));

      // Size limits show beside the text, per agent that reads it.
      await open("instructions-large");
      await page.getByText("Codex reads the first 32 KiB.", { exact: false }).first().waitFor();
      await audit("large");

      // The review preview opens on load.
      await open("instructions-review");
      await dialog("Save AGENTS.md").waitFor();
      await page.getByTestId("diff-block").first().waitFor();
      await audit("review");

      assert.deepEqual(errors, [], `${engine} page errors`);
      console.log(`${engine}: Instructions flows passed`);
    } finally {
      await browser.close();
    }
  }
  console.log(`check-instructions: ${audits} axe scans, 0 violations`);
} finally {
  server?.kill();
}
