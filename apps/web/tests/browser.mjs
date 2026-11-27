// The built site in owned headless browsers. Requests to anything but the local server are refused.
import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";

/** A download button goes through the site's counter (app/counted.ts). */
const counted = (url) =>
  url.replace(
    /^https:\/\/github\.com\/itscool2b\/(?:ghost|o7|auracode)\/releases\/download\//,
    "/get/",
  );

// Graphite settles a control's colours over 300ms when its state changes; audit the settled page.
const settled = (page) =>
  page.waitForFunction(() =>
    document.getAnimations().every((animation) => !(animation instanceof CSSTransition)),
  );

/** The page is always dark: X Money's black, whatever the system asks for. */
const PAGE = "rgb(0, 0, 0)";
const pageColour = (page) => page.evaluate(() => getComputedStyle(document.body).backgroundColor);

const audit = async (page, where, include) => {
  let builder = new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]);
  if (include) builder = builder.include(include);
  const axe = await builder.analyze();
  assert.deepEqual(
    axe.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
    [],
    where,
  );
};

/**
 * The agents in New chat's menu (Agents.tsx): every agent named once, each row wholly inside the
 * viewport and none overlapping the next.
 */
const agentWall = (page) =>
  page.evaluate(() => {
    const shown = [...document.querySelectorAll(".pick-list > .pick-row:not(.pick-more)")].filter(
      (item) => item.getClientRects().length > 0,
    );
    const boxes = shown.map((item) => item.getBoundingClientRect());
    return {
      names: shown.map((item) => item.querySelector(".pick-name")?.textContent),
      inView: boxes.every(
        (box) => box.left >= 0 && box.right <= document.documentElement.clientWidth,
      ),
      apart: boxes.every((box, i) =>
        boxes.every(
          (other, j) =>
            i === j ||
            box.right <= other.left + 0.5 ||
            other.right <= box.left + 0.5 ||
            box.bottom <= other.top + 0.5 ||
            other.bottom <= box.top + 0.5,
        ),
      ),
    };
  });

const assertWall = (wall, where) => {
  assert.equal(wall.names.length, 9, `${where}: the menu names every agent`);
  assert.equal(new Set(wall.names).size, 9, `${where}: each agent once`);
  assert.ok(wall.inView, `${where}: the menu fits the page`);
  assert.ok(wall.apart, `${where}: no two rows overlap`);
};

const sideways = (page) =>
  page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);

const root = resolve(fileURLToPath(new URL("../build/client/", import.meta.url)));
const headerText = await readFile(join(root, "_headers"), "utf8");
const csp = headerText.match(/Content-Security-Policy: ([^\n]+)/)[1];
const mime = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".png": "image/png",
  ".webp": "image/webp",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".json": "application/json",
};
const server = createServer(async (request, response) => {
  try {
    let path = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
    if (!extname(path)) path += "/index.html";
    const target = resolve(root, `.${path}`);
    if (!target.startsWith(root + sep)) throw new Error("path");
    const body = await readFile(target);
    response.writeHead(200, {
      "Content-Type": mime[extname(target)] ?? "application/octet-stream",
      "Content-Security-Policy": csp,
    });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end("Not found");
  }
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const base = `http://127.0.0.1:${server.address().port}`;
const artifacts = fileURLToPath(new URL("../../../target/web-checks/", import.meta.url));
await mkdir(artifacts, { recursive: true });
try {
  for (const [name, engine] of [
    ["chromium", chromium],
    ["webkit", webkit],
  ]) {
    if (process.argv[2] && process.argv[2] !== name) continue;
    const browser = await engine.launch({ headless: true });
    try {
      const context = await browser.newContext({
        viewport: { width: 1440, height: 1000 },
        reducedMotion: "reduce",
        colorScheme: "dark",
      });
      const page = await context.newPage();
      const failures = [];
      page.on("pageerror", (error) => failures.push(error.message));
      page.on("console", (message) => {
        if (message.type() === "error" && message.text().includes("Content Security Policy"))
          failures.push(message.text());
      });
      await page.route("**/*", (route) => {
        const url = route.request().url();
        if (!url.startsWith(base)) return route.abort();
        return route.continue();
      });
      await page.addInitScript(() => localStorage.setItem("auracode-appearance", "light"));
      await page.goto(base);
      const heroStage = page.locator(".showcase-window .handoff-stage");
      assert.equal(await heroStage.locator(".handoff-prior").isVisible(), true);
      assert.equal(await heroStage.locator(".handoff-next").isVisible(), true);
      // The site is always dark: pinned on the root, with no switch and nothing read from
      // storage (the stored value above asks for light and changes nothing).
      assert.equal(await page.locator("html").getAttribute("data-theme"), "dark");
      assert.equal(await page.getByLabel("Appearance").count(), 0);
      assert.equal(await pageColour(page), PAGE);
      // The agents in New chat's menu: every agent once, in view.
      assertWall(await agentWall(page), "1440px");
      assert.equal(await page.getByRole("list", { name: "Agents Ghost runs" }).count(), 1);
      // The sidebar: the home page's parts and the pages of their own, Overview in view.
      const main = page.getByRole("navigation", { name: "Main" });
      assert.equal(await main.getByRole("link").count(), 6);
      assert.equal(
        await main.getByRole("link", { name: "Overview" }).getAttribute("aria-current"),
        "location",
      );
      const side = await page.locator(".site-nav").evaluate((nav) => {
        const box = nav.getBoundingClientRect();
        return {
          left: box.left,
          width: box.width,
          height: box.height,
          position: getComputedStyle(nav).position,
        };
      });
      assert.equal(side.position, "fixed", "the sidebar stays put");
      assert.equal(side.left, 0);
      assert.ok(side.width >= 200 && side.width <= 260, `the sidebar's width: ${side.width}`);
      await settled(page);
      await page.screenshot({ path: join(artifacts, `${name}-home-dark.png`), fullPage: true });
      await page.reload();
      // One statement opens the page, its words in the markup.
      await page
        .getByRole("heading", { level: 1, name: "Every coding agent, in one window." })
        .waitFor();
      await page.evaluate(() => window.scrollTo(0, 0));
      await settled(page);
      await audit(page, "the home page");
      // This Windows WebKit harness does not expose OS full-keyboard-access settings.
      // Chromium verifies Tab order; WebKit verifies the same link's explicit focus and activation.
      if (name === "webkit") await page.getByRole("link", { name: "Skip to content" }).focus();
      else await page.keyboard.press("Tab");
      assert.equal(
        await page.evaluate(() => document.activeElement?.textContent),
        "Skip to content",
      );
      await page.keyboard.press("Enter");
      assert.equal(new URL(page.url()).hash, "#main");
      await page.evaluate(() => document.activeElement?.blur());
      // The opening's object is decoration: the wing as a slab, hidden from assistive technology,
      // with no canvas. Nothing plays that would need a Pause button.
      assert.equal(await page.locator("canvas").count(), 0);
      assert.equal(await page.locator('.hw3[aria-hidden="true"]').isVisible(), true);
      assert.equal(await page.getByRole("button", { name: /pause|play/i }).count(), 0);
      // No page shows a screenshot: the app is drawn in HTML.
      assert.equal(await page.locator("main img, main picture").count(), 0);
      // The FAQ opens on its first answer and keeps one open at a time.
      assert.equal(await page.locator("details.faq-item[open]").count(), 1);
      assert.equal(await page.locator("details.faq-item").first().getAttribute("open"), "");
      // What the app does: six parts, each a picture in a panel with its name and two lines
      // beside it, all shown; the pictures are decoration. The rest of the workspace: eight rows.
      const six = page.locator(".six-item");
      assert.equal(await six.count(), 6);
      for (const part of await six.all()) {
        assert.equal(await part.locator(".six-text h3").isVisible(), true);
        assert.equal(await part.locator(".six-text p").isVisible(), true);
        assert.equal(await part.locator(".six-panel").getAttribute("aria-hidden"), "true");
      }
      assert.equal(await page.locator(".part").count(), 8);
      await page.locator(".stat-panel").scrollIntoViewIfNeeded();
      await page
        .locator(".stat-panel")
        .screenshot({ path: join(artifacts, `${name}-handoff.png`) });
      await page.evaluate(() => document.activeElement?.blur());
      await page.setViewportSize({ width: 390, height: 844 });
      await page.evaluate(() => document.activeElement?.blur());
      await page.setViewportSize({ width: 390, height: 844 });
      await page
        .locator(".stat-panel")
        .screenshot({ path: join(artifacts, `${name}-handoff-mobile.png`) });
      await page.screenshot({ path: join(artifacts, `${name}-mobile.png`), fullPage: true });
      assert.ok(await sideways(page));
      await settled(page);
      assertWall(await agentWall(page), "390px");
      // In one column a part's panel stands under its words.
      const stacked = await page.evaluate(() => {
        const text = document.querySelector(".agents .split-text").getBoundingClientRect();
        const panel = document.querySelector(".agents .split-panel").getBoundingClientRect();
        return text.bottom <= panel.top + 0.5;
      });
      assert.ok(stacked, "the agents' panel stands under their words on a phone");
      // Below 1024px the bar's Download waits while the hero's own is in view, and comes back
      // once that has scrolled away.
      const barAction = () =>
        page.evaluate(() => getComputedStyle(document.querySelector(".nav-cta")).visibility);
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.waitForFunction(
        () => getComputedStyle(document.querySelector(".nav-cta")).visibility === "hidden",
      );
      await page.evaluate(() => window.scrollTo(0, 1600));
      await page.waitForFunction(
        () => getComputedStyle(document.querySelector(".nav-cta")).visibility === "visible",
      );
      assert.equal(await barAction(), "visible");
      // The phone menu is a solid sheet under the bar, over whatever the page shows behind it.
      await page.evaluate(() => {
        const band = document.querySelector("#features");
        window.scrollTo(0, band.getBoundingClientRect().top + window.scrollY + 400);
      });
      await page.locator(".nav-menu-button").click();
      await page.getByRole("navigation", { name: "Menu" }).waitFor();
      // The menu holds the sidebar's six links, the public repository and nothing else: there
      // is no account to sign in to.
      assert.equal(
        await page.getByRole("navigation", { name: "Menu" }).getByRole("link").count(),
        7,
      );
      await page.waitForFunction(() =>
        document
          .querySelector(".nav-sheet")
          .getAnimations()
          .every((animation) => animation.playState === "finished"),
      );
      await settled(page);
      await audit(page, "the phone menu", ".nav-sheet");
      assert.equal(
        await page
          .locator(".nav-sheet")
          .evaluate((sheet) => getComputedStyle(sheet).backgroundColor),
        PAGE,
        "the phone menu is solid",
      );
      // And it covers the page from under the bar to the screen's foot.
      const cover = await page.locator(".nav-sheet").evaluate((sheet) => {
        const box = sheet.getBoundingClientRect();
        const hit = document.elementFromPoint(window.innerWidth / 2, window.innerHeight - 140);
        return { bottom: box.bottom, height: window.innerHeight, covers: sheet.contains(hit) };
      });
      assert.ok(cover.bottom >= cover.height - 1, "the phone menu reaches the screen's foot");
      assert.ok(cover.covers, "the phone menu covers the page");
      await page.keyboard.press("Escape");
      await page.getByRole("navigation", { name: "Menu" }).waitFor({ state: "hidden" });
      // A link to a section of this page closes the menu and goes there.
      await page.locator(".nav-menu-button").click();
      await page
        .getByRole("navigation", { name: "Menu" })
        .getByRole("link", { name: "FAQ" })
        .click();
      await page.getByRole("navigation", { name: "Menu" }).waitFor({ state: "hidden" });
      assert.equal(new URL(page.url()).hash, "#faq");
      await page.evaluate(() => window.scrollTo(0, 0));
      // The narrowest phones still in use: nothing may scroll sideways at 320px either.
      await page.setViewportSize({ width: 320, height: 640 });
      assert.ok(await sideways(page), "the home page scrolls sideways at 320px");
      assertWall(await agentWall(page), "320px");
      await settled(page);
      await audit(page, "the home page at 320px");
      // The changelog page is built from CHANGELOG.md: its newest day leads, every change names its
      // pull request without a link, and it holds at the narrowest width.
      await page.goto(`${base}/changelog`);
      await page.getByRole("heading", { name: "Changelog. Every change, as it lands." }).waitFor();
      assert.ok((await page.locator(".change").count()) > 100, "the changelog lists its changes");
      assert.equal(await page.locator(".change a").count(), 0);
      assert.match(await page.locator(".release-meta").first().textContent(), /20\d\d/);
      assert.ok(await sideways(page), "the changelog scrolls sideways at 320px");
      await settled(page);
      await audit(page, "the changelog page");
      await page.goto(`${base}/download`);
      // The download page follows the committed catalog: its installers once one is published,
      // and three disabled buttons until then.
      const { release } = JSON.parse(
        await readFile(new URL("../app/release-catalog.json", import.meta.url), "utf8"),
      );
      const waiting = page.getByRole("button", { name: "Installer coming soon" });
      if (release) {
        for (const download of release.downloads)
          assert.equal(
            await page
              .getByRole("link", { name: `Download ${download.label}`, exact: true })
              .getAttribute("href"),
            counted(download.url),
          );
        assert.equal(await waiting.count(), 0);
        assert.ok(await sideways(page), "the download page scrolls sideways at 320px");
        await settled(page);
        await audit(page, "the download page");
      } else {
        assert.equal(await waiting.count(), 3);
      }
      for (const path of ["privacy", "terms"]) {
        await page.goto(`${base}/${path}`);
        await page.getByRole("heading", { level: 1 }).waitFor();
        assert.equal(await pageColour(page), PAGE, path);
        assert.ok(await sideways(page), `the ${path} page scrolls sideways at 320px`);
        await settled(page);
        await audit(page, `the ${path} page`);
      }
      // The stored value is left as it was: the site keeps no appearance of its own.
      assert.equal(await page.evaluate(() => localStorage.getItem("auracode-appearance")), "light");
      assert.deepEqual(failures, []);
      // Full motion, under a system that asks for light: the page is dark all the same, the wing
      // flies in and rides the air, the handoff loop runs, New chat's menu moves through the
      // agents by itself, and nothing errors or scrolls sideways.
      const moving = await browser.newContext({
        viewport: { width: 1440, height: 900 },
        reducedMotion: "no-preference",
        colorScheme: "light",
      });
      const motion = await moving.newPage();
      const motionFailures = [];
      motion.on("pageerror", (error) => motionFailures.push(error.message));
      motion.on("console", (message) => {
        if (message.type() === "error") motionFailures.push(message.text());
      });
      await motion.route("**/*", (route) =>
        route.request().url().startsWith(base) ? route.continue() : route.abort(),
      );
      await motion.goto(base);
      await motion.waitForFunction(
        () => !document.querySelector(".hw")?.hasAttribute("data-settled"),
        null,
        {
          timeout: 10000,
        },
      );
      // The wing rides the air.
      assert.ok(
        await motion.evaluate(() => document.querySelector(".hw3-idle").getAnimations().length > 0),
        "the wing moves",
      );
      await motion.waitForTimeout(1200);
      await motion.screenshot({ path: join(artifacts, `${name}-motion-hero.png`) });
      assert.equal(await pageColour(motion), PAGE);
      assert.equal(await motion.locator("html").getAttribute("data-theme"), "dark");
      for (const width of [1440, 2560, 1024, 768, 390, 320]) {
        await motion.setViewportSize({ width, height: 900 });
        await motion.evaluate(() => window.scrollTo(0, 0));
        assertWall(await agentWall(motion), `${width}px with motion`);
        assert.ok(await sideways(motion), `the moving home page scrolls sideways at ${width}px`);
      }
      await motion.setViewportSize({ width: 1440, height: 900 });
      // In view, New chat's menu moves on to its second agent by itself.
      await motion.locator(".pick-panel").scrollIntoViewIfNeeded();
      await motion.waitForFunction(
        () => document.querySelectorAll(".pick-row")[1]?.hasAttribute("data-chosen"),
        null,
        { timeout: 15000 },
      );
      const pageHeight = await motion.evaluate(() => document.documentElement.scrollHeight);
      for (let top = 900; top < pageHeight; top += 900) {
        await motion.evaluate((y) => window.scrollTo(0, y), top);
        await motion.waitForTimeout(350);
        assert.ok(await sideways(motion), `the moving home page scrolls sideways at ${top}`);
      }
      await motion.evaluate(() => window.scrollTo(0, document.documentElement.scrollHeight));
      // Every block has arrived by now, so the page can be audited whole.
      await motion.waitForFunction(
        () => document.querySelectorAll("[data-reveal]:not([data-revealed])").length === 0,
      );
      await motion.waitForTimeout(1200);
      await audit(motion, "the home page with motion, under a light system");
      await motion.screenshot({ path: join(artifacts, `${name}-motion-end.png`) });
      await motion.setViewportSize({ width: 390, height: 844 });
      await motion.evaluate(() => window.scrollTo(0, 0));
      await motion.waitForTimeout(600);
      await motion.screenshot({ path: join(artifacts, `${name}-motion-mobile.png`) });
      assert.ok(await sideways(motion), "the moving home page scrolls sideways at 390px");
      assert.deepEqual(motionFailures, []);
      await moving.close();
      const nojs = await browser.newContext({ javaScriptEnabled: false, colorScheme: "light" });
      const staticPage = await nojs.newPage();
      // Without scripts the page is whole: the agents in their menu, the six parts of the app
      // with their words and pictures, the eight rows, and the wing.
      await staticPage.goto(base);
      for (const width of [1440, 390]) {
        await staticPage.setViewportSize({ width, height: 900 });
        assertWall(await agentWall(staticPage), `${width}px without scripts`);
      }
      assert.equal(await staticPage.locator(".six-text:visible").count(), 6);
      assert.equal(await staticPage.locator(".part:visible").count(), 8);
      assert.equal(await staticPage.locator(".hw3").isVisible(), true);
      assert.equal(await staticPage.locator("details.faq-item[open]").count(), 1);
      await staticPage.goto(`${base}/download`);
      // Without scripts the page names every system, and every installer once one is published.
      for (const name of release
        ? release.downloads.map((download) => download.label)
        : ["Windows", "macOS", "Linux"])
        await staticPage.getByRole("heading", { name, exact: true }).waitFor();
      if (release)
        for (const download of release.downloads)
          assert.equal(
            await staticPage
              .getByRole("link", { name: `Download ${download.label}`, exact: true })
              .getAttribute("href"),
            counted(download.url),
          );
      // The appearance needs no script: dark, under a system that asks for light.
      assert.equal(await pageColour(staticPage), PAGE);
      await nojs.close();
      await context.close();
      console.log(
        `${name}: always dark, desktop/mobile, motion and stillness, no-JS, keyboard and accessibility passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  await new Promise((resolve) => server.close(resolve));
}
