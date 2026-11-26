/**
 * Regenerates the repository README's pictures (.github/assets/readme) from the desktop app
 * itself, as product-images.mjs does for the website: its chat preview (development fixtures
 * only, no agent or prompt), in light and in dark, at 2x, encoded through Chromium's canvas into
 * WebP. GitHub shows the one that matches the reader's appearance (<picture> in README.md).
 *
 * - hero: a chat at work, its window's corners rounded on a transparent ground with a hairline.
 * - menu: the model menu with Plan and build open.
 * - plan: a plan waiting for review, offering Accept and build with.
 * - wing-light.svg and wing-dark.svg: the logo in each appearance's ink.
 *
 * The preview's fixture project is called "auracode", the app's first name; the pictures show it
 * as "ghost", the repository's name.
 *
 * Usage: node apps/web/scripts/readme-images.mjs [--server http://127.0.0.1:1427]
 * Without --server it starts the desktop's Vite on 127.0.0.1:1427 (strictPort) and stops it after.
 */
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { chromium } from "playwright";

const { values } = parseArgs({ options: { server: { type: "string" } } });
const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const root = resolve(web, "../..");
const desktop = resolve(root, "apps/desktop");
const out = resolve(root, ".github/assets/readme");
const url = values.server ?? "http://127.0.0.1:1427";

/** The wing, as packages/ui/src/graphite/logoPaths.ts draws it: outline and eyes, even-odd. */
const { LOGO_PATH, LOGO_VIEWBOX } = await import(
  new URL("../../../packages/ui/src/graphite/logoPaths.ts", import.meta.url).href
);

let server = null;
let output = "";
if (!values.server) {
  const require = createRequire(resolve(desktop, "package.json"));
  const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
  server = spawn(
    process.execPath,
    [vite, "--host", "127.0.0.1", "--port", "1427", "--strictPort"],
    {
      cwd: desktop,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  for (const stream of [server.stdout, server.stderr])
    stream.on("data", (data) => {
      output = (output + data).slice(-8192);
    });
}

await mkdir(out, { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  for (let attempt = 0; ; attempt++) {
    if (server && server.exitCode !== null) throw new Error(output);
    if (attempt > 150) throw new Error(`The desktop preview did not start at ${url}.\n${output}`);
    const ok = await fetch(`${url}/?preview=settled`).then(
      (response) => response.ok && (!server || /ready in/i.test(output)),
      () => false,
    );
    if (ok) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  for (const theme of ["light", "dark"]) {
    const context = await browser.newContext({
      viewport: { width: 1440, height: 900 },
      deviceScaleFactor: 2,
      reducedMotion: "reduce",
      colorScheme: theme,
    });
    const page = await context.newPage();

    // The hero: a chat whose earlier turn is opened to its reasoning, its calls and its answer.
    await open(page, `preview=thinking&theme=${theme}&palette=aura&agents=lineup&roles=1`);
    await page.getByTestId("worked-for").first().click();
    await page.waitForTimeout(400);
    // The turn's reasoning opens too, where the fold shows it as a row of its own.
    const thought = page.locator('details:not([open]) > [data-testid="thought-toggle"]').first();
    if (await thought.count()) {
      await thought.click();
      await page.waitForTimeout(400);
    }
    await page.getByTestId("transcript").evaluate((node) => {
      node.scrollTop = 0;
    });
    await page.waitForTimeout(300);
    await rename(page);
    await rest(page);
    const hero = await page.screenshot();
    await encode(context, hero, `hero-${theme}.webp`, 2400, { radius: 24, rim: theme });

    // The model menu, with Plan and build open.
    await open(page, `preview=settled&theme=${theme}&palette=aura&agents=lineup&roles=1`);
    await page.locator(".chat-model-popover > summary").first().click();
    await page.locator(".chat-picker-roles > summary").click();
    const panel = page.locator(".chat-model-popover > .chat-popover-panel").first();
    await panel.evaluate((node) => {
      node.scrollTop = node.scrollHeight;
    });
    await page.waitForTimeout(300);
    await encode(context, await panel.screenshot(), `menu-${theme}.webp`, 920, {
      radius: 28,
      rim: theme,
    });

    // A plan waiting for review, offering the chat's Build with: a taller window, so the docked
    // card fits whole.
    await page.setViewportSize({ width: 1440, height: 1300 });
    await open(page, `preview=plan&theme=${theme}&palette=aura&roles=1`);
    const plan = page.locator('[data-kind="plan-approval"]').first();
    await plan.waitFor();
    await rename(page);
    await encode(context, await plan.screenshot(), `plan-${theme}.webp`, 1400, {
      radius: 32,
      rim: theme,
    });
    await context.close();
  }
  for (const [name, ink] of [
    ["wing-light.svg", "#0a0a0a"],
    ["wing-dark.svg", "#ededed"],
  ]) {
    const [width, height] = LOGO_VIEWBOX;
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}"><title>Ghost</title><path fill="${ink}" fill-rule="evenodd" d="${LOGO_PATH}"/></svg>\n`;
    await writeFile(resolve(out, name), svg);
    console.log(`${name}: ${svg.length} bytes`);
  }
} finally {
  await browser.close();
  server?.kill();
}

/** Opens a preview and waits until it has joined and its fonts are in. */
async function open(page, query) {
  await page.goto(`${url}/?${query}`);
  // A plan waiting for review docks in the composer's place, so the box is only attached.
  await page.getByTestId("composer-input").waitFor({ state: "attached" });
  await page.evaluate(() => document.fonts.ready);
  // Nothing under the hand: no hover fill, no tooltip.
  await page.mouse.move(1439, 899);
  await page.waitForTimeout(1200);
}

/** Nothing under the hand or focused: no hover fill, no focus ring, no tooltip. */
async function rest(page) {
  await page.mouse.move(1439, 899);
  await page.evaluate(() =>
    document.activeElement instanceof HTMLElement ? document.activeElement.blur() : null,
  );
  await page.waitForTimeout(400);
}

/** The fixture project's old name, as the app is called now. */
async function rename(page) {
  await page.evaluate(() => {
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode())
      if (node.nodeValue?.includes("auracode"))
        node.nodeValue = node.nodeValue.replaceAll("auracode", "ghost");
  });
}

/**
 * Writes `capture` (a PNG) as WebP `width` px wide, its corners rounded (`radius`, in output px)
 * on a transparent ground and edged with the appearance's hairline.
 */
async function encode(context, capture, name, width, { radius, rim }) {
  const encoder = await context.newPage();
  const dataUrl = await encoder.evaluate(
    async ({ source, width, radius, rim }) => {
      const image = new Image();
      image.src = source;
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = Math.round((width * image.height) / image.width);
      const context = canvas.getContext("2d");
      context.imageSmoothingEnabled = true;
      context.imageSmoothingQuality = "high";
      context.beginPath();
      context.roundRect(0.5, 0.5, canvas.width - 1, canvas.height - 1, radius);
      context.save();
      context.clip();
      context.drawImage(image, 0, 0, canvas.width, canvas.height);
      context.restore();
      context.lineWidth = 2;
      context.strokeStyle = rim === "dark" ? "rgba(255,255,255,0.14)" : "rgba(0,0,0,0.12)";
      context.stroke();
      return canvas.toDataURL("image/webp", 0.92);
    },
    { source: `data:image/png;base64,${capture.toString("base64")}`, width, radius, rim },
  );
  const bytes = Buffer.from(dataUrl.slice(dataUrl.indexOf(",") + 1), "base64");
  await writeFile(resolve(out, name), bytes);
  console.log(`${name}: ${bytes.length} bytes`);
  await encoder.close();
}
