/**
 * The game beside the chat (`?preview=game`, docs/game.md): a folder with no game and turning
 * Game on for it, the engines Ghost names but does not run, getting Godot (the offer, the
 * download, a failure, a Godot that is too old), a game ready, starting, running and ended, the
 * pane's own input (keys down and up by their code, every key released when the pane loses the
 * keyboard, the mouse in the game's own pixels), its controls, the errors log, the agent's line,
 * an agent opening the pane, the header's button and its shortcut, a narrow window, and Game
 * under Settings (`?preview=customize`), in both appearances, with axe. There is no engine here:
 * the checks against a real Godot are crates/ghost-game/tests/godot.rs.
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

const CHAT = "game-fixture-chat";

async function axe(page, label, scope = "aside[aria-label=Game]") {
  // Entrances are short and finite: judge the colours once they have ended. The wait is bounded,
  // since WebKit keeps some finished-looking animations pending for ever.
  await page.evaluate(() =>
    Promise.race([
      Promise.all(
        document
          .getAnimations()
          .filter((animation) => animation.effect?.getComputedTiming().iterations !== Infinity)
          .map((animation) => animation.finished.catch(() => {})),
      ),
      new Promise((done) => setTimeout(done, 1500)),
    ]),
  );
  const result = await new AxeBuilder({ page }).include(scope).analyze();
  assert.deepEqual(
    result.violations.map(
      (violation) => `${violation.id}: ${violation.nodes.map((node) => node.target).join(" ")}`,
    ),
    [],
    `${label}: axe`,
  );
}

async function until(fn, label, ms = 5000) {
  const deadline = Date.now() + ms;
  for (;;) {
    if (await fn()) return;
    if (Date.now() > deadline) throw new Error(`${label}: timed out`);
    await new Promise((done) => setTimeout(done, 50));
  }
}

const calls = (page) => page.evaluate(() => [...window.gameFixture.calls]);
const inputs = (page) => page.evaluate(() => [...window.gameFixture.inputs]);
const called = (page, call, label) =>
  until(async () => (await calls(page)).includes(call), `${label}: ${call}`);

export async function checkGame(browser, url, out, engine) {
  for (const theme of ["light", "dark"]) {
    const label = `${engine} ${theme}`;
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      colorScheme: theme,
      reducedMotion: "reduce",
    });
    try {
      const page = await context.newPage();
      const errors = [];
      page.on("pageerror", (error) => errors.push(error.message));
      const open = async (state, extra = "") => {
        await page.goto(`${url}/?preview=game&theme=${theme}&state=${state}${extra}`);
        await page.getByTestId("game-panel").waitFor();
      };
      const shot = (name) =>
        page.screenshot({ path: resolve(out, `${engine}-${theme}-${name}.png`) });
      const panel = page.getByTestId("game-panel");

      // A folder with no game: the pane says so, and offers the tools for starting one.
      await open("none");
      await panel.getByRole("heading", { name: "No game here yet" }).waitFor();
      assert.equal(await panel.getByTestId("game-play").count(), 0, `${label}: nothing to play`);
      await axe(page, `${label} none`);
      await shot("none");
      await panel.getByTestId("game-turn-on").click();
      await called(page, "turnOn:true", label);
      await until(
        async () => /Game is on for this chat/.test(await panel.textContent()),
        `${label}: the pane says Game is on`,
      );
      assert.match(
        await panel.getByTestId("game-notice").textContent(),
        /Claude Code is starting again with the game tools/,
      );
      assert.equal(await panel.getByTestId("game-turn-on").count(), 0);

      // Engines Ghost names and does not run: each says where the game does run.
      await open("web");
      assert.match(await panel.textContent(), /orbit-run is made with Web \(three\)/);
      assert.match(await panel.textContent(), /runs in Ghost's browser beside the chat/);
      // This fixture has no browser to open, so the pane offers none.
      assert.equal(await panel.getByTestId("game-open-browser").count(), 0);
      await axe(page, `${label} web`);
      await open("unity");
      assert.match(await panel.textContent(), /Harbor is made with Unity/);
      assert.match(await panel.textContent(), /Ghost does not run Unity itself/);
      assert.equal(await panel.getByTestId("game-play").count(), 0);
      await axe(page, `${label} unity`);
      await shot("unity");
      await open("bevy");
      assert.match(await panel.textContent(), /Ghost has no pane for Bevy games/);
      await open("unsupported");
      assert.match(await panel.getByTestId("game-unsupported").textContent(), /written in C#/);
      assert.equal(await panel.getByTestId("game-play").count(), 0);
      await axe(page, `${label} unsupported`);

      // Godot is not here: the pane says what getting it means before anything is fetched.
      await open("get");
      await panel.getByRole("heading", { name: "This game needs Godot" }).waitFor();
      assert.match(
        await panel.getByTestId("game-get-godot").textContent(),
        /Godot 4\.7\.2, 82 MB, from Godot's releases on GitHub, checked against the hash/,
      );
      assert.ok(!(await calls(page)).includes("fetchStart"), `${label}: nothing fetched unasked`);
      assert.equal(await panel.getByTestId("game-play").count(), 0, `${label}: no Play yet`);
      await axe(page, `${label} get`);
      await shot("get");
      await panel.getByTestId("game-get").click();
      await called(page, "fetchStart", label);
      await panel.getByTestId("game-fetching").waitFor();
      assert.match(await panel.getByTestId("game-fetch-progress").textContent(), /21 MB of 82 MB/);
      await axe(page, `${label} fetching`);
      await shot("fetching");
      await panel.getByRole("button", { name: "Cancel", exact: true }).click();
      await called(page, "fetchCancel", label);
      await panel.getByTestId("game-get").waitFor();
      await open("unpacking");
      assert.match(await panel.getByTestId("game-fetch-progress").textContent(), /unpacking/);
      assert.equal(await panel.getByRole("button", { name: "Cancel", exact: true }).count(), 0);
      await open("fetch-failed");
      assert.match(await panel.getByRole("alert").textContent(), /did not match the hash/);
      await panel.getByRole("button", { name: "Try again" }).click();
      await called(page, "fetchStart", `${label} again`);
      await open("too-old");
      await panel.getByRole("heading", { name: "This game needs another Godot" }).waitFor();
      assert.match(await panel.textContent(), /newer than the Godot here \(4\.3\)/);
      await axe(page, `${label} too old`);
      await open("looking");
      assert.match(await panel.getByTestId("game-looking").textContent(), /Looking for Godot/);

      // Ready: Play starts it, and Godot's editor is one press away.
      await open("ready");
      await panel.getByRole("heading", { name: "Moon Hop" }).waitFor();
      assert.match(await panel.getByTestId("game-ready").textContent(), /A Godot 4\.7 project/);
      await axe(page, `${label} ready`);
      await shot("ready");
      await panel.getByTestId("game-open-editor").click();
      await called(page, "openEditor", label);
      await panel.getByTestId("game-play").click();
      await called(page, "run:main", label);
      const canvas = panel.getByTestId("game-canvas");
      await canvas.waitFor();
      await called(page, "watch", label);

      // Running: a frame is the game's own picture, whole.
      await page.evaluate((chat) => window.gameFixture.frame(chat, 960, 540, 300), CHAT);
      await until(
        async () => (await canvas.getAttribute("width")) === "960",
        `${label}: the frame is drawn`,
      );
      assert.equal(await canvas.getAttribute("height"), "540");
      assert.match(await panel.getByTestId("game-fps").textContent(), /60 fps · 1152×648/);
      await panel.getByTestId("game-hint").waitFor();
      await axe(page, `${label} running`);
      await shot("running");

      // Keys go down once and up once, by their code; chords and the web view's keys stay here.
      await canvas.click({ position: { x: 5, y: 5 } });
      const before = (await inputs(page)).length;
      await page.keyboard.down("ArrowRight");
      await page.keyboard.down("ArrowRight");
      await page.keyboard.down("Space");
      await page.keyboard.up("ArrowRight");
      await until(
        async () => (await inputs(page)).length >= before + 3,
        `${label}: three key events`,
      );
      const keys = (await inputs(page)).slice(before).filter((event) => event.t === "key");
      assert.deepEqual(
        keys.map((event) => `${event.code}:${event.down}`),
        ["ArrowRight:true", "Space:true", "ArrowRight:false"],
        `${label}: a held key goes down once`,
      );
      assert.equal(keys[1].text, " ", `${label}: what a key types goes with it`);
      assert.equal(keys[0].text, "", `${label}: a key's name is not text`);
      // The pane loses the keyboard with Space still down: the game hears it come up.
      await page.getByTestId("chat-game").focus();
      await until(
        async () =>
          (await inputs(page)).some(
            (event) => event.t === "key" && event.code === "Space" && !event.down,
          ),
        `${label}: a held key is released when the pane loses the keyboard`,
      );
      await page.keyboard.up("Space");
      // A chord is Ghost's, not the game's.
      await canvas.click({ position: { x: 5, y: 5 } });
      const chordAt = (await inputs(page)).length;
      await page.keyboard.press("Control+KeyK");
      await page.keyboard.press("F5").catch(() => {});
      await canvas.waitFor();
      assert.deepEqual(
        (await inputs(page)).slice(chordAt).filter((event) => event.t === "key"),
        [],
        `${label}: chords and F5 never reach the game`,
      );

      // The mouse, in the game's own pixels: the picture is 960 wide and the game 1152.
      const box = await canvas.boundingBox();
      const mouseAt = (await inputs(page)).length;
      await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      await until(
        async () =>
          (await inputs(page))
            .slice(mouseAt)
            .some((event) => event.t === "mouse" && event.down === false),
        `${label}: a click is a press and a release`,
      );
      const press = (await inputs(page))
        .slice(mouseAt)
        .find((event) => event.t === "mouse" && event.down);
      assert.equal(press.button, "left");
      assert.ok(
        Math.abs(press.x - 576) <= 3 && Math.abs(press.y - 324) <= 3,
        `${label}: ${press.x},${press.y} is the game's middle`,
      );

      // The controls.
      await panel.getByTestId("game-pause").click();
      await called(page, "time:true:same", label);
      await panel.getByTestId("game-frozen").waitFor();
      assert.match(await panel.getByTestId("game-fps").textContent(), /^Frozen/);
      await panel.getByTestId("game-step").click();
      await called(page, "step", label);
      await panel.getByRole("button", { name: "Resume", exact: true }).click();
      await called(page, "time:false:same", label);
      assert.ok(await panel.getByTestId("game-step").isDisabled(), `${label}: no step while live`);
      await panel.getByTestId("game-speed").click();
      await called(page, "time:same:2", label);
      await until(
        async () => (await panel.getByTestId("game-speed").textContent()) === "2x",
        `${label}: the speed shows`,
      );
      await panel.getByTestId("game-speed").click();
      await called(page, "time:same:0.25", label);
      await panel.getByTestId("game-sound").click();
      await called(page, "sound:true", label);
      await panel.getByRole("button", { name: "Mute the game" }).waitFor();
      await panel.getByTestId("game-picture").click();
      await called(page, "picture", label);
      assert.match(await panel.getByTestId("game-notice").textContent(), /Saved to .*ghost-game-/);
      await panel.getByTestId("game-window").click();
      await called(page, "window:true", label);
      await panel.getByRole("button", { name: "Put the game's window away" }).waitFor();
      await panel.getByTestId("game-restart").click();
      await called(page, "run:res://main.tscn", label);
      await panel.getByTestId("game-stop").click();
      await called(page, "stop", label);
      await panel.getByTestId("game-ended").waitFor();
      assert.match(await panel.getByTestId("game-ended-why").textContent(), /Stopped\./);
      await panel.getByRole("button", { name: "Play again" }).waitFor();

      // Errors: the count in the header opens the log, with each error at its file and line.
      await open("errors");
      const chip = panel.getByTestId("game-errors");
      assert.match(await chip.textContent(), /3 errors/);
      await chip.click();
      const log = panel.getByTestId("game-log");
      await log.waitFor();
      await called(page, "log", label);
      await until(
        async () => (await log.locator(".game-log-errors > li").count()) === 2,
        `${label}: both entries listed`,
      );
      assert.match(await log.textContent(), /Nonexistent function 'hop'.*\(×3\)/s);
      assert.match(await log.textContent(), /res:\/\/player\.gd:12/);
      assert.match(await log.getByTestId("game-log-lines").textContent(), /fixture ready/);
      await axe(page, `${label} log`);
      await shot("log");
      await log.getByRole("button", { name: "Close the log" }).click();
      await log.waitFor({ state: "detached" });

      await open("ended");
      assert.match(await panel.getByTestId("game-ended-why").textContent(), /stopped by itself/);
      assert.match(await panel.getByTestId("game-last-error").textContent(), /player\.gd:12/);
      await axe(page, `${label} ended`);
      await shot("ended");

      await open("starting");
      await panel.getByRole("heading", { name: "Starting Moon Hop" }).waitFor();
      await axe(page, `${label} starting`);

      // The agent at work: the header says what it does, and the picture says why nothing moves.
      await open("agent");
      await page.evaluate((chat) => window.gameFixture.frame(chat), CHAT);
      await panel.getByTestId("game-agent").waitFor();
      assert.match(
        await panel.getByTestId("game-agent").textContent(),
        /Claude Code · Holding ui_right/,
      );
      await until(
        async () => /Frozen while the agent plays/.test(await panel.textContent()),
        `${label}: frozen for the agent`,
      );
      await axe(page, `${label} agent`);
      await shot("agent");

      // An agent that started before the folder was a game: one press gives it the tools.
      await open("no-tools");
      await panel.getByTestId("game-tools-offer").waitFor();
      await axe(page, `${label} no tools`);
      await panel.getByTestId("game-give-tools").click();
      await called(page, "turnOn:true", `${label} tools`);
      await panel.getByTestId("game-tools-offer").waitFor({ state: "detached" });
      // An agent on another computer can never have them: nothing is offered.
      await open("remote");
      await panel.getByTestId("game-ready").waitFor();
      assert.equal(await panel.getByTestId("game-tools-offer").count(), 0);

      // The header's button and Ctrl+Shift+G close and open the pane.
      await open("ready");
      const toggle = page.getByTestId("chat-game");
      assert.equal(await toggle.getAttribute("aria-expanded"), "true");
      await toggle.click();
      await panel.waitFor({ state: "detached" });
      assert.equal(await toggle.getAttribute("aria-expanded"), "false");
      await page.keyboard.press("Control+Shift+G");
      await panel.waitFor();
      await panel.getByRole("button", { name: "Close the game" }).click();
      await panel.waitFor({ state: "detached" });

      // An agent's tool opens the pane; the button says so while Changes holds the side.
      await page.goto(`${url}/?preview=game&theme=${theme}&state=running&closed`);
      await page.getByTestId("chat-view").waitFor();
      // The button shows once the page knows the chat has a game; a call before that is from
      // before this window, and opens nothing.
      await page.getByTestId("chat-game").waitFor();
      assert.equal(await page.getByTestId("game-panel").count(), 0, `${label}: starts closed`);
      await page.evaluate((chat) => window.gameFixture.summon(chat), CHAT);
      await panel.waitFor();
      // The person closes it: the agent's next call in the same stretch leaves it closed.
      await panel.getByRole("button", { name: "Close the game" }).click();
      await panel.waitFor({ state: "detached" });
      await page.evaluate((chat) => window.gameFixture.summon(chat), CHAT);
      await new Promise((done) => setTimeout(done, 300));
      assert.equal(await page.getByTestId("game-panel").count(), 0, `${label}: stays closed`);
      // While the agent is at it, the button says so.
      await page.evaluate(
        (chat) => window.gameFixture.game(chat, { agent: "Holding ui_right" }),
        CHAT,
      );
      await until(
        async () => (await page.getByTestId("chat-game").getAttribute("aria-label")) !== "Game",
        `${label}: the button changes while the agent plays`,
      );
      assert.equal(
        await page.getByTestId("chat-game").getAttribute("aria-label"),
        "Game: Claude Code is playing it",
        `${label}: the button says the agent plays`,
      );

      // A narrow window: the pane covers the chat's work area instead of sitting beside it.
      await open("ready");
      await page.setViewportSize({ width: 900, height: 800 });
      await until(
        async () => (await panel.getAttribute("data-full")) === "",
        `${label}: full on a narrow window`,
      );
      await axe(page, `${label} narrow`);
      await shot("narrow");
      await page.setViewportSize({ width: 1440, height: 900 });

      // Game turned off: no button and no pane.
      await page.goto(`${url}/?preview=game&theme=${theme}&state=off`);
      await page.getByTestId("chat-view").waitFor();
      assert.equal(await page.getByTestId("chat-game").count(), 0, `${label}: no button when off`);
      assert.equal(await page.getByTestId("game-panel").count(), 0, `${label}: no pane when off`);

      // Settings > Game: the switch, what agents get, and Godot.
      await page.goto(`${url}/?preview=customize&theme=${theme}`);
      await page.getByRole("button", { name: "Game", exact: true }).click();
      const settings = page.getByTestId("game-settings");
      await settings.waitFor();
      assert.equal(await settings.getByRole("switch", { name: "Game" }).isChecked(), true);
      assert.equal(
        await settings.locator(".plugins-universal-tools li").count(),
        14,
        `${label}: fourteen tools`,
      );
      assert.match(await settings.textContent(), /only in a chat whose folder holds a game/);
      assert.match(await settings.textContent(), /runs code inside your game/);
      assert.match(await settings.getByTestId("game-godot").textContent(), /Godot 4\.7\.2/);
      // It comes with Ghost: nothing to add or remove, only the switch.
      assert.equal(
        await settings.getByRole("button", { name: /^(Add|Remove|Install|Uninstall)/ }).count(),
        0,
        `${label}: no add or remove`,
      );
      await axe(page, `${label} settings`, "[data-testid=game-settings]");
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-settings.png`) });
      await settings.getByRole("switch", { name: "Game" }).uncheck();
      await until(
        async () => /Off for every agent/.test(await settings.textContent()),
        `${label}: off says what stops`,
      );
      assert.equal(await settings.locator(".plugins-universal-tools").count(), 0);

      assert.deepEqual(errors, [], `${label}: page errors`);
    } finally {
      await context.close();
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/game-panel/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkGame(browser, process.env.CHECK_URL ?? "http://localhost:1476", out, engine);
      console.log(
        `${engine}: the Game pane's engines, Godot offer, play, input, controls, log, agent line and Settings > Game passed in both appearances`,
      );
    } finally {
      await browser.close();
    }
  }
}
