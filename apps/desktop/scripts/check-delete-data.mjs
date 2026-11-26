/**
 * Settings' "Delete all Ghost data" in Chromium and WebKit on synthetic reviews
 * (dev/deleteDataPreview): the review, the worktree warning, the macOS Trash choice and a failed
 * start. No file, key, worktree or process is touched. Every state passes axe with no violations.
 *
 *   node scripts/check-delete-data.mjs [--server URL] [--engines chromium,webkit]
 *
 * Without --server it starts its own Vite on 127.0.0.1:1426 (strictPort) and stops it after.
 * Screenshots go to target/delete-data-browser/<engine>-<state>.png.
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
const out = resolve(app, "../../target/delete-data-browser");
const require = createRequire(import.meta.url);

// Dialogs lift in and controls settle their colours; audit only once every finite animation ends.
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
    { cwd: app, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
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
      ready = (await fetch(`${url}/?preview=delete-data`)).ok;
    } catch {
      /* The server is starting. */
    }
    if (ready) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.ok(ready, `The dev server did not answer at ${url}. ${output}`);
  // Windows lets a second server bind the same port; make sure this checkout's page answers.
  const main = await (await fetch(`${url}/src/main.tsx`)).text();
  assert.ok(
    main.includes("deleteDataPreview"),
    "Another checkout's dev server answers on port 1426. Wait for it to stop and run again.",
  );
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
      const calls = () => page.evaluate(() => window.deleteDataFixture.calls());
      const dialog = () => page.getByRole("dialog", { name: "Delete all Ghost data" });
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
        await dialog().waitFor();
        await page.getByTestId("delete-data-confirm").waitFor();
      };
      const apply = () => page.getByTestId("delete-data-apply");

      // The review: what goes, the agent tokens, what stays; the red pill waits for DELETE.
      await open("delete-data");
      const review = dialog();
      await review.getByText("Chats and settings", { exact: true }).waitFor();
      await review.getByText("C:\\Users\\you\\AppData\\Local\\si.auracode.desktop").waitFor();
      await review.getByText("1.2 GB", { exact: true }).waitFor();
      await review.getByText("Update downloads", { exact: true }).waitFor();
      await review
        .getByText(
          "Sign-in tokens of agent accounts you added in Ghost, kept in Windows Credential Manager.",
          { exact: true },
        )
        .waitFor();
      assert.equal(await review.getByRole("list", { name: "Secure-store entries" }).count(), 0);
      await review.getByText(/Stays: your agents and their folders/).waitFor();
      assert.equal(await review.getByTestId("delete-data-worktrees").count(), 0);
      assert.equal(await review.getByText("Also move Ghost to the Trash").count(), 0);
      assert.equal(await apply().isDisabled(), true);
      assert.equal(await apply().textContent(), "Delete data and quit");
      await audit("review");
      // Escape and Cancel close it without applying; the row opens it again.
      await page.keyboard.press("Escape");
      await review.waitFor({ state: "detached" });
      await page.getByTestId("settings-delete-data").click();
      await dialog().getByTestId("delete-data-confirm").fill("delete");
      assert.equal(await apply().isDisabled(), true, "the word is case-sensitive");
      await dialog().getByRole("button", { name: "Cancel", exact: true }).click();
      await dialog().waitFor({ state: "detached" });
      assert.ok(!(await calls()).some((call) => call.startsWith("apply")));
      await page.getByTestId("settings-delete-data").click();
      await dialog().getByLabel("Type DELETE to confirm").fill("DELETE");
      assert.equal(await apply().isDisabled(), false);
      await apply().click();
      await dialog()
        .getByRole("status")
        .getByText(/Ghost is quitting/)
        .waitFor();
      assert.equal(await apply().isDisabled(), true);
      assert.ok(
        (await calls()).includes('apply:preview:{"remove_worktrees":[],"move_app_to_trash":false}'),
        JSON.stringify(await calls()),
      );
      await audit("quitting");

      // Worktrees with unsaved work: named with what they would lose, kept unless ticked.
      await open("delete-data-worktrees");
      const warning = dialog().getByTestId("delete-data-worktrees");
      await warning
        .getByRole("heading", { name: "2 worktrees have work saved nowhere else" })
        .waitFor();
      await warning.getByText("ghost/fix-login-redirect", { exact: true }).waitFor();
      await warning
        .getByText("3 uncommitted files, 1 commit only on ghost/fix-login-redirect", {
          exact: true,
        })
        .waitFor();
      await warning.getByText("2 commits on no branch", { exact: true }).waitFor();
      await dialog()
        .getByText(/^1 worktree with nothing unsaved is deleted too/)
        .waitFor();
      const unsaved = dialog().getByTestId("delete-data-unsaved");
      assert.equal(await unsaved.isChecked(), false);
      await audit("worktrees");
      await unsaved.check();
      await dialog().getByTestId("delete-data-confirm").fill("DELETE");
      await page.keyboard.press("Enter");
      await dialog()
        .getByRole("status")
        .getByText(/Ghost is quitting/)
        .waitFor();
      const applied = (await calls()).find((call) => call.startsWith("apply"));
      assert.match(
        applied ?? "",
        /"remove_worktrees":\["C:\\\\Users.*4f1c2a9b7e01","C:.*9d0b6c3e2f44"\]/,
      );
      assert.doesNotMatch(applied ?? "", /c7e5a1d08b32/, "clean worktrees need no choice");

      // macOS: the Trash is offered and changes the red pill's words.
      await open("delete-data-macos");
      await dialog()
        .getByText(
          "Sign-in tokens of agent accounts you added in Ghost, kept in the macOS keychain.",
          { exact: true },
        )
        .waitFor();
      const trash = dialog().getByLabel("Also move Ghost to the Trash");
      assert.equal(await trash.isChecked(), false);
      await trash.check();
      assert.equal(await apply().textContent(), "Delete data and move to Trash");
      await audit("macos");
      await dialog().getByTestId("delete-data-confirm").fill("DELETE");
      await apply().click();
      await dialog()
        .getByRole("status")
        .getByText(/Ghost is quitting/)
        .waitFor();
      assert.ok((await calls()).some((call) => call.includes('"move_app_to_trash":true')));

      // A start that fails says so, uses up the review, and offers a fresh one.
      await open("delete-data&case=fail");
      await dialog().getByTestId("delete-data-confirm").fill("DELETE");
      await apply().click();
      await dialog()
        .getByRole("alert")
        .getByText(/The cleanup could not start/)
        .waitFor();
      await audit("failed");
      await dialog().getByRole("button", { name: "Review again", exact: true }).click();
      await dialog().getByTestId("delete-data-confirm").waitFor();
      assert.equal(await apply().isDisabled(), true);

      // A narrow window: nothing scrolls sideways.
      await page.setViewportSize({ width: 390, height: 800 });
      await open("delete-data-worktrees");
      assert.equal(
        await page.locator(".g-dialog").evaluate((node) => node.scrollWidth <= node.clientWidth),
        true,
      );
      await audit("narrow");
      assert.deepEqual(errors, [], `${engine} page errors`);
      console.log(`${engine}: review, worktree warning, Trash, failed start and narrow passed`);
    } finally {
      await browser.close();
    }
  }
  console.log(`${audits} axe audits, no violations`);
} finally {
  server?.kill();
}
