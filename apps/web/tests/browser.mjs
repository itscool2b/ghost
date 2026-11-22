// The built site in owned headless browsers. Requests to anything but the local server are refused.
import assert from "node:assert/strict";
import { mkdir, readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { chromium, webkit } from "playwright";

// Graphite settles a control's colours over 300ms when its state changes; audit the settled page.
const settled = (page) =>
  page.waitForFunction(() =>
    document.getAnimations().every((animation) => !(animation instanceof CSSTransition)),
  );

/**
 * The agents' chips: every agent named once, each wholly inside the viewport and none overlapping
 * the next, and nothing in them moving.
 */
const agentWall = (page) =>
  page.evaluate(() => {
    const shown = [...document.querySelectorAll(".agent-chip")].filter(
      (item) => item.getClientRects().length > 0,
    );
    const boxes = shown.map((item) => item.getBoundingClientRect());
    return {
      names: shown.map((item) => item.textContent),
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
      moving: shown.some((item) => item.getAnimations({ subtree: true }).length > 0),
    };
  });

const assertWall = (wall, where) => {
  assert.equal(wall.names.length, 9, `${where}: the row names every agent`);
  assert.equal(new Set(wall.names).size, 9, `${where}: each agent once`);
  assert.ok(wall.inView, `${where}: the row fits the page`);
  assert.ok(wall.apart, `${where}: no two names overlap`);
  assert.equal(wall.moving, false, `${where}: the row holds still`);
};

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
      await page.addInitScript(() => localStorage.setItem("auracode-appearance", "dark"));
      await page.goto(base);
      const heroStage = page.locator(".hero-detail .handoff-stage");
      assert.equal(await heroStage.locator(".handoff-prior").isVisible(), true);
      assert.equal(await heroStage.locator(".handoff-next").isVisible(), true);
      // The site follows the system appearance: this context asks for dark, so the page is dark,
      // with no switch, nothing pinned on the root and nothing read from storage.
      assert.equal(await page.locator("html").getAttribute("data-theme"), null);
      assert.equal(await page.getByLabel("Appearance").count(), 0);
      assert.equal(
        await page.evaluate(() => getComputedStyle(document.body).backgroundColor),
        "rgb(10, 10, 10)",
      );
      // The agents' chips: every agent once, in view.
      assertWall(await agentWall(page), "1440px");
      assert.equal(await page.getByRole("list", { name: "Agents o7 runs" }).count(), 1);
      assert.equal(
        await page.getByRole("navigation", { name: "Main" }).getByRole("link").count(),
        5,
      );
      const shot = () =>
        page.waitForFunction(() => {
          const image = document.querySelector(".showcase-frame img");
          return image?.complete && image.naturalWidth > 0;
        });
      await shot();
      await settled(page);
      await page.screenshot({ path: join(artifacts, `${name}-home-dark.png`), fullPage: true });
      await page.reload();
      await page.getByRole("heading", { name: "Meet o7" }).waitFor();
      await shot();
      // The real app is shown in the appearance the page is in.
      assert.match(
        await page.locator(".showcase-frame img").evaluate((image) => image.currentSrc),
        /desktop-dark-\d+\.webp$/,
      );
      await page.evaluate(() => window.scrollTo(0, 0));
      await page.screenshot({ path: join(artifacts, `${name}-home.png`), fullPage: true });
      await settled(page);
      let axe = await new AxeBuilder({ page })
        .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
        .analyze();
      assert.deepEqual(
        axe.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
        [],
      );
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
      // No canvas anywhere (the window is CSS) and no film Pause button.
      assert.equal(await page.locator("canvas").count(), 0);
      assert.equal(await page.getByRole("button", { name: /pause|play/i }).count(), 0);
      // The feature cards: six, two to a row as dot.com's, each a title over a product view; the
      // handoff shows both its parts.
      assert.equal(await page.locator(".work-card").count(), 6);
      // The FAQ opens on its first answer, as dot.com's does, and keeps one open at a time.
      assert.equal(await page.locator("details.faq-item[open]").count(), 1);
      assert.equal(await page.locator("details.faq-item").first().getAttribute("open"), "");
      const handoff = page.locator(".work-view .handoff-stage");
      await handoff.scrollIntoViewIfNeeded();
      assert.equal(await handoff.locator(".handoff-prior").isVisible(), true);
      assert.equal(await handoff.locator(".handoff-next").isVisible(), true);
      assert.match(await handoff.locator(".handoff-prior").textContent(), /Frame pacing now waits/);
      assert.match(
        await handoff.locator(".handoff-next").textContent(),
        /Next message includes a handoff to Codex/,
      );
      await handoff.screenshot({ path: join(artifacts, `${name}-handoff.png`) });
      // The agents' chips are toggle buttons: one is pressed, choosing another names it under the
      // chips and picks it in the New chat view, by pointer and by keyboard.
      const chips = page.locator(".agent-chip");
      const pressed = page.locator(".agent-chip[aria-pressed=true]");
      assert.equal(await chips.count(), 9);
      assert.equal(await pressed.count(), 1);
      for (const agent of ["Codex", "Hermes", "Pi"]) {
        const chip = page.getByRole("button", { name: agent, exact: true });
        await chip.click();
        assert.equal(await chip.getAttribute("aria-pressed"), "true");
        assert.equal(await pressed.count(), 1);
        assert.match(await page.locator(".agents-note").textContent(), new RegExp(`^${agent}, as`));
        await settled(page);
        axe = await new AxeBuilder({ page })
          .include(".agents")
          .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
          .analyze();
        assert.deepEqual(
          axe.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
          [],
          `agent ${agent}`,
        );
      }
      const cursor = page.getByRole("button", { name: "Cursor", exact: true });
      await cursor.focus();
      await page.keyboard.press("Enter");
      assert.equal(await cursor.getAttribute("aria-pressed"), "true");
      assert.equal(await page.locator(".agents-view .m-pill.is-on").textContent(), "Cursor");
      // The chosen chip shows its mark, in its agent's own colour; the others are names alone.
      assert.equal(
        await page.locator(".agent-chip[aria-pressed=true] .g-agent-mark").isVisible(),
        true,
      );
      assert.equal(
        await page.locator(".agent-chip[aria-pressed=false] .g-agent-mark:visible").count(),
        0,
      );
      await page.evaluate(() => document.activeElement?.blur());
      await page.setViewportSize({ width: 390, height: 844 });
      await handoff.screenshot({ path: join(artifacts, `${name}-handoff-mobile.png`) });
      await page.screenshot({ path: join(artifacts, `${name}-mobile.png`), fullPage: true });
      assert.ok(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
      );
      // A chip's colours settle over a moment after it is chosen; measure them once they have.
      await settled(page);
      assertWall(await agentWall(page), "390px");
      // Below 1024px the bar's Download waits while the hero's own is in view, and comes back
      // once that has scrolled away, as dot.com's does.
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
      // The menu holds the page's five sections, the public repository and nothing else: there
      // is no account to sign in to.
      assert.equal(
        await page.getByRole("navigation", { name: "Menu" }).getByRole("link").count(),
        6,
      );
      await page.waitForFunction(() =>
        document
          .querySelector(".nav-sheet")
          .getAnimations()
          .every((animation) => animation.playState === "finished"),
      );
      await settled(page);
      axe = await new AxeBuilder({ page })
        .include(".nav-sheet")
        .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
        .analyze();
      assert.deepEqual(
        axe.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
        [],
        "the phone menu",
      );
      assert.equal(
        await page
          .locator(".nav-sheet")
          .evaluate((sheet) => getComputedStyle(sheet).backgroundColor),
        "rgb(10, 10, 10)",
        "the phone menu is solid",
      );
      // And it covers the page from under the bar to the screen's foot, as dot.com's does.
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
      assert.ok(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        "the home page scrolls sideways at 320px",
      );
      assertWall(await agentWall(page), "320px");
      // The changelog page is built from CHANGELOG.md: its newest day leads, every change names its
      // pull request without a link, and it holds at the narrowest width.
      await page.goto(`${base}/changelog`);
      await page.getByRole("heading", { name: "Changelog. Every change, as it lands." }).waitFor();
      assert.ok((await page.locator(".change").count()) > 100, "the changelog lists its changes");
      assert.equal(await page.locator(".change a").count(), 0);
      assert.match(await page.locator(".release-meta").first().textContent(), /20\d\d/);
      assert.ok(
        await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        "the changelog scrolls sideways at 320px",
      );
      await settled(page);
      axe = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
      assert.deepEqual(
        axe.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
        [],
        "the changelog page",
      );
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
            download.url,
          );
        assert.equal(await waiting.count(), 0);
        assert.ok(
          await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
          "the download page scrolls sideways at 320px",
        );
        await settled(page);
        axe = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
        assert.deepEqual(
          axe.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
          [],
          "the download page",
        );
      } else {
        assert.equal(await waiting.count(), 3);
      }
      // The stored value is left as it was: the site keeps no appearance of its own.
      assert.equal(await page.evaluate(() => localStorage.getItem("auracode-appearance")), "dark");
      assert.deepEqual(failures, []);
      // Full motion: the hero's handoff loop, the reveals and the capability cycle run without
      // errors or sideways scrolling, and the loop leaves its printed first frame.
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
      await motion.waitForTimeout(1200);
      await motion.screenshot({ path: join(artifacts, `${name}-motion-hero.png`) });
      // The agents' chips hold still and fit at every width, with motion on as well. This context
      // asks for light, so the page is white and shows the light capture of the app.
      assert.equal(
        await motion.evaluate(() => getComputedStyle(document.body).backgroundColor),
        "rgb(255, 255, 255)",
      );
      for (const width of [1440, 2560, 1024, 768, 390, 320]) {
        await motion.setViewportSize({ width, height: 900 });
        await motion.evaluate(() => window.scrollTo(0, 0));
        assertWall(await agentWall(motion), `${width}px with motion`);
      }
      await motion.setViewportSize({ width: 1440, height: 900 });
      await motion.locator(".showcase-frame").scrollIntoViewIfNeeded();
      await motion.waitForFunction(() => {
        const image = document.querySelector(".showcase-frame img");
        return image?.complete && image.naturalWidth > 0;
      });
      assert.match(
        await motion.locator(".showcase-frame img").evaluate((image) => image.currentSrc),
        /desktop-light-\d+\.webp$/,
      );
      for (const top of [1100, 2200, 3300, 4400, 5500, 6400, 1e6]) {
        await motion.evaluate((y) => window.scrollTo(0, y), top);
        await motion.waitForTimeout(600);
        assert.ok(
          await motion.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
          `the moving home page scrolls sideways at ${top}`,
        );
      }
      // Every block has arrived by now, so the light page can be audited whole.
      await motion.waitForFunction(
        () => document.querySelectorAll("[data-reveal]:not([data-revealed])").length === 0,
      );
      await motion.waitForTimeout(1200);
      const lightAxe = await new AxeBuilder({ page: motion })
        .withTags(["wcag2a", "wcag2aa", "wcag21aa"])
        .analyze();
      assert.deepEqual(
        lightAxe.violations.map((v) => ({ id: v.id, nodes: v.nodes.map((n) => n.target) })),
        [],
        "the home page in light",
      );
      await motion.screenshot({ path: join(artifacts, `${name}-motion-end.png`) });
      await motion.setViewportSize({ width: 390, height: 844 });
      await motion.evaluate(() => window.scrollTo(0, 0));
      await motion.waitForTimeout(600);
      await motion.screenshot({ path: join(artifacts, `${name}-motion-mobile.png`) });
      assert.ok(
        await motion.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        "the moving home page scrolls sideways at 390px",
      );
      assert.deepEqual(motionFailures, []);
      await moving.close();
      const nojs = await browser.newContext({ javaScriptEnabled: false, colorScheme: "dark" });
      const staticPage = await nojs.newPage();
      // Without scripts the agents' chips are the same complete list, at desktop and phone widths.
      await staticPage.goto(base);
      for (const width of [1440, 390]) {
        await staticPage.setViewportSize({ width, height: 900 });
        assertWall(await agentWall(staticPage), `${width}px without scripts`);
      }
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
            download.url,
          );
      // The appearance needs no script: dark under a dark system.
      assert.equal(
        await staticPage.evaluate(() => getComputedStyle(document.body).backgroundColor),
        "rgb(10, 10, 10)",
      );
      await nojs.close();
      await context.close();
      console.log(
        `${name}: dark and light with the system, desktop/mobile, no-JS download, keyboard and accessibility passed`,
      );
    } finally {
      await browser.close();
    }
  }
} finally {
  await new Promise((resolve) => server.close(resolve));
}
