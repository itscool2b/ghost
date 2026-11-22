/**
 * Browser fixture evidence for the tour (`?preview=tour`). No native window: the chats, the
 * browser and the device are stand-ins, and the profile's saved setup lives in the page.
 *
 * The tour is offered, never started by itself. A flight visits every stop the registry names
 * (lib/tour/stops.ts): the ring is on the stop's own control, the wing and the card are on
 * screen, the stop's heading has focus, nothing covers the window, and the keyboard moves it.
 * What a profile saw is saved, so it is not offered again; a stop added since is offered alone.
 * And the rule the owner set: every control of the sidebar and of the chat's header either has a
 * stop or a written reason for having none.
 * Usage: node scripts/check-tour.mjs [--server URL] [--engines chromium,webkit]
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
const url = given ?? "http://127.0.0.1:1464";
const require = createRequire(import.meta.url);
let server;
let serverOutput = "";

/** Stops whose control this fixture cannot show: no worktree here saved a Run command. */
const ABSENT_HERE = new Set(["run"]);

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

/** Runs in the page: where the current stop's ring, wing, card and control are. */
function measure() {
  const box = (node) => {
    if (!node) return null;
    const rect = node.getBoundingClientRect();
    return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom };
  };
  const card = document.querySelector('[data-testid="tour-card"]');
  const stop = card?.getAttribute("data-stop") ?? null;
  let control = null;
  for (const node of document.querySelectorAll(`[data-tour="${stop}"]`)) {
    const host = node.closest("summary") ?? node;
    const rect = host.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) {
      control = host;
      break;
    }
  }
  const layer = document.querySelector('[data-testid="tour"]');
  const style = layer ? getComputedStyle(layer) : null;
  return {
    stop,
    title: card?.querySelector("h2")?.textContent ?? null,
    count: card?.querySelector(".tour-count")?.textContent ?? null,
    focused: document.activeElement === card?.querySelector("h2"),
    card: box(card),
    ring: box(document.querySelector(".tour-ring")),
    rings: document.querySelectorAll(".tour-ring").length,
    wing: box(document.querySelector('[data-testid="tour-wing"]')),
    settled: document.querySelector('[data-testid="tour-wing"]')?.hasAttribute("data-settled"),
    control: box(control),
    view: { width: innerWidth, height: innerHeight },
    layer: style && {
      background: style.backgroundColor,
      pointer: style.pointerEvents,
      filter: style.backdropFilter ?? "none",
    },
    logoHidden:
      getComputedStyle(document.querySelector(".chat-sidebar-footer-logo")).visibility === "hidden",
  };
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
      page.on("console", (message) => {
        if (message.type() === "error") errors.push(message.text());
      });
      const what = (text) => `${engine} ${theme}: ${text}`;
      const offer = page.getByTestId("tour-offer");
      const tour = page.getByTestId("tour");
      const registry = async () => page.evaluate(() => window.tourFixture);
      const seen = async () => page.evaluate(() => window.tourFixture.seen());
      /** Waits for the card of the stop after `previous` to be placed and its wing to settle. */
      const arrive = async (previous) => {
        await page.waitForFunction((before) => {
          const node = document.querySelector('[data-testid="tour-card"][data-placed]');
          const wing = document.querySelector('[data-testid="tour-wing"]');
          return (
            node !== null &&
            node.getAttribute("data-stop") !== before &&
            wing?.hasAttribute("data-settled")
          );
        }, previous ?? "");
        // The card's entrance and the ring's have a moment left to run, and a control that moved
        // as its page settled (New chat's agents arriving) takes the ring with it.
        await page.waitForTimeout(260);
        for (let attempt = 0; attempt < 12; attempt += 1) {
          const state = await page.evaluate(measure);
          const on =
            state.ring &&
            state.control &&
            Math.abs(state.control.top - state.ring.top - 4) <= 1 &&
            Math.abs(state.control.left - state.ring.left - 4) <= 1;
          if (on) return state;
          await page.waitForTimeout(100);
        }
        return page.evaluate(measure);
      };
      const audit = async (text) =>
        assert.deepEqual(
          (await new AxeBuilder({ page }).include('[data-testid="tour"]').analyze()).violations,
          [],
          what(text),
        );

      // Offered, never started: the card waits at the sidebar's foot and no flight begins.
      await page.goto(`${url}/?preview=tour&offer`);
      await offer.waitFor();
      const { stops, notToured } = await registry();
      assert.ok(stops.length >= 10, what("the registry reached the page"));
      assert.equal(await offer.getByRole("heading").textContent(), "Want the tour?");
      await page.waitForTimeout(1200);
      assert.equal(await page.getByTestId("tour-card").count(), 0, what("no flight by itself"));
      assert.deepEqual(await seen(), [], what("looking at the offer saves nothing"));
      const waiting = await page.evaluate(measure);
      assert.equal(waiting.logoHidden, true, what("the wing stands in for the logo it sits on"));
      assert.equal(waiting.layer.pointer, "none", what("the layer takes no clicks"));
      assert.match(waiting.layer.background, /rgba\(0, 0, 0, 0\)|transparent/, what("no fill"));
      await audit("the offer");

      // The whole flight, by the keyboard and the buttons.
      await offer.getByRole("button", { name: "Take the tour" }).click();
      const expected = stops.filter((stop) => !ABSENT_HERE.has(stop));
      const flown = [];
      let previous = null;
      for (let at = 0; at < expected.length; at += 1) {
        const state = await arrive(previous);
        flown.push(state.stop);
        assert.equal(state.stop, expected[at], what(`stop ${at + 1}`));
        assert.ok(state.title && state.title.length > 2, what(`${state.stop} has a heading`));
        assert.equal(state.focused, true, what(`${state.stop}: its heading has focus`));
        assert.equal(state.rings, 1, what(`${state.stop}: one ring`));
        assert.ok(state.control, what(`${state.stop}: its control is on screen`));
        // The ring is on the stop's own control, a few pixels out all round.
        for (const [edge, sign] of [
          ["left", -1],
          ["top", -1],
          ["right", 1],
          ["bottom", 1],
        ]) {
          const out = (state.ring[edge] - state.control[edge]) * sign;
          assert.ok(out >= 2 && out <= 6, what(`${state.stop}: ring ${edge} is ${out}px out`));
        }
        // The card and the wing are inside the window, and the card is not over the control.
        for (const part of ["card", "wing"]) {
          assert.ok(state[part].left >= 0 && state[part].top >= 0, what(`${state.stop} ${part}`));
          assert.ok(state[part].right <= state.view.width, what(`${state.stop} ${part} right`));
          assert.ok(state[part].bottom <= state.view.height, what(`${state.stop} ${part} foot`));
        }
        const apart =
          state.card.left >= state.control.right ||
          state.card.right <= state.control.left ||
          state.card.top >= state.control.bottom ||
          state.card.bottom <= state.control.top;
        assert.ok(apart, what(`${state.stop}: the card leaves its control in view`));
        assert.equal(state.logoHidden, true, what("the logo waits while the wing is out"));
        if (at === 0 || at === 7) await audit(`the card at ${state.stop}`);
        previous = state.stop;
        if (at === 2) {
          // Back and forward again by the arrow keys.
          await page.keyboard.press("ArrowLeft");
          const back = await arrive(previous);
          assert.equal(back.stop, expected[1], what("ArrowLeft goes back"));
          await page.keyboard.press("ArrowRight");
          const again = await arrive(back.stop);
          assert.equal(again.stop, expected[2], what("ArrowRight goes on"));
        }
        if (at === expected.length - 1) {
          // The count leaves out the stop this chat had no control for.
          assert.equal(
            state.count?.replace(/s+/g, " "),
            `${expected.length} of ${expected.length}`,
            what("the count ends on the stops it could show"),
          );
          assert.equal(await page.getByTestId("tour-next").textContent(), "Done");
          await page.getByTestId("tour-next").click();
        } else if (at % 3 === 0) await page.keyboard.press("Enter");
        else if (at % 3 === 1) await page.keyboard.press("ArrowRight");
        else await page.getByTestId("tour-next").click();
      }
      await tour.waitFor({ state: "detached" });
      assert.deepEqual(flown, expected, what("every stop was flown in order"));
      // Finished: every stop reached is saved, with the one this chat had no control for.
      assert.deepEqual([...(await seen())].sort(), [...stops].sort(), what("the flight is saved"));
      assert.equal(
        await page
          .locator(".chat-sidebar-footer-logo")
          .evaluate((node) => getComputedStyle(node).visibility),
        "visible",
        what("the wing is home: the logo shows again"),
      );
      // What the window showed before the flight is back.
      assert.equal(await page.getByTestId("new-chat").count(), 0, what("New chat is put away"));

      // The rule: every control of the sidebar and the header is toured, or says why not.
      await page.goto(`${url}/?preview=tour&open`);
      await page.getByTestId("chat-header").waitFor();
      await page.locator('[data-tour="composer"]').waitFor();
      const untoured = await page.evaluate((reasons) => {
        const controls = [
          ...document.querySelectorAll(
            ".chat-nav-actions > button, .chat-section-label button, .chat-sidebar-toggle, " +
              ".chat-sidebar-footer button, .chat-sidebar-footer summary, " +
              ".chat-header-controls > *",
          ),
        ];
        const missing = [];
        for (const control of controls) {
          const box = control.getBoundingClientRect();
          if (box.width === 0 || box.height === 0) continue;
          if (control.closest("[data-tour]") || control.querySelector("[data-tour]")) continue;
          // A menu's panel is reached through its button, which is the control.
          if (control.closest("details") && !control.matches("summary, details")) continue;
          const key =
            control.getAttribute("data-testid") ??
            control.getAttribute("aria-label") ??
            control.querySelector("[data-testid]")?.getAttribute("data-testid") ??
            control.className;
          if (!(key in reasons)) missing.push(key);
        }
        return { missing, counted: controls.length };
      }, notToured);
      assert.ok(untoured.counted >= 10, what("the controls were found"));
      assert.deepEqual(
        untoured.missing,
        [],
        what(
          "a control has neither a tour stop nor a reason in NOT_TOURED (lib/tour/stops.ts): add data-tour and a stop",
        ),
      );

      // Escape ends a flight, and a flight that was stopped is not offered again.
      await page.goto(`${url}/?preview=tour&go`);
      await arrive(null);
      await page.keyboard.press("Escape");
      await tour.waitFor({ state: "detached" });
      assert.deepEqual([...(await seen())].sort(), [...stops].sort(), what("stopped means no"));

      // Not now: nothing flies, and the profile is not asked again.
      await page.goto(`${url}/?preview=tour&offer`);
      await offer.getByRole("button", { name: "Not now" }).click();
      await tour.waitFor({ state: "detached" });
      assert.deepEqual([...(await seen())].sort(), [...stops].sort(), what("declined is saved"));
      await page.goto(`${url}/?preview=tour&offer&seen=${stops.join(",")}`);
      await page.getByTestId("chat-list").waitFor();
      await page.waitForTimeout(1500);
      assert.equal(await tour.count(), 0, what("a profile that saw every stop is left alone"));

      // A stop added since: offered by name, flown alone.
      const older = stops.filter((stop) => stop !== "dashboard");
      await page.goto(`${url}/?preview=tour&offer&seen=${older.join(",")}`);
      await offer.waitFor();
      assert.equal(await offer.getByRole("heading").textContent(), "One more thing to see");
      assert.match(await offer.textContent(), /Dashboard/);
      await offer.getByRole("button", { name: "Show me" }).click();
      const only = await arrive(null);
      assert.equal(only.stop, "dashboard");
      assert.equal(only.count, "1 of 1");
      await page.getByTestId("tour-next").click();
      await tour.waitFor({ state: "detached" });

      // A profile without a chat flies what it can; the stops inside a chat stay new.
      await page.goto(`${url}/?preview=tour&offer&nochats`);
      await offer.getByRole("button", { name: "Take the tour" }).click();
      let before = null;
      let outside = 0;
      for (;;) {
        const state = await arrive(before);
        outside += 1;
        before = state.stop;
        const done = (await page.getByTestId("tour-next").textContent()) === "Done";
        await page.getByTestId("tour-next").click();
        if (done) break;
        assert.ok(outside < stops.length, what("the flight ends"));
      }
      await tour.waitFor({ state: "detached" });
      const kept = await seen();
      assert.equal(kept.length, outside, what("only the stops flown are saved"));
      assert.ok(!kept.includes("composer") && !kept.includes("browser"), what("chat stops wait"));
      assert.deepEqual(errors, [], what("no page errors"));
      await context.close();
    }

    // Reduced motion: the wing is simply beside each stop, and nothing animates.
    const still = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      reducedMotion: "reduce",
    });
    const page = await still.newPage();
    page.setDefaultTimeout(12000);
    await page.goto(`${url}/?preview=tour&go`);
    const first = page.locator('[data-testid="tour-card"][data-placed]');
    await first.waitFor();
    assert.equal(
      await first.evaluate((node) => getComputedStyle(node).animationName),
      "none",
      `${engine}: no entrance with reduced motion`,
    );
    assert.equal(
      await page
        .locator(".tour-wing-float")
        .evaluate((node) => getComputedStyle(node).animationName),
      "none",
      `${engine}: the wing does not hover with reduced motion`,
    );
    const placed = await page.evaluate(measure);
    assert.equal(placed.settled, true, `${engine}: the wing is beside its stop at once`);
    await still.close();

    // The smallest window: every stop still has room for its card.
    const small = await browser.newContext({ viewport: { width: 960, height: 640 } });
    const narrow = await small.newPage();
    narrow.setDefaultTimeout(12000);
    await narrow.goto(`${url}/?preview=tour&go`);
    let last = null;
    for (let at = 0; at < 24; at += 1) {
      await narrow.waitForFunction((before) => {
        const node = document.querySelector('[data-testid="tour-card"][data-placed]');
        return node !== null && node.getAttribute("data-stop") !== before;
      }, last ?? "");
      const state = await narrow.evaluate(measure);
      last = state.stop;
      assert.ok(
        state.card.left >= 0 &&
          state.card.top >= 0 &&
          state.card.right <= state.view.width &&
          state.card.bottom <= state.view.height,
        `${engine}: at 960 by 640 the card of ${state.stop} is on screen`,
      );
      const done = (await narrow.getByTestId("tour-next").textContent()) === "Done";
      await narrow.getByTestId("tour-next").click();
      if (done) break;
    }
    await small.close();
    console.log(
      `${engine}: the tour is offered, flies every stop, saves what was seen and leaves no control out`,
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
      [vite, "--host", "127.0.0.1", "--port", "1464", "--strictPort"],
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
