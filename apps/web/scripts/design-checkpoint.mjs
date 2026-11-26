/** Reproducible local visual checkpoint. Run after `pnpm --filter @ghost/web build`.
 * `node apps/web/scripts/design-checkpoint.mjs --serve` keeps both preview servers available.
 * Fixtures are synthetic and never invoke an agent or shell. */
import { spawn } from "node:child_process";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, extname, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";
import { captureDesktopRoutes, captureSiteRoutes } from "./design-capture-extra.mjs";

const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const desktop = resolve(web, "../desktop");
const output = resolve(web, "../../target/visual-checkpoint");
const built = resolve(web, "build/client");
const serve = process.argv.includes("--serve");
const desktopUrl = "http://127.0.0.1:1438";
const galleryUrl = "http://127.0.0.1:1439";
const siteUrl = "http://127.0.0.1:1440";
await mkdir(output, { recursive: true });
const headers = await readFile(resolve(built, "_headers"), "utf8");
const csp = headers.match(/Content-Security-Policy: ([^\n]+)/)?.[1];
if (!csp) throw new Error("Build the website first: missing CSP");
const mime = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".png": "image/png",
  ".webp": "image/webp",
  ".woff2": "font/woff2",
  ".svg": "image/svg+xml",
};
const handle = async (request, response) => {
  try {
    let path = decodeURIComponent(new URL(request.url, galleryUrl).pathname);
    const site = request.socket.localPort === 1440;
    const root = site ? built : output;
    if (path.endsWith("/")) path += "index.html";
    else if (site && !extname(path)) path += "/index.html";
    const file = resolve(root, `.${path}`);
    if (!file.startsWith(root + sep)) throw new Error("Invalid path");
    const data = await readFile(file);
    response.writeHead(200, {
      "Content-Type": mime[extname(file)] ?? "application/octet-stream",
      ...(site ? { "Content-Security-Policy": csp } : {}),
    });
    response.end(data);
  } catch {
    response.writeHead(404);
    response.end("Not found");
  }
};
const http = createServer(handle);
const siteServer = createServer(handle);
await new Promise((ok, fail) => {
  http.once("error", fail);
  http.listen(1439, "127.0.0.1", ok);
});
await new Promise((ok, fail) => {
  siteServer.once("error", fail);
  siteServer.listen(1440, "127.0.0.1", ok);
});
const require = createRequire(resolve(desktop, "package.json"));
const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
const server = spawn(
  process.execPath,
  [vite, "--host", "127.0.0.1", "--port", "1438", "--strictPort"],
  { cwd: desktop, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] },
);
let logs = "";
for (const stream of [server.stdout, server.stderr])
  stream.on("data", (data) => {
    logs = (logs + data).slice(-8192);
  });
let browser;
try {
  for (let i = 0; ; i++) {
    if (server.exitCode !== null) throw new Error(`Desktop server exited: ${logs}`);
    if (i > 150) throw new Error(`Desktop preview did not start: ${logs}`);
    if (
      /ready in/i.test(logs) &&
      (await fetch(desktopUrl).then(
        (r) => r.ok,
        () => false,
      ))
    )
      break;
    await new Promise((done) => setTimeout(done, 100));
  }
  browser = await chromium.launch({ headless: true });
  const entries = [];
  const save = async (page, theme, id, title, fullPage = false) => {
    await page.evaluate(() => document.fonts.ready);
    await page.mouse.move(0, 0);
    const name = `${id}-${theme}.png`;
    await page.screenshot({ path: resolve(output, name), fullPage, animations: "disabled" });
    entries.push({ id, title, theme, name, viewport: page.viewportSize(), fullPage });
    console.log(name);
  };
  for (const theme of ["light", "dark"]) {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      deviceScaleFactor: 1,
      reducedMotion: "reduce",
      colorScheme: theme,
    });
    const page = await context.newPage();
    page.setDefaultTimeout(20000);
    await context.route("**/*", (route) => {
      const origin = new URL(route.request().url()).origin;
      return [siteUrl, desktopUrl].includes(origin) ? route.continue() : route.abort();
    });
    if (theme === "light") {
      await page.goto(`${siteUrl}/`);
      await page.getByRole("heading", { name: "One window for your coding agents" }).waitFor();
      await page
        .waitForFunction(() =>
          [...document.querySelectorAll(".product-frame img")].some(
            (i) => i.complete && i.naturalWidth > 0,
          ),
        )
        .catch(async () => {
          throw new Error(
            `Product images unavailable: ${JSON.stringify(await page.locator(".product-frame img").evaluateAll((images) => images.map((i) => [i.currentSrc, i.complete, i.naturalWidth])))}`,
          );
        });
      await page.evaluate(() => window.scrollTo(0, 0));
      await save(page, theme, "homepage-full", "Homepage, full page", true);
      await save(page, theme, "homepage-viewport", "Homepage, first viewport");
      await page.setViewportSize({ width: 390, height: 844 });
      await page.reload();
      await save(page, theme, "homepage-mobile", "Homepage, mobile", true);
      await page.setViewportSize({ width: 1440, height: 900 });
      await captureSiteRoutes(page, theme, save, siteUrl);
    }
    const preview = (mode, extra = "") =>
      `${desktopUrl}/?preview=${mode}&theme=${theme}&agents=lineup${extra}`;
    await page.goto(preview("settled"));
    await page.getByTestId("composer-input").waitFor();
    await page.getByRole("button", { name: "Changes", exact: true }).click();
    await page.getByRole("table", { name: "File line changes" }).waitFor();
    await save(page, theme, "chat-changes", "Chat and Changes");
    await page.goto(preview("settings"));
    await page.getByRole("radio", { name: "System", exact: true }).waitFor();
    await page.locator(".settings-main").evaluate((node) => {
      node.scrollTop = 0;
    });
    await save(page, theme, "settings-general", "Settings, General");
    await page.goto(preview("plugins"));
    await page.getByRole("heading", { name: "Plugin library", exact: true }).waitFor();
    await page.getByText("From claude plugin list --json --available").waitFor();
    await save(page, theme, "plugins", "Plugin library");
    await page.goto(preview("permission"));
    await page.locator("[data-test-request-id]").first().waitFor();
    await save(page, theme, "permission", "Permission request");
    await page.goto(preview("settled"));
    await page.getByTestId("composer-input").waitFor();
    await page.keyboard.press("Control+.");
    await page.locator(".chat-model-popover[open]").waitFor();
    await save(page, theme, "model-picker", "Agent and model picker");
    await page.goto(preview("terminal-panel", "&tabs=2&height=280"));
    await page.getByTestId("terminal-panel").waitFor();
    await page.locator('.terminal-panel [role="tab"][data-state="running"]').first().waitFor();
    await save(page, theme, "terminal-panel", "Terminal panel");
    await page.setViewportSize({ width: 960, height: 640 });
    await page.goto(preview("settled"));
    await page.getByTestId("composer-input").waitFor();
    await save(page, theme, "desktop-minimum", "Desktop, minimum size");
    await page.setViewportSize({ width: 1440, height: 900 });
    await captureDesktopRoutes(page, theme, save, desktopUrl);
    await context.close();
  }
  const data = JSON.stringify(entries).replaceAll("<", "\\u003c");
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Aura styling review</title><style>body{margin:0;background:#101112;color:#fafafa;font:15px/1.5 system-ui,sans-serif}header{position:sticky;top:0;background:#141516;padding:16px 24px;border-bottom:1px solid #303236;display:flex;gap:16px;align-items:center;flex-wrap:wrap}h1{font-size:18px;margin:0 auto 0 0}select,a{color:inherit;background:#1c1d1f;border:1px solid #666a70;border-radius:6px;padding:8px}main{max-width:1440px;margin:auto;padding:24px}h2{font-size:16px}img{display:block;max-width:100%;height:auto;border:1px solid #303236}p{color:#a1a1aa}</style></head><body><header><h1>Aura styling review</h1><label>Theme <select id="theme"><option>light</option><option>dark</option></select></label><label>View <select id="view"></select></label><a href="${siteUrl}/">Live website</a><a href="${desktopUrl}/?preview=settled">Live desktop fixture</a></header><main><h2 id="title"></h2><p>The website is always light. Desktop captures include light and dark. Captures use synthetic preview data; full homepage and mobile views scroll vertically.</p><img id="capture" alt=""></main><script>const items=${data},theme=document.querySelector('#theme'),view=document.querySelector('#view'),image=document.querySelector('#capture'),title=document.querySelector('#title'); function views(){const selected=view.value;view.replaceChildren();for(const item of items.filter(i=>i.theme===theme.value))view.add(new Option(item.title,item.id));if([...view.options].some(o=>o.value===selected))view.value=selected}function update(){const item=items.find(i=>i.theme===theme.value&&i.id===view.value);image.src=item.name;image.alt=item.title+' in '+item.theme;title.textContent=image.alt}theme.onchange=()=>{views();update()};view.onchange=update;views();update()</script></body></html>`;
  await writeFile(resolve(output, "index.html"), html);
  await writeFile(resolve(output, "manifest.json"), `${JSON.stringify(entries, null, 2)}\n`);
  console.log(`Gallery: ${output}/index.html`);
  if (serve) {
    console.log(`Live gallery: ${galleryUrl}/  Website: ${siteUrl}/  Desktop: ${desktopUrl}/`);
    await new Promise((resolve) => {
      process.once("SIGINT", resolve);
      process.once("SIGTERM", resolve);
    });
  }
} finally {
  await browser?.close();
  server.kill();
  await new Promise((done) => http.close(done));
  await new Promise((done) => siteServer.close(done));
}
