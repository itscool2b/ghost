/**
 * Scripted Dashboard checks (`?preview=dashboard`). Reads no process table, stops nothing. The
 * Dashboard opens over the window as a modal panel and goes again (Dashboard.tsx): the checks open
 * it from its button, close it with Escape, its close button and by opening a chat.
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

export async function checkDashboard(browser, url, out, engine) {
  for (const theme of ["light", "dark"]) {
    for (const [width, height] of [
      [1280, 860],
      [960, 640],
    ]) {
      const context = await browser.newContext({
        viewport: { width, height },
        colorScheme: theme,
        reducedMotion: "reduce",
      });
      try {
        const page = await context.newPage();
        const errors = [];
        page.on("pageerror", (error) => errors.push(error.message));
        const audit = async (name) => {
          assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
          const overflow = await page.evaluate(
            () => document.documentElement.scrollWidth > document.documentElement.clientWidth,
          );
          assert.equal(overflow, false, "no horizontal overflow");
          await page.screenshot({
            path: resolve(out, `${engine}-${theme}-${width}-${name}.png`),
            fullPage: true,
          });
        };
        const laneOf = (chatId) =>
          page.evaluate(
            (id) =>
              document.querySelector(`[data-test-chat-id="${id}"]`)?.closest("section")?.dataset
                .lane ?? null,
            chatId,
          );

        await page.goto(`${url}/?preview=dashboard&theme=${theme}`);
        const dashboard = page.getByRole("dialog", { name: "Dashboard" });
        await dashboard.getByRole("heading", { name: "Dashboard", level: 2 }).waitFor();
        const lanes = page.getByTestId("dashboard-lane");
        assert.equal(await lanes.count(), 3);
        // A panel over the window, as wide as the window allows up to 1180px, its lanes side by
        // side (they once shrank to a 70px column, every word on its own line).
        const frame = await page.evaluate(() => {
          const panel = document.querySelector(".dashboard-panel").getBoundingClientRect();
          const boxes = [...document.querySelectorAll('[data-testid="dashboard-lane"]')].map(
            (lane) => lane.getBoundingClientRect(),
          );
          return {
            left: panel.left,
            right: panel.right,
            top: panel.top,
            bottom: panel.bottom,
            width: panel.width,
            window: innerWidth,
            height: innerHeight,
            tops: boxes.map((box) => Math.round(box.top)),
            widths: boxes.map((box) => Math.round(box.width)),
          };
        });
        assert.ok(
          frame.left >= 16 && frame.right <= frame.window - 16,
          `the panel stands inside the window: ${frame.left}..${frame.right}`,
        );
        assert.ok(frame.top >= 16 && frame.bottom <= frame.height - 16, "and inside its height");
        assert.ok(
          frame.width >= Math.min(1180, frame.window - 48) - 1,
          `as wide as the window allows: ${frame.width}`,
        );
        assert.equal(new Set(frame.tops).size, 1, `lanes side by side: ${frame.tops}`);
        assert.ok(Math.min(...frame.widths) >= 220, `lanes have room: ${frame.widths}`);
        assert.deepEqual(await dashboard.getByRole("heading", { level: 3 }).allTextContents(), [
          "Needs you 2",
          "Working 3",
          "Finished 2",
          "AI line edits",
        ]);
        // Opened, focus is on the board; the button that opens it says it is open.
        assert.equal(
          await page.evaluate(() => document.activeElement?.classList.contains("dashboard-page")),
          true,
        );
        assert.equal(
          await page.getByTestId("dashboard-button").getAttribute("aria-expanded"),
          "true",
        );
        assert.equal(
          await page.getByTestId("dashboard-summary").textContent(),
          "2 need you, 3 working, 2 finished in the last day.",
        );
        // A worktree chat names its project, not its app-owned folder; archived chats are off.
        assert.match(
          await page.locator('[data-test-chat-id="chat-worktree"]').textContent(),
          /Codex · auracode/,
        );
        assert.equal(await laneOf("chat-archived"), null);

        // A chat that waits on you sits under Needs you even though its agent still runs.
        assert.equal(await laneOf("chat-review"), "attention");
        assert.equal(await laneOf("chat-grok"), "attention");
        assert.equal(await laneOf("chat-claude"), "working");
        assert.equal(await laneOf("chat-pi"), "finished");
        // Quiet for two days: in the sidebar only.
        assert.equal(await laneOf("chat-old"), null);
        const review = page.locator('[data-test-chat-id="chat-review"]');
        assert.match(await review.textContent(), /Waiting for your answer · 2 minutes ago/);

        // A working card says what its tasks do now, and folds its programs.
        const claude = page.locator('[data-test-chat-id="chat-claude"]');
        const tasks = claude.getByTestId("dashboard-task");
        // Tasks and programs arrive with the first reading of the process table.
        await tasks.nth(1).waitFor();
        assert.equal(await tasks.count(), 2, "running tasks only");
        assert.match(await tasks.nth(1).textContent(), /Explore dictation feature end to end/);
        assert.match(await tasks.nth(1).textContent(), /Reading audio\.rs/);
        const programs = claude.getByTestId("dashboard-programs");
        assert.equal(await programs.evaluate((node) => node.open), false);
        await programs.locator("summary").click();
        const rows = claude.getByTestId("process-row");
        assert.equal(await rows.count(), 4);
        assert.deepEqual(
          await rows.evaluateAll((items) => items.map((item) => item.dataset.depth)),
          ["0", "1", "2", "1"],
        );
        assert.match(await rows.nth(0).textContent(), /claude\.exe.*Agent · PID 4120/);
        // The agent stops from its chat; every program under it has its own Stop.
        assert.equal(await rows.nth(0).getByRole("button").count(), 0);
        assert.equal(await rows.getByRole("button", { name: /^Stop / }).count(), 3);

        // AI line edits, under the board: a year of days, All or one agent at a time.
        const activity = page.getByTestId("dashboard-activity");
        const total = activity.getByTestId("dashboard-activity-total");
        await page.waitForFunction(
          () =>
            document.querySelector('[data-testid="dashboard-activity-total"]')?.textContent !== "–",
        );
        const squares = activity.locator(".dashboard-activity-grid > span");
        const days = await squares.count();
        assert.ok(days >= 365 && days <= 371, `a year of days: ${days}`);
        const fits = await activity.evaluate((card) => {
          const grid = card.querySelector(".dashboard-activity-grid").getBoundingClientRect();
          return grid.right <= card.getBoundingClientRect().right;
        });
        assert.ok(fits, "the squares fit the card");
        const agents = activity.getByTestId("dashboard-activity-agent");
        assert.deepEqual(await agents.allTextContents(), ["All", "Claude Code", "Codex"]);
        const figure = async () => Number((await total.textContent()).replaceAll(",", ""));
        const everyAgent = await figure();
        assert.ok(everyAgent > 0, "the invented year has edits");
        await agents.nth(2).click();
        assert.equal(await agents.nth(2).getAttribute("aria-pressed"), "true");
        assert.ok((await figure()) < everyAgent, "Codex's share is less than all of them");
        await agents.nth(0).click();
        assert.equal(await figure(), everyAgent);
        assert.deepEqual(await activity.locator(".dashboard-activity-stats dt").allTextContents(), [
          "Most active month",
          "Most active day",
          "Longest streak",
          "Current streak",
        ]);
        // A square says its day under the pointer.
        await activity.locator('.dashboard-activity-grid > [data-level="4"]').first().hover();
        assert.match(
          await activity.locator(".dashboard-activity-tip").textContent(),
          /^[\d,]+ line edits? on \w+day, \w+ \d+, \d{4}$/,
        );
        await page.mouse.move(0, 0);
        await activity.locator(".dashboard-activity-tip").waitFor({ state: "detached" });

        // Processor use appears from the second reading.
        await page.waitForFunction(() => document.body.textContent.includes("% CPU"), null, {
          timeout: 6000,
        });
        await audit("board");

        await page.getByRole("button", { name: "Stop bash.exe, process 4388" }).click();
        await page.getByRole("status").filter({ hasText: "Stopped bash.exe" }).waitFor();
        await page.waitForFunction(
          () =>
            document
              .querySelector('[data-test-chat-id="chat-claude"]')
              ?.querySelectorAll('[data-testid="process-row"]').length === 2,
        );
        assert.equal(await page.getByText("cargo.exe").count(), 0, "what it started stopped too");

        // Escape closes it and hands focus back to its button, which opens it again.
        const opener = page.getByTestId("dashboard-button");
        const reopen = async () => {
          await dashboard.waitFor({ state: "detached" });
          await opener.click();
          await dashboard.waitFor();
        };
        await page.keyboard.press("Escape");
        await dashboard.waitFor({ state: "detached" });
        assert.equal(await opener.getAttribute("aria-expanded"), "false");
        assert.equal(
          await page.evaluate(() => document.activeElement?.dataset.testid),
          "dashboard-button",
        );
        await opener.click();
        await dashboard.waitFor();
        // A click beside the panel closes it; so does its close button.
        await page.mouse.click(8, frame.height - 8);
        await reopen();
        await dashboard.getByRole("button", { name: "Close the Dashboard" }).click();
        await reopen();

        // The whole card opens its chat, not only its title, and the Dashboard goes; its button
        // takes the focus.
        const pi = page.locator('[data-test-chat-id="chat-pi"]');
        const box = await pi.boundingBox();
        await page.mouse.click(box.x + box.width / 2, box.y + box.height - 8);
        assert.equal(await page.evaluate(() => document.body.dataset.openedChat), "chat-pi");
        await reopen();
        await page.evaluate(() => {
          document.body.dataset.openedChat = "";
        });
        const open = page.getByRole("button", { name: "Tidy the changelog check", exact: true });
        await open.focus();
        await page.keyboard.press("Enter");
        assert.equal(await page.evaluate(() => document.body.dataset.openedChat), "chat-pi");
        await reopen();

        // A card whose agent works or waits for an answer offers Stop; every other card Clear.
        assert.equal(await claude.getByTestId("dashboard-card-stop").count(), 1);
        assert.equal(await review.getByTestId("dashboard-card-stop").count(), 1);
        assert.equal(await pi.getByTestId("dashboard-card-clear").count(), 1);
        assert.equal(await pi.getByTestId("dashboard-card-stop").count(), 0);
        await claude.getByTestId("dashboard-card-stop").click();
        assert.equal(await page.evaluate(() => document.body.dataset.stoppedChat), "chat-claude");
        assert.equal(
          await page.evaluate(() => document.body.dataset.openedChat),
          "chat-pi",
          "Stop does not open the chat",
        );
        // Clear takes a card off the board without opening it; Clear finished takes the rest.
        await pi.getByTestId("dashboard-card-clear").click();
        await page.getByRole("heading", { name: "Finished 1", level: 3 }).waitFor();
        // The lane's count changes at once; the card leaves once its (here instant) exit settles,
        // which a slower machine reads before.
        await pi.waitFor({ state: "detached" });
        assert.equal(await laneOf("chat-pi"), null);
        await page.getByTestId("dashboard-clear-finished").click();
        await page.getByRole("heading", { name: "Finished 0", level: 3 }).waitFor();
        assert.equal(await page.getByTestId("dashboard-clear-finished").count(), 0);

        await page.goto(`${url}/?preview=dashboard&theme=${theme}&case=empty`);
        await page.getByText("Nothing is running.", { exact: false }).waitFor();
        assert.equal(await page.getByText("Nothing needs you.").count(), 1);
        // No edits yet: the empty year, no agents to choose between.
        await page.getByTestId("dashboard-activity-total").filter({ hasText: /^0$/ }).waitFor();
        assert.equal(await page.getByTestId("dashboard-activity-agent").count(), 0);
        assert.equal(await page.getByText("None yet").count(), 2);
        await audit("empty");

        // A busy day: Finished shows its newest eight, and the rest on request.
        await page.goto(`${url}/?preview=dashboard&theme=${theme}&case=busy`);
        const finished = page.locator('[data-lane="finished"] [data-testid="dashboard-card"]');
        await page.getByRole("heading", { name: "Finished 13", level: 3 }).waitFor();
        assert.equal(await finished.count(), 8);
        assert.equal(
          await finished.first().getAttribute("data-test-chat-id"),
          "chat-pi",
          "newest first",
        );
        await page.getByRole("button", { name: "Show 5 more" }).click();
        assert.equal(await finished.count(), 13);
        await audit("busy");
        await page.getByRole("button", { name: "Show fewer" }).click();
        // The cards that go leave on the next frame.
        await page.waitForFunction(
          () =>
            document.querySelectorAll('[data-lane="finished"] [data-testid="dashboard-card"]')
              .length === 8,
        );
        assert.deepEqual(errors, []);
      } finally {
        await context.close();
      }
    }
  }
  await checkMoves(browser, url, engine);
}

/**
 * Cards that change lane, with motion on: a card leaves one lane and arrives in the other, and is
 * never seen outside its own lane's column (cards used to fly across the board over the others,
 * their two copies cross-fading).
 */
async function checkMoves(browser, url, engine) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 860 } });
  try {
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`${url}/?preview=dashboard&case=live`);
    await page.getByRole("heading", { name: "Dashboard", level: 2 }).waitFor();
    const samples = () =>
      page.evaluate(() => {
        const columns = Object.fromEntries(
          [...document.querySelectorAll('[data-testid="dashboard-lane"]')].map((lane) => {
            const box = lane.getBoundingClientRect();
            return [lane.dataset.lane, [box.left, box.right]];
          }),
        );
        return [...document.querySelectorAll('[data-testid="dashboard-card"]')].map((card) => {
          const box = card.getBoundingClientRect();
          const [left, right] = columns[card.dataset.lane];
          return {
            id: card.dataset.testChatId,
            lane: card.dataset.lane,
            inside: box.left >= left - 1 && box.right <= right + 1,
          };
        });
      });
    const lanesSeen = new Set();
    const deadline = Date.now() + 8000;
    while (Date.now() < deadline) {
      for (const card of await samples()) {
        assert.ok(card.inside, `${engine}: ${card.id} stays in its ${card.lane} column`);
        if (card.id === "chat-codex") lanesSeen.add(card.lane);
      }
      await page.waitForTimeout(40);
    }
    assert.deepEqual([...lanesSeen].sort(), ["attention", "working"], "the chat changed lane");
    assert.deepEqual(errors, []);
  } finally {
    await context.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/dashboard-browser/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkDashboard(browser, process.env.CHECK_URL ?? "http://localhost:1448", out, engine);
      console.log(
        `${engine}: dashboard lanes, cards, tasks, programs, stop, AI line edits and empty board passed in both appearances and sizes`,
      );
    } finally {
      await browser.close();
    }
  }
}
