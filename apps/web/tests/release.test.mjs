// Offline synthetic release qualification. No network, release keys, installers or publication.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, webkit } from "playwright";

/** A download button goes through the site's counter (app/counted.ts). */
const counted = (url) =>
  url.replace(/^https:\/\/github\.com\/itscool2b\/(?:o7|auracode)\/releases\/download\//, "/get/");

const app = fileURLToPath(new URL("..", import.meta.url));
const repo = resolve(app, "../..");
const require = createRequire(import.meta.url);
const router = join(dirname(require.resolve("@react-router/dev/package.json")), "bin.js");
const env = { ...process.env };
delete env.O7_RELEASE_STAGE;
const cargo = env.CARGO ?? "cargo";
function run(command, args, variables = env, cwd = repo) {
  const result = spawnSync(command, args, {
    cwd,
    env: variables,
    encoding: "utf8",
    windowsHide: true,
    timeout: 180000,
    maxBuffer: 2 * 1024 * 1024,
  });
  assert.equal(result.error, undefined, result.error?.message);
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
  return result;
}
await mkdir(join(repo, "target"), { recursive: true });
const scratch = await mkdtemp(join(repo, "target/release-preview-"));
const artifacts = join(scratch, "artifacts");
const stage = join(scratch, "stage");
await mkdir(artifacts);
const fixture = JSON.parse(
  await readFile(join(repo, "crates/o7-release/tests/fixtures/signatures.json"), "utf8"),
);
env.O7_UPDATER_PUBLIC_KEY = fixture.public_key;
const entries = [
  ["macos_universal_dmg", "o7-universal.dmg"],
  ["macos_universal_updater", "o7.app.tar.gz"],
  ["windows_x64_nsis", "o7-x64.exe"],
  ["windows_arm64_nsis", "o7-arm64.exe"],
  ["linux_x64_appimage", "auracode-x86_64.AppImage"],
  ["linux_x64_deb", "auracode-amd64.deb"],
  ["linux_x64_rpm", "auracode-x86_64.rpm"],
];
const plan = {
  channel: "staging",
  version: "0.1.0",
  pub_date: "2026-10-26T12:00:00Z",
  notes: "Synthetic browser fixture. Not an installer.\n<script>not executable</script>",
  public_key: fixture.public_key,
  artifacts: [],
};
for (const [kind, file] of entries) {
  await writeFile(join(artifacts, file), fixture.artifact);
  const signature = kind === "macos_universal_dmg" ? null : `${file}.sig`;
  if (signature) await writeFile(join(artifacts, signature), fixture.signature);
  plan.artifacts.push({ kind, file, signature });
}
await writeFile(join(scratch, "plan.json"), JSON.stringify(plan));
run(cargo, [
  "run",
  "--locked",
  "-p",
  "o7-release",
  "--",
  "prepare",
  join(scratch, "plan.json"),
  artifacts,
  stage,
]);
try {
  run(process.execPath, [router, "build"], { ...env, O7_RELEASE_STAGE: stage }, app);
  const html = await readFile(join(app, "build/client/download/index.html"), "utf8");
  const catalog = JSON.parse(await readFile(join(stage, "downloads.json"), "utf8"));
  assert.match(html, /Staging preview/);
  assert.doesNotMatch(html, /Installer coming soon|Not published/);
  // A stage has updater signatures, so the page says an installed copy updates itself, and
  // without a publisher's signature it says each system warns and how to open the installer.
  assert.equal(catalog.release.signed, true);
  assert.equal(catalog.release.publisher_signed, false);
  assert.doesNotMatch(html, /These installers are unsigned/);
  assert.match(html, /Once installed, o7 updates itself/);
  assert.match(html, /Run anyway/);
  for (const download of catalog.release.downloads)
    assert.match(
      download.url,
      /^https:\/\/github\.com\/itscool2b\/auracode\/releases\/download\/staging-v/,
    );
  assert.match(html, /&lt;script&gt;not executable&lt;\/script&gt;/);
  for (const download of catalog.release.downloads) {
    assert.ok(html.includes(`href="${counted(download.url)}"`));
    assert.ok(html.includes(download.sha256));
  }
  const build = join(app, "build/client");
  const server = createServer(async (request, response) => {
    try {
      let path = decodeURIComponent(new URL(request.url, "http://localhost").pathname);
      if (!extname(path)) path += "/index.html";
      const file = resolve(build, `.${path}`);
      if (!file.startsWith(build + sep)) throw new Error("Outside static build");
      const types = {
        ".html": "text/html",
        ".css": "text/css",
        ".woff2": "font/woff2",
        ".woff": "font/woff",
        ".png": "image/png",
      };
      const body = await readFile(file);
      response.writeHead(200, {
        "Content-Type": types[extname(file)] ?? "application/octet-stream",
      });
      response.end(body);
    } catch {
      response.writeHead(404);
      response.end("Not found");
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;
  // Inspect the already rendered document with scripting disabled. No request leaves this test.
  try {
    for (const [engine, launcher] of Object.entries({ chromium, webkit })) {
      const browser = await launcher.launch();
      try {
        const context = await browser.newContext({
          javaScriptEnabled: false,
          viewport: { width: 1440, height: 1100 },
        });
        await context.route("**/*", (route) =>
          route.request().url().startsWith(base) ? route.continue() : route.abort(),
        );
        const page = await context.newPage();
        await page.goto(`${base}/download`, { waitUntil: "networkidle" });
        for (const download of catalog.release.downloads) {
          assert.equal(
            await page
              .getByRole("link", { name: `Download ${download.label}`, exact: true })
              .getAttribute("href"),
            counted(download.url),
          );
        }
        assert.equal(
          await page.getByRole("link", { name: "All SHA-256 checksums" }).getAttribute("href"),
          catalog.release.checksums_url,
        );
        await page.screenshot({
          path: join(scratch, `${engine}-release-no-js.png`),
          fullPage: true,
        });
        await page.setViewportSize({ width: 390, height: 844 });
        assert.ok(
          await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        );
        console.log(
          `${engine}: all ${catalog.release.downloads.length} download links, checksums and release notes work without JavaScript`,
        );
      } finally {
        await browser.close();
      }
    }
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
  // A modified artifact prevents any site build from activating its catalog.
  await writeFile(join(stage, "releases/0.1.0/o7-x64.exe"), "tampered");
  const rejected = spawnSync(process.execPath, [router, "build"], {
    cwd: app,
    env: { ...env, O7_RELEASE_STAGE: stage },
    encoding: "utf8",
    timeout: 180000,
    windowsHide: true,
  });
  assert.notEqual(rejected.status, 0);
  assert.match(rejected.stderr, /release stage failed offline verification/);
} finally {
  // Leave the normal unpublished site in build/client after the synthetic check.
  run(process.execPath, [router, "build"], env, app);
  run(process.execPath, ["scripts/headers.mjs"], env, app);
}
