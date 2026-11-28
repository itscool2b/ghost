/**
 * Ghost's game clock for web games (src-tauri/src/browser/game.js, docs/game.md "Web games"), in
 * real browser engines. The script is added to a page before the page's own, as the browser
 * beside the chat adds it, and a small canvas game is played through it: asleep until the page is
 * marked as a game, live in real time once it is, frozen on request, stepped for an exact stretch
 * of game time with keys and a click, timers on the game's clock, a filmstrip and a last frame,
 * waits on a condition, the scene outline, and back to real time. No dev server: the game is
 * served to the engine by this script.
 * Usage: node scripts/check-web-game.mjs [--engines chromium,webkit]
 */
import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { chromium, webkit } from "playwright";

const { values } = parseArgs({
  options: { engines: { type: "string", default: "chromium,webkit" } },
});
const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const clockScript = resolve(app, "src-tauri/src/browser/game.js");
const ORIGIN = "http://game.test";

/** A game as an agent would write one: a loop on requestAnimationFrame, keys, a click, timers. */
const GAME = `<!doctype html>
<meta charset="utf-8">
<title>Moon Hop</title>
<style>html, body { margin: 0; background: #111; } canvas { display: block; }</style>
<canvas id="game" width="400" height="300"></canvas>
<script>
  const canvas = document.getElementById("game");
  const context = canvas.getContext("2d");
  const game = (window.game = { x: 100, frames: 0, clicks: [], keys: new Set(), codes: [] });
  window.ticks = 0;
  window.late = false;
  window.started = { now: performance.now(), date: Date.now() };
  addEventListener("keydown", (event) => { game.keys.add(event.code); game.codes.push(event.keyCode); });
  addEventListener("keyup", (event) => game.keys.delete(event.code));
  canvas.addEventListener("pointerdown", (event) => {
    const box = canvas.getBoundingClientRect();
    game.clicks.push([Math.round(event.clientX - box.left), Math.round(event.clientY - box.top)]);
  });
  setTimeout(() => { window.late = true; }, 300);
  setInterval(() => { window.ticks += 1; }, 100);
  let last = performance.now();
  function loop(now) {
    const delta = (now - last) / 1000;
    last = now;
    if (game.keys.has("ArrowRight")) game.x += 200 * delta;
    if (game.keys.has("KeyA")) game.x -= 200 * delta;
    game.frames += 1;
    context.fillStyle = "#14161c";
    context.fillRect(0, 0, 400, 300);
    context.fillStyle = "#e8eaf0";
    context.fillRect(game.x, 200, 20, 30);
    requestAnimationFrame(loop);
  }
  requestAnimationFrame(loop);
</script>`;

async function check(engine, launcher) {
  const browser = await launcher.launch();
  const what = (text) => `${engine}: ${text}`;
  try {
    const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
    await context.addInitScript({ path: clockScript });
    await context.route(`${ORIGIN}/**`, (route) =>
      route.fulfill({ contentType: "text/html", body: GAME }),
    );
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const clock = (call) => page.evaluate(`window.__ghostGame.${call}`);
    const job = async (start) => {
      const started = await clock(start);
      assert.ok(
        typeof started.id === "number",
        what(`${start} started: ${JSON.stringify(started)}`),
      );
      for (let tries = 0; tries < 400; tries += 1) {
        const answer = await clock(`poll(${started.id})`);
        if (answer !== null) return answer;
        await page.waitForTimeout(10);
      }
      throw new Error(what(`${start} never finished`));
    };
    const x = () => page.evaluate(() => window.game.x);

    // Asleep: a page nobody marked keeps its own time, and nothing of it is changed.
    await page.goto(`${ORIGIN}/`);
    assert.equal(await clock("armed()"), false, what("asleep until marked"));
    assert.equal(
      await page.evaluate(() => /native code/.test(String(window.requestAnimationFrame))),
      true,
      what("the page's own requestAnimationFrame is untouched"),
    );
    await page.waitForFunction(() => window.game.frames > 5);
    assert.deepEqual(await clock("act([{ ms: 100 }], 0, 800)"), { error: "not armed" });
    assert.deepEqual(await clock("perf(10)"), { error: "not armed" });
    // A picture can still be read from a page that keeps its own time.
    assert.match((await clock("look(800)")).data, /^\/9j\//, what("a picture without the clock"));

    // Marked and loaded again: the clock is the game's, and live it follows real time.
    assert.equal(await clock("arm(true)"), true);
    await page.reload();
    assert.equal(await clock("armed()"), true, what("armed after the load"));
    await page.waitForFunction(() => window.game.frames > 20);
    await page.waitForFunction(() => window.late === true && window.ticks >= 5, null, {
      timeout: 5000,
    });
    await page.waitForFunction(() => window.__ghostGame.status().fps > 20, null, { timeout: 5000 });
    const live = await clock("status()");
    assert.deepEqual(live.canvas, [400, 300]);
    assert.equal(live.frozen, false);

    // Frozen: no frame, no timer, and the page's clocks stand still.
    assert.deepEqual(await clock("time(true)"), { frozen: true, speed: 1 });
    const still = await page.evaluate(() => ({
      frames: window.game.frames,
      ticks: window.ticks,
      now: performance.now(),
      date: Date.now(),
      fresh: new Date().getTime(),
    }));
    await page.waitForTimeout(400);
    const later = await page.evaluate(() => ({
      frames: window.game.frames,
      ticks: window.ticks,
      now: performance.now(),
      date: Date.now(),
      fresh: new Date().getTime(),
    }));
    assert.deepEqual(later, still, what("nothing moves while frozen"));
    assert.ok(Math.abs(still.fresh - still.date) <= 1, what("new Date() reads the game's clock"));

    // An action: half a second of ArrowRight is thirty frames and exactly 100 pixels, each time.
    for (const round of [1, 2]) {
      const before = await x();
      const ticks = await page.evaluate(() => window.ticks);
      const done = await job('act([{ press: "ArrowRight", ms: 500 }], 6, 800)');
      assert.equal(done.frames, 30, what(`round ${round}: thirty frames`));
      assert.ok(Math.abs(done.game_ms - 500) < 0.001, what("500 ms of game time"));
      assert.ok(
        Math.abs((await x()) - before - 100) < 1e-6,
        what(`round ${round}: exactly 100 px`),
      );
      // Timers ran on the game's clock: five ticks of a 100 ms interval in 500 ms.
      assert.equal(
        (await page.evaluate(() => window.ticks)) - ticks,
        5,
        what("timers keep game time"),
      );
      assert.equal(done.strip_frames, 6, what("six frames in the filmstrip"));
      assert.match(done.strip.data, /^\/9j\//, what("the filmstrip is a JPEG"));
      // Six cells of 360 across, four to a row, with a gap: two rows.
      assert.equal(done.strip.width, 4 * 360 + 3 * 4);
      assert.equal(done.strip.height, 2 * 270 + 4);
      assert.deepEqual([done.final.width, done.final.height], [400, 300]);
      assert.match(done.final.data, /^\/9j\//);
      assert.deepEqual(done.errors, []);
      assert.equal((await clock("status()")).frozen, true, what("frozen again after the action"));
      assert.equal(
        await page.evaluate(() => window.game.keys.size),
        0,
        what("the key came up with the step"),
      );
    }
    // The old numeric codes are there for games that read them.
    assert.ok((await page.evaluate(() => window.game.codes)).includes(39), what("keyCode 39"));

    // Several steps, a key by Godot's name for it, a letter, and a click in the picture's pixels.
    const start = await x();
    const mixed = await job(
      'act([{ press: "ui_right", ms: 250 }, { press: "a", ms: 500 }, { click: [200, 150], ms: 50 }], 0, 800)',
    );
    assert.equal(mixed.frames, 15 + 30 + 3);
    assert.equal(mixed.strip, null, what("no filmstrip when none was asked for"));
    assert.ok(Math.abs((await x()) - (start + 50 - 100)) < 1e-6, what("right then left"));
    assert.deepEqual(
      await page.evaluate(() => window.game.clicks),
      [[200, 150]],
      what("the click"),
    );

    // What cannot be played says so, and plays nothing.
    const frames = await page.evaluate(() => window.game.frames);
    assert.match((await clock('act([{ press: "jump_button", ms: 100 }], 0, 800)')).error, /no key/);
    assert.match((await clock("act([], 0, 800)")).error, /1 to 64 steps/);
    assert.match((await clock("act([{ ms: 999999999 }], 0, 800)")).error, /two minutes/);
    assert.equal(
      await page.evaluate(() => window.game.frames),
      frames,
      what("refusals play nothing"),
    );

    // Waiting on a condition, on the game's clock.
    const ticks = await page.evaluate(() => window.ticks);
    const met = await job(`wait("window.ticks >= ${ticks + 3}", 5000, 800)`);
    assert.equal(met.met, true);
    assert.ok(met.game_ms > 190 && met.game_ms <= 310, what(`three ticks took ${met.game_ms} ms`));
    const never = await job('wait("window.game.x > 99999", 200, 800)');
    assert.equal(never.met, false);
    assert.equal(never.frames, 12);
    const broken = await job('wait("window.no.such.thing", 200, 800)');
    assert.match(broken.failed, /undefined|null/);
    assert.match((await clock('wait("this is not code", 200, 800)')).error, /could not be read/);

    // Reading and changing the game's own state.
    assert.equal((await clock('eval("window.game.x = 250")')).value, "250");
    assert.equal(
      (await clock('eval("const twice = window.game.x * 2; return twice;")')).value,
      "500",
    );
    assert.equal(JSON.parse((await clock('eval("({ a: [1, 2] })")')).value).a[1], 2);

    // A picture while frozen, at the size asked for.
    const seen = await clock("look(200)");
    assert.deepEqual([seen.width, seen.height], [200, 150]);

    // Measuring leaves the game as it was.
    const measured = await clock("perf(30)");
    assert.equal(measured.frames, 30);
    assert.ok(measured.could_run_at_fps > 60, what("a tiny game runs faster than a display"));
    assert.equal((await clock("status()")).frozen, true);

    // The outline: nothing to list here, until an engine announces a scene.
    const bare = await clock("tree()");
    assert.deepEqual([bare.engine, bare.nodes.length, bare.title], [null, 0, "Moon Hop"]);
    await page.evaluate(() => {
      const scene = {
        isScene: true,
        type: "Scene",
        name: "World",
        position: { x: 0, y: 0, z: 0 },
        visible: true,
        children: [
          {
            type: "Mesh",
            name: "Player",
            position: { x: 1, y: 2.345, z: 3 },
            visible: false,
            speed: 4,
            children: [],
          },
        ],
      };
      window.__THREE_DEVTOOLS__.dispatchEvent(new CustomEvent("observe", { detail: scene }));
    });
    const listed = await clock("tree()");
    assert.equal(listed.engine, "three");
    assert.deepEqual(
      listed.nodes.map((node) => [node.ref, node.depth, node.name, node.kind, node.visible]),
      [
        ["w1", 0, "World", "Scene", true],
        ["w2", 1, "Player", "Mesh", false],
      ],
    );
    assert.deepEqual(listed.nodes[1].at, [1, 2.35, 3]);
    const player = await clock('inspect("w2")');
    assert.equal(player.values.position, "x 1, y 2.35, z 3");
    assert.equal(player.values.speed, "4");
    assert.match((await clock('inspect("w99")')).error, /No such thing/);

    // Resumed, it follows real time again at the speed asked for.
    assert.deepEqual(await clock("time(false, 2)"), { frozen: false, speed: 2 });
    const moving = await page.evaluate(() => window.game.frames);
    await page.waitForFunction((count) => window.game.frames > count + 10, moving);

    // Unmarked and loaded again: the page keeps its own time, as before.
    assert.equal(await clock("arm(false)"), true);
    await page.reload();
    assert.equal(await clock("armed()"), false);
    assert.equal(
      await page.evaluate(() => /native code/.test(String(window.requestAnimationFrame))),
      true,
    );
    await page.waitForFunction(() => window.game.frames > 5);

    assert.deepEqual(errors, [], what("page errors"));
    console.log(
      `${engine}: a web game sleeps until marked, runs live, freezes, steps exactly with keys, clicks and timers, and comes back as a filmstrip`,
    );
  } finally {
    await browser.close();
  }
}

for (const [engine, launcher] of Object.entries({ chromium, webkit })) {
  if (values.engines.split(",").includes(engine)) await check(engine, launcher);
}
