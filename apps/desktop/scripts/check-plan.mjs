/**
 * Browser fixture evidence for the plan card (`?preview=plan`): a plan an agent asks to leave
 * plan mode with. No native window and no agent: the fixture answers as the core would.
 *
 * The plan reads as a document, not as a tool's request: its own title, whose it is, its
 * sections and its prose, with its answers in view. It can be accepted, accepted as the person
 * edited it, sent back edited, sent back with a note, or declined, by the pointer and by the
 * keyboard; an accepted plan shows the edits it was accepted with.
 * Usage: node scripts/check-plan.mjs [--server URL] [--engines chromium,webkit]
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";

const { values } = parseArgs({
  options: { server: { type: "string" }, engines: { type: "string", default: "chromium,webkit" } },
});
const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// The fixture suite hands its own server over in CHECK_URL; alone, this check starts one.
const given = values.server ?? process.env.CHECK_URL;
const url = given ?? "http://127.0.0.1:1465";
const require = createRequire(import.meta.url);
let server;
let serverOutput = "";

async function ready() {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // Not up yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`The preview server did not start.\n${serverOutput}`);
}

async function inspect(engine, launcher) {
  const browser = await launcher.launch();
  try {
    for (const theme of ["light", "dark"]) {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 900 },
        colorScheme: theme,
      });
      const page = await context.newPage();
      page.setDefaultTimeout(12000);
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const what = (text) => `${engine} ${theme}: ${text}`;
      const waiting = page.locator('[data-kind="plan-approval"][data-resolved="false"]').first();
      const answered = page.locator('[data-kind="plan-approval"][data-resolved="true"]').first();
      const open = async () => {
        await page.goto(`${url}/?preview=plan`);
        await waiting.getByRole("button", { name: "Accept plan" }).waitFor();
      };

      // A document, not a tool's request.
      await open();
      await waiting.getByRole("heading", { name: "Keep the active chat visible" }).waitFor();
      const text = await waiting.textContent();
      assert.match(text, /Claude Code's plan · 3 steps/, what("whose plan, and how much"));
      for (const gone of ["Plan ready for review", "ExitPlanMode", "Allow once"])
        assert.ok(!text.includes(gone), what(`no "${gone}"`));
      assert.equal(
        await waiting.evaluate((node) => node.classList.contains("chat-block")),
        false,
        what("it is not a tool block"),
      );
      assert.equal(
        (text.match(/Keep the active chat visible/g) ?? []).length,
        1,
        what("the title is said once"),
      );
      // Its sections are chips that jump to them, after the title.
      const sections = waiting.getByRole("navigation", { name: "Sections of the plan" });
      assert.deepEqual(await sections.getByRole("button").allTextContents(), [
        "Context",
        "Changes",
        "Verification",
      ]);
      await sections.getByRole("button", { name: "Verification" }).click();
      // The answers stay in view, and nothing spills sideways.
      const layout = await page.evaluate(() => {
        const accept = document.querySelector(
          '[data-kind="plan-approval"] [data-testid="permission-option"]',
        );
        const box = accept.getBoundingClientRect();
        return {
          inView: box.top >= 0 && box.bottom <= innerHeight,
          overflow: document.documentElement.scrollWidth - innerWidth,
        };
      });
      assert.equal(layout.inView, true, what("the answers are in view"));
      assert.ok(layout.overflow <= 0, what("no sideways overflow"));
      assert.deepEqual(
        (await new AxeBuilder({ page }).include('[data-kind="plan-approval"]').analyze())
          .violations,
        [],
        what("the plan card"),
      );

      // Edit: the plan's own text. Nothing can be answered until it differs; Escape puts it back.
      await waiting.getByRole("button", { name: "Edit" }).click();
      const editor = waiting.getByLabel("The plan, as Markdown");
      await editor.waitFor();
      assert.equal(await editor.evaluate((node) => node === document.activeElement), true);
      const original = await editor.inputValue();
      assert.ok(original.startsWith("# Keep the active chat visible"), what("the plan's text"));
      const acceptEdited = waiting.getByRole("button", { name: "Accept edited plan" });
      assert.equal(await acceptEdited.isDisabled(), true, what("nothing to accept yet"));
      assert.equal(
        await waiting.getByRole("button", { name: "Send back edited" }).isDisabled(),
        true,
      );
      await page.keyboard.press("Escape");
      await waiting.getByRole("button", { name: "Accept plan" }).waitFor();
      assert.equal(await editor.count(), 0, what("Escape leaves the editor"));
      // Accepted as edited, by the keyboard: the card then shows what was accepted.
      await waiting.getByRole("button", { name: "Edit" }).click();
      await editor.fill(`${original}\n4. Check the collapsed rail too.`);
      assert.equal(await acceptEdited.isEnabled(), true);
      await page.keyboard.press("Control+Enter");
      await answered.getByText("Plan accepted with your edits.").waitFor();
      await answered.getByText("Check the collapsed rail too.").waitFor();
      assert.match(await answered.textContent(), /4 steps/, what("the edited plan's steps"));
      assert.equal(
        await answered.locator(".chat-plan-foot, .chat-plan-edit").count(),
        0,
        what("nothing left to answer"),
      );

      // Sent back edited: declined, with the person's version for the agent to plan on.
      await open();
      await waiting.getByRole("button", { name: "Edit" }).click();
      await editor.fill("# Keep the active chat visible\n\n1. Only scroll it into view.");
      const calls = () => page.evaluate(() => window.planPreview.calls());
      await waiting.getByRole("button", { name: "Send back edited" }).click();
      await answered.getByText("Sent back with your edits.").waitFor();
      // The edited plan went inside the refusal itself: one call, and no message beside it.
      assert.equal((await calls()).length, 1, what("one call for an edited plan sent back"));
      assert.match(
        (await calls())[0],
        /^decline:I edited the plan\. Use this version in place of yours/,
        what("the edited plan goes with the refusal"),
      );
      assert.equal(await page.getByTestId("plan-note-sent").count(), 0);
      // A declined plan is still the agent's own.
      await answered.getByText("Give each project a clearer heading row.").waitFor();

      // Request changes: a note, sent by the keyboard. It goes with the refusal, as written:
      // nothing is put around it, so a question stays a question.
      await open();
      await waiting.getByRole("button", { name: "Request changes" }).click();
      const note = waiting.getByLabel("What should change?");
      assert.equal(await note.evaluate((node) => node === document.activeElement), true);
      assert.match(
        await note.getAttribute("placeholder"),
        /Claude Code reads it with the plan and keeps planning\.$/,
        what("where the note goes"),
      );
      await note.fill("Are we keeping the scroll position?");
      await page.keyboard.press("Control+Enter");
      await answered.getByText("Sent back with your note.").waitFor();
      assert.equal(
        await answered.getByTestId("plan-note-sent").textContent(),
        "Are we keeping the scroll position?",
      );
      assert.deepEqual(
        await calls(),
        ["decline:Are we keeping the scroll position?"],
        what("the note goes inside the refusal, with nothing added and no message after it"),
      );
      assert.deepEqual(
        (await new AxeBuilder({ page }).include('[data-kind="plan-approval"]').analyze())
          .violations,
        [],
        what("a plan sent back with a note"),
      );

      // Decline says nothing more, and says the next word is the person's.
      await open();
      await waiting.getByRole("button", { name: "Decline" }).click();
      await answered.getByText("Declined. Tell Claude Code what you want instead.").waitFor();
      assert.deepEqual(await calls(), ["respond:deny"], what("a plain decline is one answer"));
      assert.equal(await page.getByTestId("plan-note-sent").count(), 0);

      // An agent that takes no words with a refusal: the plan is declined and the note follows
      // as the person's next message, and the field says so before it is written.
      await page.goto(`${url}/?preview=plan&edits=none`);
      await waiting.getByRole("button", { name: "Request changes" }).click();
      assert.match(
        await note.getAttribute("placeholder"),
        /It goes to Claude Code as your next message\.$/,
        what("where the note goes without words on a refusal"),
      );
      await note.fill("Fewer steps, please.");
      await waiting.getByRole("button", { name: "Send back" }).click();
      await answered.getByText("Sent back with your note.").waitFor();
      assert.deepEqual(await calls(), ["respond:deny"], what("declined first"));
      await page.getByText("Fewer steps, please.").first().waitFor();

      // Accept, by the keyboard from wherever the sheet has focus.
      await open();
      await page.locator(".chat-permission-dock").focus();
      await page.keyboard.press("Control+Enter");
      await answered.getByText("Plan accepted.", { exact: true }).waitFor();
      assert.deepEqual(errors, [], what("no page errors"));
      await context.close();
    }

    // Reduced motion: the sheet is simply there.
    const still = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      reducedMotion: "reduce",
    });
    const page = await still.newPage();
    page.setDefaultTimeout(12000);
    await page.goto(`${url}/?preview=plan`);
    const card = page.locator(".chat-permission-dock .chat-plan");
    await card.waitFor();
    assert.equal(
      await card.evaluate((node) => getComputedStyle(node).animationName),
      "none",
      `${engine}: no entrance with reduced motion`,
    );
    await still.close();

    // The smallest window: the answers are still in view and the editor fits.
    const small = await browser.newContext({ viewport: { width: 960, height: 640 } });
    const narrow = await small.newPage();
    narrow.setDefaultTimeout(12000);
    await narrow.goto(`${url}/?preview=plan`);
    const plan = narrow.locator('[data-kind="plan-approval"]').first();
    await plan.getByRole("button", { name: "Edit" }).click();
    const fits = await narrow.evaluate(() => {
      const inView = (node) => {
        const box = node.getBoundingClientRect();
        return (
          box.top >= 0 && box.bottom <= innerHeight && box.left >= 0 && box.right <= innerWidth
        );
      };
      return {
        actions: [...document.querySelectorAll(".chat-plan-actions button")].every(inView),
        overflow: document.documentElement.scrollWidth - innerWidth,
      };
    });
    assert.equal(fits.actions, true, `${engine}: at 960 by 640 the editor's answers are in view`);
    assert.ok(fits.overflow <= 0, `${engine}: at 960 by 640 nothing spills sideways`);
    await small.close();
    console.log(
      `${engine}: the plan reads as a document and is accepted, accepted edited, sent back and declined`,
    );
  } finally {
    await browser.close();
  }
}

try {
  if (!given) {
    const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
    server = spawn(
      process.execPath,
      [vite, "--host", "127.0.0.1", "--port", "1465", "--strictPort"],
      { cwd: app, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
    );
    for (const stream of [server.stdout, server.stderr])
      stream.on("data", (data) => {
        serverOutput = (serverOutput + data).slice(-8192);
      });
  }
  await ready();
  for (const engine of values.engines.split(",")) {
    const launcher = { chromium, webkit }[engine];
    assert.ok(launcher, `Unsupported engine: ${engine}`);
    await inspect(engine, launcher);
  }
} finally {
  server?.kill();
}
