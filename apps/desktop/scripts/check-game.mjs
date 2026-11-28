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

      // Start a game: four small projects of Ghost's own, one press each.
      await open("none");
      const starters = panel.getByTestId("game-starter");
      assert.equal(await starters.count(), 4, `${label}: four starters`);
      assert.match(await starters.nth(0).textContent(), /Godot, 2D\s*A player that runs and jumps/);
      assert.match(await panel.textContent(), /writes over nothing/);
      assert.ok(!(await calls(page)).some((call) => call.startsWith("startProject")));
      await panel.getByRole("button", { name: "Start a Godot, 2D game" }).click();
      await called(page, "startProject:godot2d", label);
      await panel.getByTestId("game-ready").waitFor();
      await panel.getByRole("heading", { name: "Moon Hop" }).waitFor();
      assert.match(
        await panel.getByTestId("game-notice").textContent(),
        /Claude Code is starting again with the game tools/,
      );
      await open("none");
      await panel.getByTestId("game-start-web3d").click();
      await called(page, "startProject:web3d", label);
      await until(
        async () => /moon-hop is made with Web \(three\)/.test(await panel.textContent()),
        `${label}: a web starter shows where it runs`,
      );

      // Engines Ghost names and does not run: each says where the game does run.
      await open("web");
      assert.match(await panel.textContent(), /orbit-run is made with Web \(three\)/);
      assert.match(await panel.textContent(), /runs in Ghost's browser beside the chat/);
      // This fixture has no browser to open, so the pane offers none.
      assert.equal(await panel.getByTestId("game-open-browser").count(), 0);
      await axe(page, `${label} web`);
      // Engines Ghost connects to: the maker's own steps, a look for its tools, its guide.
      await open("unity");
      const connect = panel.getByTestId("game-connect");
      await connect.getByRole("heading", { name: "Connect Unity for Harbor" }).waitFor();
      assert.match(await connect.textContent(), /Ghost does not run Unity itself/);
      assert.equal(await connect.locator(".game-steps > li").count(), 4, `${label}: Unity's steps`);
      assert.match(await connect.textContent(), /unity pipeline install/);
      assert.match(await connect.textContent(), /\/plugin install unity@unity-agent-plugin/);
      await called(page, "probe", label);
      await until(
        async () =>
          /The Unity CLI is not on this computer yet/.test(
            await connect.getByTestId("game-probe").textContent(),
          ),
        `${label}: what the look found`,
      );
      assert.equal(await connect.getByTestId("game-probe").getAttribute("data-ready"), null);
      // Unity is connected from its own CLI: there is no server for Ghost to add.
      assert.equal(await connect.getByTestId("game-connect-add").count(), 0);
      assert.deepEqual(
        await connect.locator("a").evaluateAll((links) => links.map((link) => link.href)),
        [
          "https://docs.unity.com/en-us/ai/unity-plugin/about-unity-plugin",
          "https://docs.unity.com/en-us/unity-cli/use-unity-cli",
        ],
      );
      const looks = (await calls(page)).filter((call) => call === "probe").length;
      await connect.getByTestId("game-probe-again").click();
      await until(
        async () => (await calls(page)).filter((call) => call === "probe").length > looks,
        `${label}: Check again looks again`,
      );
      assert.equal(await panel.getByTestId("game-play").count(), 0);
      await axe(page, `${label} unity`);
      await shot("unity");

      await open("unreal");
      await connect.getByRole("heading", { name: "Connect Unreal Engine for Harbor" }).waitFor();
      assert.match(await connect.textContent(), /http:\/\/127\.0\.0\.1:8000\/mcp/);
      assert.match(await connect.textContent(), /has no sign-in/);
      await until(
        async () => (await connect.getByTestId("game-probe").getAttribute("data-ready")) === "",
        `${label}: Unreal's server answers`,
      );
      assert.ok(!(await calls(page)).includes("connect"), `${label}: nothing added unasked`);
      await connect.getByTestId("game-connect-add").click();
      await called(page, "connect", label);
      await until(
        async () => /^Added\./.test(await connect.getByTestId("game-connected").textContent()),
        `${label}: added to the project`,
      );
      await connect.getByTestId("game-connect-add").click();
      await until(
        async () => /already one of this project's MCP servers/.test(await connect.textContent()),
        `${label}: a second press adds nothing`,
      );
      await axe(page, `${label} unreal`);
      await shot("unreal");

      await open("roblox");
      await connect.getByRole("heading", { name: "Connect Roblox Studio for obby" }).waitFor();
      assert.equal(await connect.locator(".game-steps > li").count(), 3);
      assert.match(await connect.textContent(), /Enable Studio as MCP server/);
      await connect.getByTestId("game-connect-add").waitFor();
      assert.equal(
        await connect.locator("a").getAttribute("href"),
        "https://create.roblox.com/docs/studio/mcp",
      );
      await axe(page, `${label} roblox`);
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

      // Telling the agent. Point: the game freezes, what is under the pointer is outlined and
      // named, a click opens the note, and the note goes with the node, its scene, its script
      // and a picture that marks it.
      await open("running");
      const live = panel.getByTestId("game-canvas");
      await live.waitFor();
      await page.evaluate((chat) => window.gameFixture.frame(chat, 960, 540, 300), CHAT);
      await until(
        async () => (await live.getAttribute("width")) === "960",
        `${label}: a frame to point at`,
      );
      const sent = () => page.evaluate(() => [...window.gameFixture.sent]);
      const pastes = () => page.evaluate(() => [...window.gameFixture.pastes]);
      // A place in the picture's own 960 by 540, as a place on the page.
      const place = async (x, y) => {
        const box = await live.boundingBox();
        return { x: box.x + (x / 960) * box.width, y: box.y + (y / 540) * box.height };
      };
      const point = panel.getByTestId("game-point");
      assert.equal(await point.getAttribute("aria-pressed"), "false");
      await point.click();
      await called(page, "time:true:same", label);
      assert.equal(await point.getAttribute("aria-pressed"), "true");
      await panel.getByTestId("game-pointing").waitFor();
      // Over the player: outlined, with its name. Nothing goes to the game while pointing.
      const inputsBefore = (await inputs(page)).length;
      const player = await place(320, 430);
      await page.mouse.move(player.x, player.y);
      const hover = panel.getByTestId("game-pick-hover");
      await hover.waitFor();
      assert.equal(await hover.textContent(), "Sprite2D");
      {
        const [frame, box] = [await live.boundingBox(), await hover.boundingBox()];
        // The player was drawn at 300, 400, 40 by 60 of 960 by 540.
        assert.ok(
          Math.abs(box.x - (frame.x + (300 / 960) * frame.width)) < 2 &&
            Math.abs(box.width - (40 / 960) * frame.width) < 2,
          `${label}: the outline is on the player: ${JSON.stringify(box)}`,
        );
      }
      await axe(page, `${label} pointing`);
      await shot("pointing");
      // The sky: nothing to outline, and a click says so.
      const sky = await place(600, 120);
      await page.mouse.move(sky.x, sky.y);
      await until(async () => (await hover.count()) === 0, `${label}: nothing outlined in the sky`);
      await page.mouse.click(sky.x, sky.y);
      await until(
        async () =>
          /Nothing is drawn there/.test(
            (await panel
              .getByTestId("game-notice")
              .textContent()
              .catch(() => "")) ?? "",
          ),
        `${label}: a click on nothing says so`,
      );
      assert.equal(await page.getByTestId("game-note").count(), 0);
      // The player: the note opens beside it.
      await page.mouse.click(player.x, player.y);
      const note = page.getByTestId("game-note");
      await note.waitFor();
      await panel.getByTestId("game-pick-marked").waitFor();
      assert.match(await note.textContent(), /^Sprite2D/);
      assert.equal(
        (await inputs(page)).length,
        inputsBefore,
        `${label}: pointing sends the game nothing`,
      );
      const send = note.getByRole("button", { name: /^Send to / });
      assert.equal(await send.isDisabled(), true, `${label}: a pointed note needs words`);
      await axe(page, `${label} point note`);
      await shot("point-note");
      await note.getByRole("textbox").fill("It sinks into the floor when it lands");
      await send.click();
      await until(async () => (await sent()).length === 1, `${label}: the note is sent`);
      {
        const [message] = await sent();
        assert.match(
          message.text,
          /^It sinks into the floor when it lands\n\nPointed at in the game \(Moon Hop\)/,
        );
        assert.match(message.text, /- Sprite2D at \/root\/Main\/Player\/Sprite2D/);
        assert.match(message.text, /Defined in res:\/\/player\.tscn:9 as Sprite2D/);
        assert.match(message.text, /Script: res:\/\/player\.gd \(on \/root\/Main\/Player\)/);
        assert.match(message.text, /48×72 at \(360, 480\) in a 1152×648 picture/);
        assert.match(message.text, /The attached image shows it outlined/);
        assert.equal(message.attachments.length, 1);
        const [picture] = await pastes();
        assert.ok(picture.png && picture.bytes > 500, `${label}: a PNG goes with it`);
      }
      // Sent: the note is gone, the pointer is the game's again, and the game runs as before.
      await until(async () => (await note.count()) === 0, `${label}: the note closes`);
      assert.equal(await point.getAttribute("aria-pressed"), "false");
      await called(page, "time:false:same", label);
      // Esc gives the pointer back without sending anything.
      await point.click();
      await panel.getByTestId("game-pointing").waitFor();
      await live.focus();
      await page.keyboard.press("Escape");
      await until(
        async () => (await point.getAttribute("aria-pressed")) === "false",
        `${label}: Esc ends pointing`,
      );
      assert.equal((await sent()).length, 1);

      // The last 15 seconds: pictures of what was just played and the run's name, with or
      // without words.
      await panel.getByTestId("game-recent").click();
      await called(page, "recent:15", label);
      await note.waitFor();
      assert.match(await note.textContent(), /The last 15 seconds of play/);
      assert.equal(
        await note.getByRole("button", { name: /^Send to / }).isDisabled(),
        false,
        `${label}: the last seconds may go without words`,
      );
      await axe(page, `${label} recent note`);
      await shot("recent-note");
      await note.getByRole("textbox").fill("The jump did nothing here");
      await note.getByRole("button", { name: /^Send to / }).click();
      await until(async () => (await sent()).length === 2, `${label}: the last seconds are sent`);
      {
        const message = (await sent())[1];
        assert.match(
          message.text,
          /^The jump did nothing here\n\nThe last 15 seconds of my play in Moon Hop are in the attached filmstrip: 12 pictures/,
        );
        assert.match(message.text, /The run is 48\.5 seconds long\./);
        assert.match(
          message.text,
          /game_replay and replay "r1": the same seed and the same 212 inputs/,
        );
        assert.equal(message.attachments.length, 1);
        assert.ok((await pastes())[1].png, `${label}: the filmstrip goes as a PNG`);
      }
      // A run that cannot be played again says why instead of naming a replay.
      await page.evaluate(() => {
        window.gameFixture.unrepeatable =
          "This run was given more input than Ghost keeps, so it cannot be played again. The pictures are what it looked like.";
      });
      await panel.getByTestId("game-recent").click();
      await note.waitFor();
      await note.getByRole("button", { name: /^Send to / }).click();
      await until(async () => (await sent()).length === 3, `${label}: sent without a replay`);
      {
        const message = (await sent())[2];
        assert.match(message.text, /cannot be played again/);
        assert.doesNotMatch(message.text, /game_replay/);
      }

      // Tune: the values the scene's script exports, turned while the game runs, then kept.
      const tune = panel.getByTestId("game-tune");
      await tune.click();
      const sheet = panel.getByTestId("game-tune-sheet");
      await sheet.waitFor();
      await called(page, "tunables:.", label);
      assert.equal(await tune.getAttribute("aria-pressed"), "true");
      await until(
        async () => (await sheet.getByTestId("game-tunable").count()) === 4,
        `${label}: four values`,
      );
      assert.match(await sheet.textContent(), /Tune Main/);
      const keep = sheet.getByTestId("game-tune-keep");
      assert.equal(await keep.isDisabled(), true, `${label}: nothing to keep yet`);
      // A range its author gave, and one made from the value itself.
      const jump = sheet.getByRole("slider", { name: "jump_height" });
      assert.deepEqual(
        [
          await jump.getAttribute("min"),
          await jump.getAttribute("max"),
          await jump.getAttribute("step"),
        ],
        ["0", "320", "4"],
      );
      const speed = sheet.getByRole("slider", { name: "speed" });
      assert.deepEqual(
        [await speed.getAttribute("min"), await speed.getAttribute("max")],
        ["0", "400"],
      );
      await axe(page, `${label} tune`);
      await shot("tune");
      await jump.focus();
      await page.keyboard.press("ArrowRight");
      await page.keyboard.press("ArrowRight");
      await until(
        async () => (await calls(page)).includes("set:/root/Main:jump_height=104"),
        `${label}: the game is told the turned value`,
      );
      await sheet.getByRole("switch", { name: "double_jump" }).check();
      await called(page, "set:/root/Main:double_jump=1", label);
      assert.equal(await keep.isDisabled(), false);
      await keep.click();
      await until(async () => (await sent()).length === 4, `${label}: the values are handed over`);
      {
        const message = (await sent())[3];
        assert.match(message.text, /^Keep these values I tuned while Moon Hop ran\./);
        assert.match(message.text, /On Main \(Node2D\) at \/root\/Main, script res:\/\/main\.gd:/);
        assert.match(message.text, /- jump_height = 104 \(was 96\)/);
        assert.match(message.text, /- double_jump = true \(was false\)/);
        assert.doesNotMatch(message.text, /speed/);
        assert.equal(message.attachments.length, 0);
      }
      await until(async () => await keep.isDisabled(), `${label}: kept values are the new start`);
      // Reset goes back to what was kept.
      await jump.focus();
      await page.keyboard.press("ArrowLeft");
      await called(page, "set:/root/Main:jump_height=100", label);
      await sheet.getByTestId("game-tune-reset").click();
      await called(page, "set:/root/Main:jump_height=104", label);
      // Something else, by pointing at it: a node whose script exports nothing says so.
      await sheet.getByTestId("game-tune-point").click();
      await panel.getByTestId("game-pointing").waitFor();
      assert.equal(await sheet.count(), 0, `${label}: the sheet is out of the way while pointing`);
      const again = await place(320, 430);
      await page.mouse.click(again.x, again.y);
      await called(page, "tunables:/root/Main/Player", label);
      await sheet.getByTestId("game-tune-none").waitFor();
      assert.match(await sheet.textContent(), /Tune Player/);
      await tune.click();
      await until(async () => (await sheet.count()) === 0, `${label}: the sheet closes`);

      // A controller: its buttons and sticks go to the game in Godot's numbers while the
      // picture has the keyboard, and are let go when it loses it.
      await page.evaluate(() => {
        const state = {
          buttons: Array.from({ length: 17 }, () => ({ pressed: false, value: 0 })),
          axes: [0, 0, 0, 0],
        };
        window.fakePad = state;
        navigator.getGamepads = () => [{ connected: true, mapping: "standard", ...state }];
        window.dispatchEvent(new Event("gamepadconnected"));
      });
      await panel.getByTestId("game-pad").waitFor();
      await live.click({ position: { x: 5, y: 5 } });
      const padAt = (await inputs(page)).length;
      await page.evaluate(() => {
        window.fakePad.buttons[0] = { pressed: true, value: 1 };
        window.fakePad.buttons[4] = { pressed: true, value: 1 };
        window.fakePad.buttons[7] = { pressed: true, value: 0.5 };
        window.fakePad.axes[0] = -0.71;
        window.fakePad.axes[1] = 0.05;
      });
      const padInputs = async () =>
        (await inputs(page))
          .slice(padAt)
          .filter((event) => event.t === "pad" || event.t === "axis");
      await until(async () => (await padInputs()).length >= 4, `${label}: the controller is heard`);
      assert.deepEqual(
        (await padInputs()).map((event) =>
          event.t === "pad"
            ? `pad ${event.button} ${event.down}`
            : `axis ${event.axis} ${event.value}`,
        ),
        ["pad 0 true", "pad 9 true", "axis 0 -0.7", "axis 5 0.5"],
        `${label}: buttons in Godot's numbers, a stick in twentieths past its dead zone, a trigger as an axis`,
      );
      await page.getByTestId("chat-game").focus();
      await until(
        async () => (await padInputs()).length >= 8,
        `${label}: what the controller held is let go`,
      );
      assert.deepEqual(
        (await padInputs())
          .slice(4)
          .map((event) =>
            event.t === "pad"
              ? `pad ${event.button} ${event.down}`
              : `axis ${event.axis} ${event.value}`,
          )
          .sort(),
        ["axis 0 0", "axis 5 0", "pad 0 false", "pad 9 false"],
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
        15,
        `${label}: fifteen tools`,
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
