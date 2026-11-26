/** Browser acceptance for real ChatsScreen header routing. Synthetic metadata only. */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
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
const out = resolve(app, "../../target/header-labels-browser");
const url = values.server ?? "http://127.0.0.1:1461";
const require = createRequire(import.meta.url);
const config = JSON.parse(await readFile(resolve(app, "src-tauri/tauri.conf.json"), "utf8"));
const windowConfig = config.app.windows.find((window) => window.label === "main");
assert.ok(windowConfig.minWidth > 0 && windowConfig.minHeight > 0);
const sizes = [
  { width: 1440, height: 900 },
  { width: 1024, height: 700 },
  { width: windowConfig.minWidth, height: windowConfig.minHeight },
];
const cases = [
  {
    id: "header-tree",
    title: "Review release checklist",
    label:
      "Client portal · ghost/9d6cd062 (worktree from C:/Users/Example/Work/Customer Platforms/October Delivery/Shared Engineering Workspaces/Client Portal/desktop-and-web-application)",
    visible: "Client portal · ghost/9d6cd062",
    path: "C:/Users/Example/AppData/Local/auracode/profiles/local/worktrees/9d6cd062e86e",
  },
  {
    id: "header-source",
    title: "Local source folder",
    label: "Client portal · portal-source",
    visible: "Client portal · portal-source",
    path: "C:/Users/Example/Work/Customer Platforms/October Delivery/Shared Engineering Workspaces/Client Portal/desktop-and-web-application",
  },
  {
    id: "header-documents",
    title: "Local second folder",
    label: "Client portal · Design notes",
    visible: "Client portal · Design notes",
    path: "C:/Projects/Client Portal/design-notes",
  },
  {
    id: "header-notes",
    title: "Local matching names",
    label: "notes",
    visible: "notes",
    path: "C:/Projects/notes",
  },
];
let server;
let serverOutput = "";
await mkdir(out, { recursive: true });

async function ready() {
  for (let attempt = 0; attempt < 100; attempt++) {
    if (server?.exitCode !== null && server?.exitCode !== undefined) throw new Error(serverOutput);
    try {
      if ((await fetch(url)).ok) return;
    } catch {
      // The owned Vite process has not bound its port yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Preview did not start: ${serverOutput}`);
}

async function inspect(engine, launcher) {
  const browser = await launcher.launch({ headless: true });
  const errors = [];
  const results = [];
  try {
    const context = await browser.newContext({ reducedMotion: "reduce", colorScheme: "dark" });
    await context.route("**/*", (route) =>
      route.request().url().startsWith(url) ? route.continue() : route.abort(),
    );
    const page = await context.newPage();
    page.on("pageerror", (error) => errors.push(error.message));
    for (const theme of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme: theme });
      for (const size of sizes) {
        await page.setViewportSize(size);
        await page.goto(`${url}/?preview=header-labels`);
        await page.locator('[data-testid="chat-view"][data-test-phase="live"]').waitFor();
        await page.evaluate(() => document.fonts.ready);
        for (const item of cases) {
          const key = `${engine}-${theme}-${size.width}x${size.height}-${item.id}`;
          const row = page.locator(`[data-testid="chat-row"][data-test-chat-id="${item.id}"]`);
          await row.click();
          await page
            .locator(
              `[data-testid="chat-view"][data-test-chat-id="${item.id}"][data-test-phase="live"]`,
            )
            .waitFor();
          const label = page.locator(".chat-header-folder");
          await page.waitForFunction(
            (expected) => document.querySelector(".chat-header-folder")?.textContent === expected,
            item.label,
          );
          assert.equal(await row.getAttribute("aria-current"), "true", key);
          assert.equal(await page.locator(".chat-header-title").textContent(), item.title, key);
          assert.equal(await label.textContent(), item.label, key);
          assert.equal(await label.getAttribute("title"), item.path, key);
          assert.equal(
            await page.getByRole("button", { name: "Changes", exact: true }).isVisible(),
            true,
            key,
          );
          const menu = page.locator(".chat-header-menu summary");
          assert.equal(await menu.isVisible(), true, key);
          assert.equal(await menu.getAttribute("aria-label"), "Chat details and controls", key);
          const geometry = await label.evaluate((element, prefix) => {
            const box = element.getBoundingClientRect();
            const controls = document
              .querySelector(".chat-header-controls")
              .getBoundingClientRect();
            const range = document.createRange();
            range.setStart(element.firstChild, 0);
            range.setEnd(element.firstChild, prefix.length);
            const visible = range.getBoundingClientRect();
            return {
              pageOverflow: document.documentElement.scrollWidth > innerWidth,
              width: box.width,
              controlsClear: box.right <= controls.left + 1,
              prefixVisible: visible.right <= box.right + 1,
              clipped: element.scrollWidth > element.clientWidth,
              textOverflow: getComputedStyle(element).textOverflow,
            };
          }, item.visible);
          assert.equal(geometry.pageOverflow, false, `${key}: page overflow`);
          assert.ok(geometry.width > 0, `${key}: hidden header label`);
          assert.equal(geometry.controlsClear, true, `${key}: header controls overlap`);
          assert.equal(geometry.prefixVisible, true, `${key}: project/branch name clipped`);
          assert.equal(geometry.textOverflow, "ellipsis", `${key}: path truncation`);
          if (item.id === "header-tree")
            assert.equal(geometry.clipped, true, `${key}: long fixture`);
          const scan = await new AxeBuilder({ page })
            .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"])
            .analyze();
          assert.deepEqual(scan.violations, [], `${key}: accessibility violations`);
          await page.screenshot({ path: resolve(out, `${key}.png`) });
          results.push({ size, chat: item.id, label: item.label, geometry, violations: [] });
        }
      }
    }
    assert.deepEqual(errors, [], `${engine}: browser errors`);
    console.log(
      `${engine}: ${results.length} header selections, layout views and accessibility scans passed`,
    );
  } finally {
    await writeFile(
      resolve(out, `${engine}-results.json`),
      `${JSON.stringify({ errors, results }, null, 2)}\n`,
    );
    await browser.close();
  }
}

try {
  if (!values.server) {
    const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
    server = spawn(
      process.execPath,
      [vite, "--host", "127.0.0.1", "--port", "1461", "--strictPort"],
      {
        cwd: app,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      },
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
  if (server?.exitCode === null) {
    const exited = new Promise((resolve) => server.once("exit", resolve));
    server.kill();
    await exited;
  }
}
