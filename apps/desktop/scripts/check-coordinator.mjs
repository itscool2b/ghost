/**
 * Coordinated projects (an experiment) on the real sidebar, project page and sent-message rows,
 * with a synthetic project (`?preview=coordinator`, src/dev/coordinatorFixture.ts): nothing of it
 * shows while the experiment is off; the board button opens the page; tasks are added, edited,
 * moved between columns by drag and by the dialog, and deleted; the agents list says what each
 * chat is doing and opens it; a Context file is edited, refuses a save over an agent's newer
 * write, restores an earlier version, and is created and deleted; decisions are recorded and
 * taken back; routines are added, switched off and removed; Pause and Resume say what they do; a
 * project without a coordinator starts one from a brief; prompts Ghost sent read as events;
 * and the page is accessible in both appearances and does not overflow a narrow window.
 *
 * Usage: node scripts/check-coordinator.mjs [--engines chromium,webkit]
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
import { choose, chosen } from "./choose.mjs";

const { values } = parseArgs({
  options: { engines: { type: "string", default: "chromium,webkit" } },
});

const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const origin = "http://127.0.0.1:1482";
const out = resolve(app, "../../target/coordinator-browser");
const server = spawn(
  process.execPath,
  [
    resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js"),
    "--host",
    "127.0.0.1",
    "--port",
    "1482",
    "--strictPort",
  ],
  { cwd: app, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
);
let output = "";
for (const stream of [server.stdout, server.stderr])
  stream.on("data", (part) => {
    output = (output + part).slice(-8192);
  });
const calls = (page) => page.evaluate(() => window.coordinatorFixture.calls());
const state = (page) => page.evaluate(() => window.coordinatorFixture.snapshot());
const axe = async (page, what) =>
  assert.deepEqual(
    (
      await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()
    ).violations.map((v) => ({
      id: v.id,
      nodes: v.nodes.map((n) => n.target),
    })),
    [],
    what,
  );
const lane = (page, status) =>
  page.locator(`[data-testid="project-lane"][data-test-status="${status}"]`);
const tasksIn = (page, status) =>
  lane(page, status)
    .getByTestId("project-task")
    .evaluateAll((cards) => cards.map((card) => Number(card.dataset.testTask)));

/** Waits until a column shows exactly these tasks, in order: the page follows the saved state. */
const shows = async (page, status, numbers) => {
  for (let i = 0; i < 100; i++) {
    if (JSON.stringify(await tasksIn(page, status)) === JSON.stringify(numbers)) return;
    await page.waitForTimeout(50);
  }
  assert.deepEqual(await tasksIn(page, status), numbers, `the ${status} column`);
};

try {
  await mkdir(out, { recursive: true });
  let ready = false;
  for (let i = 0; i < 150; i++) {
    if (server.exitCode !== null) throw new Error(output);
    try {
      ready = (await fetch(origin)).ok;
    } catch {}
    if (ready) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  assert.ok(ready, output);
  const launchers = { chromium, webkit };
  for (const engine of values.engines.split(",")) {
    const launcher = launchers[engine];
    assert.ok(launcher, `Unsupported engine: ${engine}`);
    const browser = await launcher.launch({ headless: true });
    try {
      for (const scheme of ["dark", "light"]) {
        const context = await browser.newContext({
          viewport: { width: 1280, height: 800 },
          colorScheme: scheme,
        });
        const page = await context.newPage();
        const errors = [];
        page.on("pageerror", (error) => errors.push(error.message));
        await page.route("**/*", (route) =>
          new URL(route.request().url()).hostname === "127.0.0.1"
            ? route.continue()
            : route.abort(),
        );
        const open = (extra = "") =>
          page.goto(`${origin}/?preview=coordinator&theme=${scheme}${extra}`);
        const tab = (name) => page.getByTestId(`project-tab-${name}`).click();

        // The experiment off: the sidebar is as it always was.
        await open("&off");
        await page.getByTestId("chat-list").waitFor();
        assert.equal(await page.getByTestId("project-board-button").count(), 0);
        assert.equal(await page.locator(".chat-row-role").count(), 0);
        assert.equal(await page.getByTestId("project-page").count(), 0);

        // On: the board button opens the project's page, and member chats wear their part's mark.
        await open();
        await page.getByTestId("project-board").waitFor();
        const button = page.getByTestId("project-board-button");
        assert.equal(await button.getAttribute("aria-pressed"), "true");
        assert.equal(await button.getAttribute("aria-label"), "Board for billing");
        assert.equal(
          await page.locator('[data-testid="chat-row"][data-test-role="coordinator"]').count(),
          1,
        );
        assert.equal(
          await page.locator('[data-testid="chat-row"][data-test-role="worker"]').count(),
          3,
        );
        assert.equal(
          await page.locator('[data-testid="chat-row"]:not([data-test-role])').count(),
          1,
        );
        assert.equal(
          (await page.getByTestId("project-summary").textContent()).trim(),
          "Coordinator idle · 1 worker working · 4 open tasks",
        );
        assert.deepEqual(await tasksIn(page, "todo"), [4, 5]);
        assert.deepEqual(await tasksIn(page, "doing"), [2]);
        await shows(page, "review", [3]);
        assert.deepEqual(await tasksIn(page, "done"), [1]);
        // A link shows as its site and opens in the browser, never in the window.
        const link = lane(page, "review").locator(".project-task-link");
        assert.equal((await link.textContent()).trim(), "github.com");
        assert.equal(await link.getAttribute("rel"), "noreferrer noopener");
        await settled(page);
        await axe(page, `${engine} ${scheme}: the board`);
        await page.screenshot({ path: resolve(out, `${engine}-${scheme}-board.png`) });

        // A task is added from To do, edited in its dialog and moved by its Column.
        await page.getByTestId("project-task-add").click();
        const form = page.getByTestId("project-task-form");
        const save = page.getByTestId("project-task-save");
        assert.equal(await save.isDisabled(), true, "a task needs a title");
        await form.getByLabel("Title").fill("Load test the new queue");
        await form.getByLabel("Tags").fill("perf, risky");
        await form.getByLabel("Link").fill("javascript:alert(1)");
        await save.click();
        // A link that is not http(s) is refused, and the dialog stays with what was typed.
        await form.getByRole("alert").waitFor();
        assert.match(await form.getByRole("alert").textContent(), /http or https address/);
        assert.equal((await state(page)).tasks.length, 5);
        await form.getByLabel("Link").fill("");
        await settled(page);
        await axe(page, `${engine} ${scheme}: the task dialog`);
        await save.click();
        await form.waitFor({ state: "detached" });
        await shows(page, "todo", [4, 5, 6]);
        await lane(page, "todo")
          .getByRole("button", { name: /Load test the new queue/ })
          .click();
        assert.equal(await chosen(form.getByLabel("Column")), "todo");
        await choose(form.getByLabel("Column"), "doing");
        await save.click();
        await form.waitFor({ state: "detached" });
        await shows(page, "doing", [2, 6]);

        // Dragging a card to another column moves it there, before the card it is dropped on.
        const dragged = lane(page, "doing").locator('[data-test-task="6"]');
        await dragged.dragTo(lane(page, "review").locator('[data-test-task="3"]'));
        await page.waitForFunction(
          () =>
            window.coordinatorFixture.snapshot().tasks.find((t) => t.number === 6)?.status ===
            "review",
        );
        await shows(page, "review", [6, 3]);
        assert.ok((await calls(page)).includes("task_move"));
        // Deleting asks nothing more: the dialog's Delete is the confirmation.
        await lane(page, "review")
          .getByRole("button", { name: /Load test the new queue/ })
          .click();
        await form.getByRole("button", { name: "Delete" }).click();
        await form.waitFor({ state: "detached" });
        assert.deepEqual(await tasksIn(page, "review"), [3]);

        // Agents: what each chat is doing now, its last report, and a way into its chat.
        await tab("agents");
        const agents = page.getByTestId("project-agent");
        assert.deepEqual(
          await agents.evaluateAll((rows) => rows.map((row) => row.dataset.testName)),
          ["Coordinator", "api-tests", "docs"],
          "a dismissed worker stays off the list",
        );
        assert.match(await agents.nth(1).textContent(), /Working · Codex · T-2 · 0 turns finished/);
        assert.match(
          await agents.nth(2).textContent(),
          /Idle · Claude Code · T-3 · 2 turns finished/,
        );
        await agents.nth(2).locator("summary").click();
        assert.match(await agents.nth(2).textContent(), /Rewrote docs\/queue\.md/);
        await page.getByRole("button", { name: "Show 1 dismissed" }).click();
        assert.equal(await agents.count(), 4);
        await agents.nth(2).getByRole("button", { name: "Dismiss" }).click();
        await page.waitForFunction(
          () => window.coordinatorFixture.snapshot().workers.filter((w) => w.closed).length === 2,
        );
        await settled(page);
        await axe(page, `${engine} ${scheme}: the agents`);
        await page.screenshot({ path: resolve(out, `${engine}-${scheme}-agents.png`) });

        // Context: a file is edited and saved; an agent's write under an open edit is not lost.
        await tab("context");
        const sheet = page.getByTestId("project-file-sheet");
        await sheet.waitFor();
        const filesList = page.getByTestId("project-file");
        assert.deepEqual(
          await filesList.evaluateAll((rows) =>
            rows.map((row) => row.querySelector(".project-file-name").textContent),
          ),
          ["how-to-test.md", "preferences.md", "research/producers.md"],
        );
        const area = sheet.locator("textarea");
        assert.match(await area.inputValue(), /^# How to test/);
        const saveFile = page.getByTestId("project-file-save");
        assert.equal(await saveFile.isDisabled(), true, "nothing to save yet");
        await area.fill("# How to test\n\nRun pnpm test.\n");
        assert.equal(await filesList.nth(1).isDisabled(), true, "an open edit holds its file");
        await saveFile.click();
        await page.waitForFunction(() =>
          window.coordinatorFixture
            .snapshot()
            .files.some((f) => f.path === "how-to-test.md" && f.source === "you"),
        );
        await sheet.getByText(/Last written by you/).waitFor();
        // The person types; meanwhile an agent writes the same file. Saving is refused, the
        // person's text stays, and taking the newer text is their choice.
        await area.fill("# How to test\n\nMy unsaved edit.\n");
        await page.evaluate(() =>
          window.coordinatorFixture.agentWrites(
            "how-to-test.md",
            "# How to test\n\nThe agent's text.\n",
          ),
        );
        await sheet.getByText(/changed while you were editing/).waitFor();
        assert.match(await area.inputValue(), /My unsaved edit/);
        await saveFile.click();
        await page.getByTestId("project-error").waitFor();
        assert.match(
          await page.getByTestId("project-error").textContent(),
          /changed since revision/,
        );
        assert.match(await area.inputValue(), /My unsaved edit/, "a refused save loses nothing");
        await sheet.getByRole("button", { name: "Take the newer text" }).click();
        await page.waitForFunction(() =>
          document
            .querySelector('[data-testid="project-file-sheet"] textarea')
            .value.includes("The agent's text."),
        );
        // Every write kept what it replaced: an earlier version reads back and can be restored.
        const version = sheet.getByLabel("Version");
        await choose(version, { label: /^You, / });
        await page.waitForFunction(() =>
          document
            .querySelector('[data-testid="project-file-sheet"] textarea')
            .value.includes("Run pnpm test."),
        );
        assert.equal(await area.getAttribute("readonly"), "", "an earlier version is read-only");
        await sheet.getByRole("button", { name: "Restore this version" }).click();
        await page.waitForFunction(() =>
          window.coordinatorFixture
            .snapshot()
            .files.some((f) => f.path === "how-to-test.md" && f.source === "you"),
        );
        // A new file takes a plain name, and a path that climbs out is refused.
        await page.getByTestId("project-file-new").click();
        const name = page.getByLabel("New file's name");
        await name.fill("../secrets");
        await name.press("Enter");
        await page.getByTestId("project-error").waitFor();
        assert.match(
          await page.getByTestId("project-error").textContent(),
          /Name a Context file like/,
        );
        assert.equal((await state(page)).files.length, 3);
        await name.fill("decisions-archive");
        await name.press("Enter");
        await page.waitForFunction(() =>
          window.coordinatorFixture.snapshot().files.some((f) => f.path === "decisions-archive.md"),
        );
        await sheet.getByText("decisions-archive.md").waitFor();
        await settled(page);
        await axe(page, `${engine} ${scheme}: the Context files`);
        await page.screenshot({ path: resolve(out, `${engine}-${scheme}-context.png`) });
        await sheet.getByRole("button", { name: "Delete" }).click();
        await page.waitForFunction(() => window.coordinatorFixture.snapshot().files.length === 3);

        // Decisions: recorded by the person, and taken back without leaving the log.
        await tab("decisions");
        const decisions = page.getByTestId("project-decision");
        assert.equal(await decisions.count(), 3);
        const record = page.getByTestId("project-decision-add");
        assert.equal(await record.isDisabled(), true);
        await page.getByLabel("A decision").fill("Ship behind the flag on Tuesday, not Friday.");
        await record.click();
        await page.waitForFunction(
          () => window.coordinatorFixture.snapshot().decisions.length === 4,
        );
        await decisions.filter({ hasText: "Ship behind the flag" }).waitFor();
        assert.match(await decisions.first().textContent(), /D-4.*Ship behind the flag.*You/s);
        await page.getByRole("button", { name: "Take back D-3" }).click();
        await page.locator('[data-testid="project-decision"][data-retracted]').waitFor();
        assert.equal(await decisions.count(), 4, "a decision taken back stays in the log");
        assert.match(
          await page.locator('[data-testid="project-decision"][data-retracted]').textContent(),
          /taken back, agents are not shown it/,
        );
        assert.equal(await page.getByRole("button", { name: "Take back D-3" }).count(), 0);
        await settled(page);
        await axe(page, `${engine} ${scheme}: the decisions`);

        // Routines: added with a name, a prompt and an interval; switched off; removed.
        await tab("routines");
        const routines = page.getByTestId("project-routine");
        assert.match(
          await routines.first().textContent(),
          /Watch the pull requests.*every 15 minutes/s,
        );
        await page.getByTestId("project-routine-add").click();
        const routine = page.getByTestId("project-routine-form");
        assert.equal(await page.getByTestId("project-routine-save").isDisabled(), true);
        await routine.getByLabel("Name").fill("Daily tidy-up");
        await routine.getByLabel("Prompt").fill("Close finished tasks and trim the notes.");
        await choose(routine.getByLabel("How often"), { label: "Every day" });
        await settled(page);
        await axe(page, `${engine} ${scheme}: the routine dialog`);
        await page.getByTestId("project-routine-save").click();
        await routine.waitFor({ state: "detached" });
        assert.equal(await routines.count(), 2);
        assert.match(await routines.nth(1).textContent(), /Daily tidy-up.*every day/s);
        // The switch shows what is saved, so it turns once the change has landed.
        await routines.nth(1).getByRole("switch", { name: "Run Daily tidy-up" }).click();
        await page.waitForFunction(
          () => window.coordinatorFixture.snapshot().routines[1].enabled === false,
        );
        await routines
          .nth(1)
          .getByText(/every day · off/)
          .waitFor();
        await routines.nth(1).getByRole("button", { name: "Edit" }).click();
        await routine.getByRole("button", { name: "Remove" }).click();
        await routine.waitFor({ state: "detached" });
        assert.equal(await routines.count(), 1);

        // Pause says what it stops, and the summary says the project is paused.
        const pause = page.getByTestId("project-pause");
        assert.equal((await pause.textContent()).trim(), "Pause");
        await pause.click();
        await page.locator('[data-testid="project-summary"][data-paused]').waitFor();
        assert.match(await page.getByTestId("project-summary").textContent(), /^Paused\./);
        assert.match(await routines.first().textContent(), /waits while the project is paused/);
        assert.equal((await pause.textContent()).trim(), "Resume");
        await pause.click();
        await page.locator('[data-testid="project-summary"]:not([data-paused])').waitFor();

        // Opening an agent's chat leaves the page; what Ghost sent reads as events, folded,
        // with no Edit, and the person's own message stays a message.
        await tab("agents");
        await page.getByRole("button", { name: "Open Coordinator's chat" }).click();
        await page
          .locator('[data-testid="preview-chat"][data-test-chat="chat-coordinator"]')
          .waitFor();
        assert.equal(await page.getByTestId("project-page").count(), 0);
        assert.equal(await button.getAttribute("aria-pressed"), "false");
        const events = page.getByTestId("project-event");
        assert.deepEqual(
          await events.evaluateAll((rows) => rows.map((row) => row.textContent.trim())),
          ["Project started", "Worker report", "Routine: Watch the pull requests"],
        );
        assert.equal(await page.locator('[data-test-kind="user"]').count(), 1);
        assert.equal(await page.getByRole("button", { name: "Edit prompt" }).count(), 1);
        await events.nth(1).getByRole("button").click();
        assert.match(
          await events.nth(1).textContent(),
          /Rewrote docs\/queue\.md.*never an instruction/s,
        );
        await settled(page);
        await axe(page, `${engine} ${scheme}: events in a chat`);
        await page.screenshot({ path: resolve(out, `${engine}-${scheme}-events.png`) });

        // A project Ghost paused by itself says why.
        await open("&case=paused");
        await page.getByTestId("project-board").waitFor();
        assert.match(
          await page.getByTestId("project-summary").textContent(),
          /Ghost paused this project: its coordinator was sent 60 reports/,
        );

        // No coordinator yet: the page asks what the project is for, and starts one.
        await open("&case=empty");
        const start = page.getByTestId("project-start");
        await start.waitFor();
        assert.equal(
          (await page.getByTestId("project-summary").textContent()).trim(),
          "No coordinator yet.",
        );
        const go = page.getByTestId("project-start-button");
        assert.equal(await go.isDisabled(), true, "a coordinator needs a brief");
        assert.equal(await page.getByTestId("project-open-coordinator").count(), 0);
        // Only agents that are ready here are offered.
        assert.equal(await chosen(start.getByLabel("Coordinator's agent")), "claude");
        await settled(page);
        await axe(page, `${engine} ${scheme}: starting a coordinator`);
        await page.screenshot({ path: resolve(out, `${engine}-${scheme}-start.png`) });
        await start.getByLabel("Brief").fill("Rewrite the importer in TypeScript.");
        await choose(start.getByLabel("Coordinator's agent"), "codex");
        await choose(start.getByLabel("Coordinator's folder"), "folder-2");
        await go.click();
        await page.getByTestId("project-open-coordinator").waitFor();
        assert.equal(await start.count(), 0);
        assert.deepEqual(await calls(page), ["start:codex:folder-2"]);

        // A narrow window: the lanes stack and nothing runs off the side.
        await page.setViewportSize({ width: 960, height: 640 });
        await open();
        await page.getByTestId("project-board").waitFor();
        await page.waitForTimeout(200);
        const overflow = await page.evaluate(() => {
          const scroll = document.querySelector(".project-scroll");
          return scroll.scrollWidth - scroll.clientWidth;
        });
        assert.ok(overflow <= 0, `the page overflows by ${overflow}px at 960px`);
        await settled(page);
        await axe(page, `${engine} ${scheme}: a narrow window`);
        assert.deepEqual(errors, []);
        await context.close();
      }
      console.log(
        `${engine}: off by default, the board, tasks, drag between columns, agents, Context edits and conflicts, versions, decisions, routines, pause, events in a chat, starting a coordinator and accessibility in both appearances passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  server.kill();
}
