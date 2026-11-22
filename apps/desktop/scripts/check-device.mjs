/**
 * The device beside the chat (`?preview=device`, docs/superpowers/plans/2026-11-21-mobile.md):
 * setting Android up (the offer, Google's licence, the download, a failure), choosing a device,
 * a device starting, live and stopped, a phone and the question about its agent, the pane's own
 * input (a click, a drag, keys, text), the header's button and its shortcut, a narrow window, and
 * Mobile under Settings (`?preview=customize`), in both appearances, with axe. There is no device
 * here, so no video; the checks against a real emulator are recorded in the plan.
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

async function axe(page, label, scope = "aside[aria-label=Device]") {
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

const calls = (page) => page.evaluate(() => [...window.deviceFixture.calls]);
const called = (page, call, label) =>
  until(async () => (await calls(page)).includes(call), `${label}: ${call}`);

export async function checkDevice(browser, url, out, engine) {
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
      const open = async (state) => {
        await page.goto(`${url}/?preview=device&theme=${theme}&state=${state}`);
        await page.getByTestId("device-panel").waitFor();
      };
      const shot = (name) =>
        page.screenshot({ path: resolve(out, `${engine}-${theme}-${name}.png`) });
      const panel = page.getByTestId("device-panel");

      // Nothing installed: the pane says what it will fetch before it fetches anything.
      await open("setup");
      await panel.getByRole("heading", { name: "Set up Android" }).waitFor();
      assert.equal(
        await panel.getByTestId("device-start").count(),
        0,
        `${label}: no phone to start`,
      );
      await axe(page, `${label} setup`);
      await shot("setup");
      await panel.getByTestId("device-setup").click();
      await panel.getByTestId("device-offer").waitFor();
      await called(page, "fetchOffer", label);
      assert.equal(
        await panel.locator(".device-offer .device-row").count(),
        3,
        `${label}: three packages`,
      );
      assert.match(await panel.getByTestId("device-license").textContent(), /License Agreement/);
      assert.match(
        await panel.locator(".device-offer").textContent(),
        /2\.2 GB in all, from dl\.google\.com/,
      );
      // The licence scrolls, and the keyboard reaches it.
      await panel.getByTestId("device-license").focus();
      assert.equal(
        await page.evaluate(() => document.activeElement?.dataset.testid),
        "device-license",
        `${label}: the licence takes focus`,
      );
      await axe(page, `${label} offer`);
      await shot("offer");
      assert.ok(
        !(await calls(page)).includes("fetchStart"),
        `${label}: nothing fetched before accepting`,
      );
      await panel.getByTestId("device-accept").click();
      await panel.getByTestId("device-fetching").waitFor();
      await called(page, "fetchStart", label);
      assert.match(
        await panel.getByTestId("device-fetching").textContent(),
        /Downloading Android Emulator\. 212 MB of 2\.2 GB\./,
      );
      await axe(page, `${label} downloading`);
      await shot("downloading");
      await panel.getByRole("button", { name: "Stop", exact: true }).click();
      await called(page, "fetchCancel", label);
      await panel.getByTestId("device-setup").waitFor();

      await open("failed");
      await panel.getByRole("heading", { name: "Android was not downloaded" }).waitFor();
      assert.match(await panel.getByRole("alert").textContent(), /did not match the checksum/);
      await axe(page, `${label} failed`);
      await panel.getByRole("button", { name: "Try again" }).click();
      await panel.getByTestId("device-offer").waitFor();

      // The emulator is there but cannot run fast: the pane says the one thing to do.
      await open("slow");
      assert.match(
        await panel.getByRole("alert").textContent(),
        /Windows Hypervisor Platform, then restart/,
      );
      assert.equal(
        await panel.getByTestId("device-start").count(),
        0,
        `${label}: no start while slow`,
      );
      assert.ok(
        await panel.getByRole("button", { name: "Use" }).isDisabled(),
        `${label}: nor by its row`,
      );
      await axe(page, `${label} slow`);

      // Choosing: a free emulator, one another chat holds, a phone, and a phone not yet allowed.
      await open("chooser");
      const rows = panel.getByTestId("device-row");
      assert.equal(await rows.count(), 4, `${label}: four devices`);
      assert.match(await rows.nth(1).textContent(), /Phone 2\s*In use by another chat/);
      assert.ok(await rows.nth(1).getByRole("button", { name: "Use" }).isDisabled());
      assert.match(await rows.nth(3).textContent(), /Galaxy S25\s*Unlock the phone and accept/);
      assert.ok(await rows.nth(3).getByRole("button", { name: "Use" }).isDisabled());
      await axe(page, `${label} chooser`);
      await shot("chooser");
      await panel.getByTestId("device-start").click();
      await called(page, "attach:any", label);
      await panel.getByTestId("device-starting").waitFor();
      assert.match(await panel.getByTestId("device-title").textContent(), /Phone 1/);
      await axe(page, `${label} starting`);
      await shot("starting");
      // Change lets go of it and comes back to the list.
      await panel.getByTestId("device-change").click();
      await called(page, "detach", label);
      await panel.getByTestId("device-chooser").waitFor();

      await open("stopped");
      await panel.getByRole("heading", { name: "Phone 1 is not running" }).waitFor();
      assert.match(await panel.getByTestId("device-stopped").textContent(), /disk is too full/);
      await panel.getByRole("button", { name: "Start it again" }).click();
      await called(page, "attach:o7_phone_1", label);

      // Live: the screen takes the pointer and the keyboard, and the device's buttons work.
      await open("live");
      const canvas = panel.getByTestId("device-canvas");
      await called(page, "watch", label);
      // An engine that cannot decode video says so, and the device's buttons still work.
      if (await page.evaluate(() => typeof VideoDecoder === "undefined")) {
        await panel.getByRole("heading", { name: "The screen cannot be shown here" }).waitFor();
        assert.ok(await canvas.isHidden(), `${label}: no empty canvas`);
      } else {
        await canvas.waitFor();
        // The video's size, as the stream's first packet says it: 720 by 1600.
        await page.evaluate(() =>
          window.deviceFixture.watching.get("device-fixture-chat")(
            new Uint8Array([1, 2, 208, 6, 64]),
          ),
        );
        const box = await canvas.boundingBox();
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
        await until(
          async () => (await calls(page)).filter((call) => call.startsWith("touch:")).length >= 2,
          `${label}: a click is a touch down and up`,
        );
        const touches = (await calls(page)).filter((call) => call.startsWith("touch:"));
        assert.match(
          touches[0],
          /^touch:down:3[56]\d,(79\d|80\d)$/,
          `${label}: at the middle of the video: ${touches[0]}`,
        );
        assert.match(touches.at(-1), /^touch:up:3[56]\d,(79\d|80\d)$/);
        await page.mouse.move(box.x + box.width / 2, box.y + box.height * 0.8);
        await page.mouse.down();
        await page.mouse.move(box.x + box.width / 2, box.y + box.height * 0.5, { steps: 4 });
        // Past the picture's edge the finger stays on its edge.
        await page.mouse.move(box.x + box.width / 2, box.y - 200, { steps: 4 });
        await page.mouse.up();
        await until(
          async () => (await calls(page)).at(-1)?.startsWith("touch:up:"),
          `${label}: the drag ends`,
        );
        const drag = (await calls(page)).filter((call) => call.startsWith("touch:"));
        assert.ok(
          drag.some((call) => call.startsWith("touch:move:")),
          `${label}: a drag moves`,
        );
        assert.match(
          drag.at(-1),
          /^touch:up:3[56]\d,0$/,
          `${label}: clamped to the top edge: ${drag.at(-1)}`,
        );
        // The keyboard goes to the device while its screen has focus.
        await canvas.focus();
        await page.keyboard.type("hi");
        await page.keyboard.press("Enter");
        await page.keyboard.press("Backspace");
        await page.keyboard.press("Escape");
        for (const call of ["text:h", "text:i", "key:enter", "key:delete", "key:back"])
          await called(page, call, label);
      }
      for (const [name, call] of [
        ["Back", "key:back"],
        ["Home", "key:home"],
        ["Recent apps", "key:recents"],
        ["Turn the device", "rotate"],
      ]) {
        await panel.getByRole("button", { name, exact: true }).click();
        await called(page, call, `${label} ${name}`);
      }
      await panel.getByTestId("device-screenshot").click();
      await panel.getByTestId("device-saved").waitFor();
      assert.match(
        await panel.getByTestId("device-saved").textContent(),
        /Saved to .*o7-device-\d+\.png/,
      );
      // What the agent is doing shows while it does it, with no word of what it types.
      await page.evaluate(() => {
        const fake = window.deviceFixture;
        fake.change({ chats: fake.current().chats.map((chat) => ({ ...chat, agent: "Typing" })) });
      });
      assert.equal(await panel.getByTestId("device-agent").textContent(), "Claude Code · Typing");
      await axe(page, `${label} live`);
      await shot("live");

      // A phone: the agent may not use it until the person says so, here.
      await open("phone");
      const consent = panel.getByTestId("device-consent");
      await consent.waitFor();
      const allow = consent.getByRole("switch");
      assert.equal(
        await allow.isChecked(),
        false,
        `${label}: a phone starts as the person's alone`,
      );
      await allow.check();
      await called(page, "allowAgent:true", label);
      assert.equal(await allow.isChecked(), true);
      await allow.uncheck();
      await called(page, "allowAgent:false", label);
      await axe(page, `${label} phone`);
      await shot("phone");

      // The header's button and Ctrl+Shift+D close and open the pane; the browser takes its side.
      await open("chooser");
      const toggle = page.getByTestId("chat-device");
      assert.equal(await toggle.getAttribute("aria-expanded"), "true");
      await toggle.click();
      await panel.waitFor({ state: "detached" });
      assert.equal(await toggle.getAttribute("aria-expanded"), "false");
      await page.keyboard.press("Control+Shift+D");
      await panel.waitFor();
      await panel.getByRole("button", { name: "Close the device" }).click();
      await panel.waitFor({ state: "detached" });

      // A narrow window: the pane covers the chat's work area instead of sitting beside it.
      await page.setViewportSize({ width: 900, height: 800 });
      await toggle.click();
      await panel.waitFor();
      assert.equal(await panel.getAttribute("data-full"), "", `${label}: full on a narrow window`);
      await axe(page, `${label} narrow`);
      await shot("narrow");
      await page.setViewportSize({ width: 1440, height: 900 });

      // Mobile turned off: no button and no pane.
      await page.goto(`${url}/?preview=device&theme=${theme}&state=off`);
      await page.getByTestId("chat-view").waitFor();
      assert.equal(
        await page.getByTestId("chat-device").count(),
        0,
        `${label}: no button when off`,
      );
      assert.equal(await page.getByTestId("device-panel").count(), 0, `${label}: no pane when off`);

      // Settings > Mobile: the switch, what agents get, and what is installed.
      await page.goto(`${url}/?preview=customize&theme=${theme}`);
      await page.getByRole("button", { name: "Mobile", exact: true }).click();
      const settings = page.getByTestId("mobile-settings");
      await settings.waitFor();
      assert.equal(await settings.getByRole("switch", { name: "Mobile" }).isChecked(), true);
      assert.equal(
        await settings.locator(".plugins-universal-tools li").count(),
        16,
        `${label}: sixteen tools`,
      );
      assert.match(await settings.getByTestId("mobile-android").textContent(), /^Set up\./);
      // It comes with o7: nothing to add or remove, only the switch.
      assert.equal(
        await settings.getByRole("button", { name: /^(Add|Remove|Install|Uninstall)/ }).count(),
        0,
        `${label}: no add or remove`,
      );
      await axe(page, `${label} settings`, "[data-testid=mobile-settings]");
      await settings.getByRole("switch", { name: "Mobile" }).uncheck();
      await until(
        async () => /Off for every agent/.test(await settings.textContent()),
        `${label}: off says what stops`,
      );
      assert.equal(await settings.locator(".plugins-universal-tools").count(), 0);
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-settings-off.png`) });

      assert.deepEqual(errors, [], `${label}: page errors`);
    } finally {
      await context.close();
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/device-panel/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkDevice(browser, process.env.CHECK_URL ?? "http://localhost:1476", out, engine);
      console.log(
        `${engine}: the Device pane's setup, chooser, live input, phone consent and Settings > Mobile passed in both appearances`,
      );
    } finally {
      await browser.close();
    }
  }
}
