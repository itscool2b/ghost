/**
 * Scripted checks for Settings' Remote access page (`?preview=settings&page=remote`). Nothing
 * connects anywhere: the fixture stands in for the service, and `window.remoteFixture` plays the
 * device that opens a pairing link.
 */
import assert from "node:assert/strict";
import { resolve } from "node:path";
import AxeBuilder from "@axe-core/playwright";

const settled = (page) =>
  page.waitForFunction(() =>
    document
      .getAnimations()
      .every((a) => a.playState !== "running" || a.effect?.getTiming().iterations === Infinity),
  );

export async function checkRemote(browser, url, out, engine) {
  for (const colorScheme of ["light", "dark"]) {
    const context = await browser.newContext({
      viewport: { width: 1180, height: 820 },
      colorScheme,
      permissions: engine === "chromium" ? ["clipboard-read", "clipboard-write"] : [],
    });
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const calls = () => page.evaluate(() => window.remoteFixture.calls());

    // Off: nothing is paired, and the page says nothing can be reached.
    await page.goto(`${url}/?preview=settings&page=remote`);
    await page.getByRole("heading", { level: 1, name: "Remote access" }).waitFor();
    const toggle = page.getByRole("switch", { name: "Remote access" });
    assert.equal(await toggle.isChecked(), false);
    await page.getByText("Off. Nothing on this computer can be reached from outside.").waitFor();
    assert.equal(await page.getByRole("button", { name: /^Remove / }).count(), 0);
    // With nothing paired or linked, each list says so where its rows will be.
    await page.getByText("No devices yet", { exact: true }).waitFor();
    await page.getByText("No bots linked", { exact: true }).waitFor();
    // What the relay can read is one line away, folded.
    const fine = page.locator("details.settings-more");
    assert.equal(await fine.evaluate((node) => node.open), false);
    await fine.getByText("What the relay can see").click();
    await fine.getByText("encrypted end to end", { exact: false }).waitFor();
    await settled(page);
    assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);

    // Reading what a bot link allows turns nothing on: closing the dialog leaves it off.
    await page.getByRole("button", { name: "Link a bot", exact: true }).click();
    await page.getByRole("dialog", { name: "Link a bot" }).waitFor();
    await page.keyboard.press("Escape");
    await page.getByRole("dialog", { name: "Link a bot" }).waitFor({ state: "detached" });
    assert.deepEqual(await calls(), []);
    assert.equal(await toggle.isChecked(), false);

    // With no bot linked, the list names the four assistants. Picking one opens the dialog for
    // it, and the link's name follows the assistant until its owner writes one.
    for (const assistant of ["Grok Bot", "Muse", "Hermes", "OpenClaw"])
      await page.getByRole("button", { name: new RegExp(`^${assistant}`) }).waitFor();
    assert.equal(await page.locator(".remote-bot-pick .g-agent-mark").count(), 4);
    await page.getByTestId("remote-link-hermes").click();
    const picked = page.getByRole("dialog", { name: "Link a bot" });
    const assistants = picked.getByRole("group", { name: "Your assistant" });
    assert.equal(await assistants.getByRole("radio").count(), 5);
    assert.equal(await assistants.getByRole("radio", { name: "Hermes" }).isChecked(), true);
    assert.equal(await picked.getByRole("textbox", { name: "Name" }).inputValue(), "Hermes");
    await assistants.getByRole("radio", { name: "OpenClaw" }).check();
    assert.equal(await picked.getByRole("textbox", { name: "Name" }).inputValue(), "OpenClaw");
    await picked.getByRole("textbox", { name: "Name" }).fill("Home server");
    await assistants.getByRole("radio", { name: "Muse" }).check();
    assert.equal(await picked.getByRole("textbox", { name: "Name" }).inputValue(), "Home server");
    await page.screenshot({ path: resolve(out, `${engine}-${colorScheme}-remote-link.png`) });
    await page.keyboard.press("Escape");
    await picked.waitFor({ state: "detached" });
    assert.deepEqual(await calls(), []);

    // Pairing turns remote access on by itself and shows the link once, as a code and as text.
    await page.getByRole("button", { name: "Pair a device", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "Pair a device" });
    await dialog.waitFor();
    assert.deepEqual(await calls(), ["enabled:true", "pair"]);
    assert.equal(await toggle.isChecked(), true);
    const code = dialog.getByRole("img", { name: "Pairing code" });
    const box = await code.boundingBox();
    assert.ok(
      box && Math.abs(box.width - box.height) < 1 && box.width >= 200,
      "the code is square",
    );
    assert.ok((await code.locator("path").getAttribute("d")).length > 2000, "the code has modules");
    // Dark modules on white whatever the appearance, or a phone's camera cannot read it.
    assert.equal(await code.locator("rect").getAttribute("fill"), "white");
    assert.equal(await code.locator("path").getAttribute("fill"), "black");
    const link = dialog.getByRole("textbox", { name: "Pairing link" });
    assert.match(await link.inputValue(), /^https:\/\/o7\.codes\/remote#h{22}\.p{11}\.k{43}$/);
    assert.equal(await link.getAttribute("readonly"), "");
    assert.match(
      await dialog.getByTestId("remote-pairing-time").textContent(),
      /^Opens once, for another (10:00|9:5\d)\./,
    );
    if (engine === "chromium") {
      await dialog.getByRole("button", { name: "Copy" }).click();
      await dialog.getByText("Copied").waitFor();
      assert.equal(
        await page.evaluate(() => navigator.clipboard.readText()),
        await link.inputValue(),
      );
    }
    await settled(page);
    assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
    await page.screenshot({ path: resolve(out, `${engine}-${colorScheme}-remote-pairing.png`) });

    // A device opens the link: the dialog says which, and the list shows it connected.
    await page.evaluate(() => window.remoteFixture.pairDevice("Pixel (Chrome)"));
    await dialog.getByText("Paired with Pixel (Chrome).").waitFor();
    assert.equal(await dialog.getByRole("img").count(), 0, "the spent link stayed on screen");
    // The next device goes on from here: a fresh link in place, without closing the dialog.
    await dialog.getByRole("button", { name: "Pair another device" }).click();
    await dialog.getByRole("img", { name: "Pairing code" }).waitFor();
    assert.equal((await calls()).at(-1), "pair");
    await page.evaluate(() => window.remoteFixture.pairDevice("iPhone (Safari)"));
    await dialog.getByText("Paired with iPhone (Safari).").waitFor();
    await dialog.getByRole("button", { name: "Done" }).click();
    await dialog.waitFor({ state: "detached" });
    assert.equal((await calls()).includes("cancel-pairing"), false, "a used link was cancelled");
    assert.equal(await page.getByText("Connected now", { exact: true }).count(), 2);

    // Closing an unused link cancels it; an expired one offers a new link instead of a dead code.
    await page.getByRole("button", { name: "Pair a device", exact: true }).click();
    await dialog.waitFor();
    await page.keyboard.press("Escape");
    await dialog.waitFor({ state: "detached" });
    assert.equal((await calls()).at(-1), "cancel-pairing");
    await page.evaluate(() => window.remoteFixture.pairingLasts(900));
    await page.getByRole("button", { name: "Pair a device", exact: true }).click();
    await dialog.getByText("This link expired before a device opened it.").waitFor();
    assert.equal(await dialog.getByRole("img").count(), 0);
    await page.evaluate(() => window.remoteFixture.pairingLasts(600_000));
    await dialog.getByRole("button", { name: "New link" }).click();
    await dialog.getByRole("img", { name: "Pairing code" }).waitFor();
    await dialog.getByRole("button", { name: "Close" }).click();

    // Removing a device asks the service by id.
    await page.getByRole("button", { name: "Remove Pixel (Chrome)" }).click();
    await page.getByText("Pixel (Chrome)").waitFor({ state: "detached" });
    assert.equal((await calls()).at(-1), "remove-device:device-1");

    // A bot link: named, created, and its address shown once with where to put it.
    await page.getByRole("button", { name: "Link a bot", exact: true }).click();
    const bot = page.getByRole("dialog", { name: "Link a bot" });
    await bot.waitFor();
    const name = bot.getByRole("textbox", { name: "Name" });
    assert.equal(await name.inputValue(), "Grok Bot");
    await name.fill("fail");
    await bot.getByRole("button", { name: "Create link" }).click();
    await bot.getByRole("alert").getByText("already has 16 bot links").waitFor();
    // The link's level is chosen here: the chat tools unless full control is asked for.
    const levels = bot.getByRole("group", { name: "What it can do" });
    assert.equal(await levels.getByRole("radio", { name: /^Chats/ }).isChecked(), true);
    // Approving and full access are off unless chosen, and full access is full control's alone.
    const approvals = bot.getByRole("group", { name: "What it may approve" });
    const approving = approvals.getByRole("checkbox", { name: /^Answer what agents ask/ });
    const fullAccess = approvals.getByRole("checkbox", { name: /^Full access in a chat/ });
    assert.equal(await approving.isChecked(), false);
    assert.equal(await fullAccess.isDisabled(), true);
    await levels.getByRole("radio", { name: /^Read only/ }).check();
    assert.equal(await approving.isDisabled(), true, "a read-only link approves nothing");
    await levels.getByText("It has no tool for Settings", { exact: false }).waitFor();
    // Neither level is a wall: an agent the bot directs can still run commands.
    await bot.getByText("the choices above limit the bot's own tools", { exact: false }).waitFor();
    await levels.getByRole("radio", { name: /^Full control/ }).check();
    await approving.check();
    assert.equal(await fullAccess.isDisabled(), false);
    await name.fill("Research bot");
    await name.press("Enter");
    const address = bot.getByRole("textbox", { name: "Bot address" });
    assert.match(
      await address.inputValue(),
      /^https:\/\/o7\.codes\/api\/remote\/v1\/mcp\/h{22}\/b{11}\.t{43}$/,
    );
    await bot.getByText("Add a custom connector").waitFor();
    await bot.getByText("works like a password").waitFor();
    // The dialog says whether the assistant has reached this computer yet.
    await bot.getByText("Waiting for Research bot's first call", { exact: false }).waitFor();
    // Then o7's skill, at the site's own address.
    assert.equal(
      await bot.getByRole("textbox", { name: "o7's skill" }).inputValue(),
      "https://o7.codes/skill.md",
    );
    // An assistant on this computer can use o7's own helper instead, with the same address.
    await bot.getByText("If your assistant runs on this computer").click();
    assert.match(
      await bot.getByRole("textbox", { name: "Command" }).inputValue(),
      /^"C:\\Users\\you\\AppData\\Local\\o7\\o7\.exe" --o7-bot-helper$/,
    );
    assert.match(
      await bot.getByRole("textbox", { name: "Environment" }).inputValue(),
      /^O7_LINK=https:\/\/o7\.codes\/api\/remote\/v1\/mcp\//,
    );
    await settled(page);
    assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
    await page.screenshot({ path: resolve(out, `${engine}-${colorScheme}-remote-bot.png`) });
    await bot.getByRole("button", { name: "Done" }).click();
    await page.getByText("Research bot", { exact: true }).waitFor();
    assert.equal((await calls()).at(-1), "create-bot:Research bot:full:approve");
    // The link's level is a chip beside its name; whether it was used is the line under it.
    const linked = page.locator(".settings-row", { hasText: "Research bot" });
    assert.equal(await linked.locator(".g-chip").textContent(), "Full control · answers agents");
    // A new address replaces the old one, shown once with the assistant's steps.
    await page.getByRole("button", { name: "Renew, Research bot" }).click();
    const renewal = page.getByRole("dialog", { name: "A new address for Research bot" });
    await renewal.getByText("The old address stopped working", { exact: false }).waitFor();
    assert.match(
      await renewal.getByRole("textbox", { name: "Bot address" }).inputValue(),
      /\.n{43}$/,
    );
    assert.equal((await calls()).at(-1), "renew-bot:bot-1");
    await renewal.getByRole("button", { name: "Done" }).click();
    await linked.getByText("Not used yet", { exact: true }).waitFor();
    // Where the bot is told when a chat is done: its assistant's address and key, typed once.
    await linked.getByRole("button", { name: "Call back, Research bot" }).click();
    const telling = page.getByRole("dialog", { name: "Tell Research bot when a chat is done" });
    await telling
      .getByText("o7 sends only to the address you give here", { exact: false })
      .waitFor();
    const kinds = telling.getByRole("group", { name: "Its assistant" });
    assert.equal(await kinds.getByRole("radio", { name: "Another" }).isChecked(), true);
    await kinds.getByRole("radio", { name: "Hermes" }).check();
    await telling.getByText("hermes webhook subscribe o7", { exact: false }).waitFor();
    await kinds.getByRole("radio", { name: "Grok Bot" }).check();
    await telling
      .getByText("Its Webhook section shows POST to and key", { exact: false })
      .waitFor();
    const save = telling.getByRole("button", { name: "Save" });
    assert.equal(await save.isDisabled(), true, "nothing to save without an address");
    await telling.getByRole("textbox", { name: "Address" }).fill("ftp://example.com/hook");
    await telling.getByLabel("Key").fill("routine-key");
    await save.click();
    await telling.getByRole("alert").getByText("The address starts with https://.").waitFor();
    await telling
      .getByRole("textbox", { name: "Address" })
      .fill("https://bot.example/routines/r1/webhook");
    await save.click();
    await telling
      .getByText("Grok Bot is told at https://bot.example. Send a test to check it.")
      .waitFor();
    assert.equal((await calls()).at(-1), "set-callback:bot-1:grok:key");
    // Once kept, neither the address's path nor the key is on the page again.
    assert.equal(await page.getByText("routines/r1").count(), 0);
    assert.equal(await telling.getByRole("textbox").count(), 0, "nothing is asked for again");
    await settled(page);
    assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
    await page.screenshot({ path: resolve(out, `${engine}-${colorScheme}-remote-callback.png`) });
    await telling.getByRole("button", { name: "Send a test" }).click();
    await telling.getByText("The last message was delivered just now.", { exact: false }).waitFor();
    assert.equal((await calls()).at(-1), "test-callback:bot-1");
    await telling.getByRole("button", { name: "Done" }).click();
    await linked.getByRole("button", { name: "Call back: on, Research bot" }).waitFor();
    assert.equal(await linked.getByTestId("remote-bot-callback").textContent(), "Call back: on");
    await linked.getByTestId("remote-bot-callback").click();
    await telling.getByRole("button", { name: "Stop telling it" }).click();
    assert.equal((await calls()).at(-1), "clear-callback:bot-1");
    assert.equal(await linked.getByTestId("remote-bot-callback").textContent(), "Call back");
    assert.equal(await page.getByText(/t{43}/).count(), 0, "the address stayed on the page");
    await page.getByRole("button", { name: "Remove Research bot" }).click();
    assert.equal((await calls()).at(-1), "remove-bot:bot-1");

    // Each assistant is given the address the way it takes one: Hermes as lines for its
    // config, OpenClaw as a command, both with room to wait for a turn. A link named for an
    // assistant wears its mark in the list.
    for (const [assistant, label, wanted] of [
      [
        "Hermes",
        "Lines for Hermes' config",
        /^mcp_servers:\n {2}o7:\n {4}url: "https:\/\/o7\.codes\/api\/remote\/v1\/mcp\/h{22}\/b{11}\.t{43}"\n {4}timeout: 300$/,
      ],
      [
        "OpenClaw",
        "Command for OpenClaw",
        /^openclaw mcp add o7 --url https:\/\/o7\.codes\/api\/remote\/v1\/mcp\/h{22}\/b{11}\.t{43} --transport streamable-http --timeout 300$/,
      ],
    ]) {
      await page.getByRole("button", { name: "Link a bot", exact: true }).click();
      await bot.getByRole("radio", { name: assistant }).check();
      await bot.getByRole("button", { name: "Create link" }).click();
      assert.match(await bot.getByRole("textbox", { name: label }).inputValue(), wanted);
      assert.equal(await bot.getByText("Add a custom connector").count(), 0);
      if (assistant === "Hermes")
        await page.screenshot({ path: resolve(out, `${engine}-${colorScheme}-remote-hermes.png`) });
      await bot.getByRole("button", { name: "Done" }).click();
      const row = page.locator(".settings-row", { hasText: assistant });
      assert.equal(await row.locator(".g-agent-mark").count(), 1, assistant);
      await page.getByRole("button", { name: `Remove ${assistant}` }).click();
    }

    // When the relay cannot be reached the row says so and why; turning it off says off.
    await page.evaluate(() => window.remoteFixture.offline("The relay could not be reached."));
    await page
      .getByText("Not reachable right now; trying again. The relay could not be reached.")
      .waitFor();
    await toggle.focus();
    await page.keyboard.press("Space");
    await page.getByText("Off. Nothing on this computer can be reached from outside.").waitFor();
    assert.equal((await calls()).at(-1), "enabled:false");

    // Already on, with a device and a bot: both lists, and the search finds the page's rows.
    await page.goto(`${url}/?preview=settings&page=remote&remote=on`);
    await page.getByText("iPhone (Safari)", { exact: true }).waitFor();
    await page.getByText("Last seen 7 min ago", { exact: true }).waitFor();
    await page.getByText("Grok Bot", { exact: true }).waitFor();
    await settled(page);
    assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
    await page.screenshot({ path: resolve(out, `${engine}-${colorScheme}-remote.png`) });
    await page.getByRole("searchbox", { name: "Search settings" }).fill("grok");
    await page
      .getByRole("navigation", { name: "Matching settings" })
      .getByRole("button", { name: /Link a bot/ })
      .click();
    await page.getByRole("heading", { level: 1, name: "Remote access" }).waitFor();

    // Staying reachable: both off until chosen. Opening at sign-in is the system's own record,
    // keeping awake the service's; each switch says what it now is.
    const loginStart = page.getByRole("switch", { name: "Open at sign-in" });
    const awake = page.getByRole("switch", { name: "Keep this computer awake" });
    assert.equal(await loginStart.isChecked(), false);
    assert.equal(await awake.isChecked(), false);
    const turnedOn = (id) =>
      page.waitForFunction(
        (testId) => document.querySelector(`[data-testid="${testId}"]`)?.checked,
        id,
      );
    await loginStart.click();
    await turnedOn("remote-login-start");
    await awake.click();
    await turnedOn("remote-keep-awake");
    assert.deepEqual(await calls(), ["login-start:true", "keep-awake:true"]);
    await page.getByText("The screen still turns off.", { exact: false }).waitFor();
    await awake.click();
    await page.waitForFunction(() => window.remoteFixture.calls().at(-1) === "keep-awake:false");
    assert.equal(await awake.isChecked(), false);

    // Automations: the one there says where its tasks start; a new one needs an instruction, is
    // made in the project chosen, and shows its address once, at /hook/.
    const automations = page.locator(".settings-row", { hasText: "CI failures" });
    await automations.getByText("In web. Last used", { exact: false }).waitFor();
    await page.getByRole("button", { name: "New automation", exact: true }).click();
    const making = page.getByRole("dialog", { name: "New automation" });
    await making.getByRole("textbox", { name: "Name" }).fill("New issues");
    await making.getByRole("combobox", { name: "Project" }).selectOption("project-api");
    const create = making.getByRole("button", { name: "Create automation" });
    assert.equal(await create.isDisabled(), true, "nothing to do yet");
    await making
      .getByRole("textbox", { name: "What its agent does with what is sent" })
      .fill("Reproduce the issue and fix it on a branch.");
    await settled(page);
    assert.deepEqual((await new AxeBuilder({ page }).analyze()).violations, []);
    await create.click();
    const hookAddress = making.getByRole("textbox", { name: "Automation address" });
    assert.match(await hookAddress.inputValue(), /^https:\/\/o7\.codes\/api\/remote\/v1\/hook\//);
    assert.equal((await calls()).at(-1), "create-automation:New issues:project-api");
    await page.screenshot({ path: resolve(out, `${engine}-${colorScheme}-remote-automation.png`) });
    await making.getByRole("button", { name: "Done" }).click();
    const made = page.locator(".settings-row", { hasText: "New issues" });
    await made.getByText("In api. Not used yet").waitFor();
    await page.getByRole("button", { name: "Renew, New issues" }).click();
    const renewed = page.getByRole("dialog", { name: "A new address for New issues" });
    assert.match(
      await renewed.getByRole("textbox", { name: "Automation address" }).inputValue(),
      /\/hook\//,
    );
    await renewed.getByRole("button", { name: "Done" }).click();
    await page.getByRole("button", { name: "Remove New issues" }).click();
    await made.waitFor({ state: "detached" });
    await page.screenshot({
      path: resolve(out, `${engine}-${colorScheme}-remote-automations.png`),
    });

    // From off, creating a bot link is what turns remote access on.
    await page.goto(`${url}/?preview=settings&page=remote`);
    await page.getByRole("button", { name: "Link a bot", exact: true }).click();
    await bot.getByRole("button", { name: "Create link" }).click();
    await bot.getByRole("textbox", { name: "Bot address" }).waitFor();
    assert.deepEqual(await calls(), ["enabled:true", "create-bot:Grok Bot"]);
    assert.equal(await toggle.isChecked(), true);
    assert.deepEqual(errors, []);
    await context.close();
  }
}
