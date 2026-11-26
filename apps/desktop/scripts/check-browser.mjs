/**
 * The browser beside the chat (`?preview=browser`, docs/superpowers/plans/2026-11-09-browser.md):
 * its page placed over the panel and hidden under anything that opens over it, tabs and the start
 * page, Annotate's note (its picture, marks, Send and Add to chat), the notices for downloads,
 * other programs' links and blocked pop-ups, history, and Browser use under Universal plugins
 * (`?preview=customize`), in both appearances, with axe. The native page is a frame here; the
 * native checks are recorded in the plan.
 */
import assert from "node:assert/strict";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";

async function axe(page, label, scope = "aside[aria-label=Browser]") {
  const result = await new AxeBuilder({ page }).include(scope).analyze();
  assert.deepEqual(
    result.violations.map(
      (violation) => `${violation.id}: ${violation.nodes.map((node) => node.target).join(" ")}`,
    ),
    [],
    `${label}: axe`,
  );
}

const frameShown = (page) =>
  page.evaluate(() => {
    const frames = [...document.querySelectorAll("iframe[data-fixture-page]")];
    return frames.some((frame) => frame.style.display === "block");
  });

async function until(fn, label, ms = 5000) {
  const deadline = Date.now() + ms;
  for (;;) {
    if (await fn()) return;
    if (Date.now() > deadline) throw new Error(`${label}: timed out`);
    await new Promise((done) => setTimeout(done, 50));
  }
}

export async function checkBrowser(browser, url, out, engine) {
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
      await page.goto(`${url}/?preview=browser&theme=${theme}`);
      const panel = page.getByTestId("browser-panel");
      await panel.waitFor();
      await until(() => frameShown(page), `${label}: the page shows over the panel`);
      const address = page.getByRole("combobox", { name: "Address and search" });
      assert.match(await address.inputValue(), /localhost:3104/, `${label}: the address shows`);
      await axe(page, `${label} page`);
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-page.png`) });

      // Anything over the page hides it; its picture stands in until it closes.
      await page.locator(".browser-menu > summary").click();
      await until(async () => !(await frameShown(page)), `${label}: the menu hides the page`);
      assert.equal(
        await panel.locator(".browser-picture img").count(),
        1,
        `${label}: the picture stands in`,
      );
      await page.keyboard.press("Escape");
      await until(() => frameShown(page), `${label}: the page shows again`);

      // A new tab opens on the start page; an address loads in it; closing it comes back.
      await page.getByRole("button", { name: "New tab", exact: true }).click();
      await page.getByTestId("browser-start").waitFor();
      await axe(page, `${label} start page`);
      await address.fill("example.com");
      await address.press("Enter");
      await until(
        async () => (await panel.locator(".browser-tab").count()) === 2 && (await frameShown(page)),
        `${label}: the typed address loads`,
      );
      await panel.locator(".browser-tab[data-active] .browser-tab-close").click();
      await until(
        async () => (await panel.locator(".browser-tab").count()) === 1,
        `${label}: the tab closes`,
      );

      // Annotate: a pick opens the note over the page's picture, with one mark on the element.
      await page.getByTestId("browser-annotate").click();
      await page.getByTestId("browser-annotate-hint").waitFor();
      // Where the element is on screen as the page shows, before the note freezes it.
      // (The hint moved the page down; the frame follows a frame later.)
      await page.waitForTimeout(250);
      const element = await page.evaluate(() => {
        const frame = document.querySelector("iframe[data-fixture-page]");
        const box = frame?.contentDocument?.querySelector(".hero .cta")?.getBoundingClientRect();
        const at = frame?.getBoundingClientRect();
        return box && at ? { x: at.x + box.x, y: at.y + box.y, width: box.width } : null;
      });
      await page.evaluate(() => window.browserFixture.pick(".hero .cta"));
      const note = page.getByTestId("browser-note");
      await note.waitFor();
      assert.equal(await panel.locator(".browser-mark").count(), 1, `${label}: one mark`);
      assert.equal(
        await panel.locator(".browser-picture-frame img").count(),
        1,
        `${label}: the picture is the page's`,
      );
      assert.ok(await page.getByTestId("browser-note-hint").isVisible(), `${label}: its hint`);
      const mark = await panel.locator(".browser-mark").boundingBox();
      assert.ok(
        mark && element && Math.abs(mark.x - element.x) < 3 && Math.abs(mark.y - element.y) < 3,
        `${label}: the mark sits on the element (${JSON.stringify({ mark, element })})`,
      );
      await axe(page, `${label} note`);
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-note.png`) });
      const field = note.getByRole("textbox");
      await field.fill("Make the button match the brand");
      await note.getByRole("button", { name: /^Send to / }).click();
      await note.waitFor({ state: "detached" });
      const sent = await page.evaluate(() => window.browserFixture.sent.at(-1));
      assert.match(sent.text, /Make the button match the brand/, `${label}: the note is sent`);
      assert.match(sent.text, /Start free/, `${label}: it names the element`);
      await until(() => frameShown(page), `${label}: the page shows after sending`);

      // Add to chat puts the note in the message box instead.
      await page.evaluate(() => window.browserFixture.pick(".card h2"));
      await note.waitFor();
      await field.fill("Larger headings");
      await note.getByRole("button", { name: "Add to chat" }).click();
      await note.waitFor({ state: "detached" });
      const composer = page.locator(".chat-composer textarea").first();
      assert.match(await composer.inputValue(), /Larger headings/, `${label}: added to the box`);
      await page.getByTestId("browser-annotate").click();
      await page.getByTestId("browser-annotate-hint").waitFor({ state: "detached" });

      // Notices: a download, another program's link, a blocked pop-up that opens as a tab.
      const emit = (event) => page.evaluate((value) => window.browserFixture.emit(value), event);
      const tab = await page.evaluate(
        () => document.querySelector("iframe[data-fixture-page]")?.dataset.fixturePage,
      );
      await emit({
        type: "download",
        chat_id: "browser-chat",
        tab_id: tab,
        url: "http://localhost:3104/report.pdf",
        file: "C:/Users/me/Downloads/report.pdf",
        state: "done",
      });
      await page.getByText("Downloaded report.pdf to your Downloads folder.").waitFor();
      await emit({
        type: "outside",
        chat_id: "browser-chat",
        tab_id: tab,
        url: "mailto:team@acme.test",
      });
      await page.getByText("This page wants to open mailto", { exact: false }).waitFor();
      await emit({
        type: "blocked",
        chat_id: "browser-chat",
        tab_id: tab,
        url: "https://ads.example.com/",
      });
      const blocked = page.locator(".browser-notice");
      await blocked.getByText("This page tried to open a window", { exact: false }).waitFor();
      await axe(page, `${label} notice`);
      await blocked.getByRole("button", { name: "Open as a tab" }).click();
      await until(
        async () => (await panel.locator(".browser-tab").count()) === 2,
        `${label}: the blocked window opens as a tab`,
      );
      await panel.locator(".browser-tab[data-active] .browser-tab-close").click();

      // An agent at work shows in the panel.
      await page.evaluate(() => window.browserFixture.agent("Clicking e3", true));
      await page.getByTestId("browser-agent").waitFor();
      await page.evaluate(() => window.browserFixture.agent(null));
      await page.getByTestId("browser-agent").waitFor({ state: "detached" });

      // History, from the menu: searchable, each page removable.
      await page.locator(".browser-menu > summary").click();
      await page.locator(".browser-menu").getByRole("button", { name: "History" }).click();
      const history = page.getByTestId("browser-history");
      await history.waitFor();
      await axe(page, `${label} history`);
      const rows = () => history.locator(".browser-history-row").count();
      const before = await rows();
      assert.ok(before > 0, `${label}: history lists pages`);
      await history.locator(".browser-history-remove").first().click();
      await until(async () => (await rows()) === before - 1, `${label}: a page comes off`);
      await page.getByRole("button", { name: "Close history" }).click();
      assert.deepEqual(errors, [], `${label}: no page errors`);

      // Browser use on its own Settings page, with what agents get said outright.
      await page.goto(`${url}/?preview=customize&theme=${theme}`);
      await page
        .getByRole("navigation", { name: "Settings sections" })
        .getByRole("button", { name: "Browser", exact: true })
        .click();
      const card = page.getByTestId("plugins-universal");
      await card.waitFor();
      await card.getByText("Exposed to every AI agent", { exact: true }).waitFor();
      assert.equal(
        await card
          .getByRole("list", { name: "Browser tools agents get" })
          .getByRole("listitem")
          .count(),
        14,
        `${label}: every browser tool is listed`,
      );
      await axe(page, `${label} universal plugins`, "[data-testid=plugins-universal]");
      await page.screenshot({ path: resolve(out, `${engine}-${theme}-universal.png`) });
      const toggle = page.getByTestId("plugins-browser");
      assert.ok(await toggle.isChecked(), `${label}: Browser use is on`);
      await toggle.click();
      await card.getByText("Off for every agent:", { exact: false }).waitFor();
      assert.equal(await card.getByText("Browsing history").count(), 0, `${label}: settings go`);
      await toggle.click();
      await card.getByText("Browsing history").waitFor();
      await card.getByRole("button", { name: "Open browsing history" }).click();
      await page.getByTestId("browser-history").waitFor();
      await page.keyboard.press("Escape");
      await card.getByRole("button", { name: "Clear browsing data" }).click();
      const dialog = page.getByRole("dialog", { name: "Clear browsing data" });
      await dialog.getByRole("button", { name: "Clear data" }).click();
      await card.getByText("Cleared.", { exact: false }).waitFor();
      assert.deepEqual(errors, [], `${label}: no page errors in Settings`);
    } finally {
      await context.close();
    }
  }
  await checkBrowserMedia(browser, url, out, engine);
}

/**
 * Linux without GStreamer's plugins (`?media=off`): the panel says once, calmly, why pages play no
 * sound or video, and not again on this device. A page that stopped working twice says it reloads
 * without them.
 */
export async function checkBrowserMedia(browser, url, out, engine) {
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    reducedMotion: "reduce",
  });
  try {
    const page = await context.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`${url}/?preview=browser&media=off`);
    const notice = page.getByTestId("browser-media-notice");
    await notice.waitFor();
    assert.match(
      await notice.textContent(),
      /^Audio and video are off here: GStreamer's base and good plugins are missing\. Install gst-plugins-base and gst-plugins-good, then restart Ghost\.$/,
      `${engine}: the notice names the packages`,
    );
    assert.equal(await notice.getAttribute("role"), "status", `${engine}: calm, not an alert`);
    await axe(page, `${engine} media notice`);
    await page.screenshot({ path: resolve(out, `${engine}-media-notice.png`) });
    await notice.getByRole("button", { name: "Dismiss" }).click();
    await notice.waitFor({ state: "detached" });
    await page.reload();
    await page.getByTestId("browser-panel").waitFor();
    await until(() => frameShown(page), `${engine}: the page shows again`);
    assert.equal(await notice.count(), 0, `${engine}: said once on this device`);

    // A page whose process ended: Reload, and when it ended twice, that media goes off.
    const crash = async (media) => {
      const chat = await page.evaluate(() => {
        const frame = document.querySelector("iframe[data-fixture-page]");
        return frame?.dataset.fixturePage ?? null;
      });
      await page.evaluate(
        ({ tabId, media }) =>
          window.browserFixture.emit({
            type: "chat",
            browser: {
              chat_id: "browser-chat",
              active: tabId,
              tabs: [
                {
                  id: tabId,
                  url: "http://localhost:3104/",
                  title: "Acme dev",
                  loading: false,
                  can_go_back: false,
                  can_go_forward: false,
                  crashed: true,
                  error: null,
                  zoom: 1,
                  asleep: true,
                  annotating: false,
                  media,
                },
              ],
            },
          }),
        { tabId: chat, media },
      );
      const card = page.getByRole("alert").filter({ hasText: "This page stopped working." });
      await card.waitFor();
      return card;
    };
    const twice = await crash(false);
    // This system has no media at all: the panel's own line said why, the card adds nothing.
    assert.equal(await twice.getByText("It stopped twice", { exact: false }).count(), 0);
    assert.deepEqual(errors, [], `${engine}: no page errors with media off`);
  } finally {
    await context.close();
  }

  const playing = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  try {
    const page = await playing.newPage();
    await page.goto(`${url}/?preview=browser`);
    await page.getByTestId("browser-panel").waitFor();
    await until(() => frameShown(page), `${engine}: the page shows`);
    assert.equal(await page.getByTestId("browser-media-notice").count(), 0, `${engine}: no notice`);
    const tabId = await page.evaluate(
      () => document.querySelector("iframe[data-fixture-page]")?.dataset.fixturePage,
    );
    for (const media of [true, false]) {
      await page.evaluate(
        ({ tabId, media }) =>
          window.browserFixture.emit({
            type: "chat",
            browser: {
              chat_id: "browser-chat",
              active: tabId,
              tabs: [
                {
                  id: tabId,
                  url: "http://localhost:3104/",
                  title: "Acme dev",
                  loading: false,
                  can_go_back: false,
                  can_go_forward: false,
                  crashed: true,
                  error: null,
                  zoom: 1,
                  asleep: true,
                  annotating: false,
                  media,
                },
              ],
            },
          }),
        { tabId, media },
      );
      const card = page.getByRole("alert").filter({ hasText: "This page stopped working." });
      await card.waitFor();
      const second = card.getByText(
        "It stopped twice in a short time, so it reloads with audio and video off.",
      );
      if (media) assert.equal(await second.count(), 0, `${engine}: one end says nothing more`);
      else await second.waitFor();
      await card.getByRole("button", { name: "Reload" }).waitFor();
    }
    await axe(page, `${engine} crashed page`);
    await page.screenshot({ path: resolve(out, `${engine}-crashed-twice.png`) });
  } finally {
    await playing.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const { chromium, webkit } = await import("playwright");
  const out = fileURLToPath(new URL("../../../target/browser-panel/", import.meta.url));
  await mkdir(out, { recursive: true });
  for (const [engine, browserType] of Object.entries({ chromium, webkit })) {
    const browser = await browserType.launch();
    try {
      await checkBrowser(browser, process.env.CHECK_URL ?? "http://localhost:1476", out, engine);
      console.log(
        `${engine}: the browser panel, its notices, Annotate's note, history and Universal plugins passed in both appearances`,
      );
    } finally {
      await browser.close();
    }
  }
}
