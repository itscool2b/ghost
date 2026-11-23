import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { access, readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

/** A download button goes through the site's counter (app/counted.ts). */
const counted = (url) =>
  url.replace(/^https:\/\/github\.com\/itscool2b\/(?:o7|auracode)\/releases\/download\//, "/get/");

const root = new URL("../build/client/", import.meta.url);
/** The committed catalog: `{ release: null }` until installers are published, then one release. */
const { release } = JSON.parse(
  await readFile(new URL("../app/release-catalog.json", import.meta.url), "utf8"),
);

/**
 * The Worker that serves the site embeds this folder and refuses a build it cannot plan
 * (apps/web-worker/src/manifest.rs, `walk` and `mime`). That check runs only in the deploy's Wasm
 * build, so its rules are mirrored here: #126's first build failed there on a chunk Vite had named
 * `__changelog-….js`.
 */
test("every built file is one the website Worker accepts", async () => {
  const served = /\.(?:js|css|json|txt|md|svg|png|jpe?g|webp|ico|woff2?)$|(?:^|\/)index\.html$/;
  const folder = fileURLToPath(root);
  const files = (await readdir(folder, { recursive: true, withFileTypes: true }))
    .filter((entry) => entry.isFile())
    .map((entry) => relative(folder, join(entry.parentPath, entry.name)).replaceAll("\\", "/"))
    .filter((path) => path !== "_headers" && path !== "__spa-fallback.html");
  assert.ok(files.length > 20 && files.length <= 1024, `${files.length} files`);
  for (const path of files) {
    assert.ok(
      path.split("/").every((part) => part !== "" && !/^[._]/.test(part)),
      `${path}: no part of a path may start with "." or "_"`,
    );
    assert.match(path, /^[A-Za-z0-9/_.-]+$/, `${path}: unsupported character`);
    assert.ok(path !== "api" && !path.startsWith("api/"), `${path}: reserved for the relay`);
    assert.match(path, served, `${path}: unsupported file type`);
  }
});
test("public pages are prerendered to follow the system appearance without JavaScript", async () => {
  for (const path of ["", "download/", "privacy/", "terms/", "changelog/"]) {
    const html = await readFile(new URL(`${path}index.html`, root), "utf8");
    // Nothing pins an appearance: the stylesheet follows prefers-color-scheme.
    assert.doesNotMatch(html, /<html[^>]*data-theme=/);
    assert.match(html, /<meta name="color-scheme" content="dark light"/);
    assert.match(html, /<main/);
    assert.match(html, /<h1/);
  }
  const downloads = await readFile(new URL("download/index.html", root), "utf8");
  assert.match(downloads, /macOS/);
  assert.match(downloads, /Windows/);
  assert.match(downloads, /Linux/);
  if (release) {
    // Published: every installer is linked beside its checksum, and nothing says otherwise.
    assert.equal(release.downloads.length, 6);
    for (const download of release.downloads) {
      assert.ok(downloads.includes(`href="${counted(download.url)}"`), download.file);
      assert.ok(downloads.includes(download.sha256), download.file);
      assert.match(
        download.url,
        /^https:\/\/(github\.com\/itscool2b\/(?:o7|auracode)\/releases\/download|download\.o7\.codes)\//,
      );
    }
    assert.ok(downloads.includes(`href="${release.checksums_url}"`));
    assert.doesNotMatch(downloads, /Not published|Installer coming soon/);
    // A build without an updater signature says it never updates itself; one with it says it
    // does. Either way, without a publisher's signature the page says how each system treats it.
    const never = /These installers are unsigned and do not update themselves/;
    const itself = /Once installed, o7 updates itself/;
    if (release.signed) assert.doesNotMatch(downloads, never);
    else assert.match(downloads, never);
    if (release.publisher_signed === true) {
      assert.doesNotMatch(downloads, /Run anyway/);
      assert.doesNotMatch(downloads, itself);
    } else {
      if (release.signed) assert.match(downloads, itself);
      assert.match(downloads, /Run anyway/);
      assert.match(downloads, /Open Anyway/);
      assert.match(downloads, /sudo apt install/);
    }
  } else {
    assert.match(downloads, /Not published/);
    assert.doesNotMatch(downloads, /href="[^"]+\.(exe|dmg)"/);
    assert.doesNotMatch(downloads, /These installers are unsigned/);
    assert.doesNotMatch(downloads, /o7 updates itself/);
  }
  await access(new URL("images/desktop-light.png", root));
  for (const theme of ["light", "dark"])
    for (const width of [480, 960, 1440, 2240])
      await access(new URL(`images/desktop-${theme}-${width}.webp`, root));
  const home = await readFile(new URL("index.html", root), "utf8");
  // The real app in the page's appearance: the dark capture under a dark system, else the light.
  assert.match(
    home,
    /<picture[^>]*><source[^>]*media="\(prefers-color-scheme: dark\)"[^>]*type="image\/webp"/,
  );
  for (const theme of ["light", "dark"])
    assert.ok(
      home.includes(
        `/images/desktop-${theme}-480.webp 480w, /images/desktop-${theme}-960.webp 960w, /images/desktop-${theme}-1440.webp 1440w`,
      ),
      theme,
    );
  // The frame is the page's 1232px column; a phone crops it to the chat pane at 179% (home.css).
  assert.match(
    home,
    /sizes="\(max-width: 639px\) calc\(179vw - 57px\), \(max-width: 1279px\) calc\(100vw - 48px\), 1232px"/,
  );
});
test("the changelog page lists every pull request in CHANGELOG.md, newest first", async () => {
  const source = await readFile(new URL("../../../CHANGELOG.md", import.meta.url), "utf8");
  const merged = [...source.matchAll(/^- .+ \(\[#(\d+)\]\(https:[^)]+\)\)\s*$/gm)].map((match) =>
    Number(match[1]),
  );
  assert.ok(merged.length > 100, "CHANGELOG.md was read");
  const html = await readFile(new URL("changelog/index.html", root), "utf8");
  // React splits "#" and the number into two text nodes.
  const shown = [...html.matchAll(/class="change-pull">.*?#(?:<!-- -->)?(\d+)<\/span>/g)].map(
    (match) => Number(match[1]),
  );
  assert.deepEqual(shown, merged);
  const days = [...source.matchAll(/^## (\d{4}-\d{2}-\d{2})\s*$/gm)].map((match) => match[1]);
  assert.deepEqual(
    [...html.matchAll(/<time dateTime="(\d{4}-\d{2}-\d{2})"/gi)].map((match) => match[1]),
    days,
  );
  // The home page shows the three newest days as cards, each leading to its day on this page.
  const home = await readFile(new URL("index.html", root), "utf8");
  assert.deepEqual(
    [...home.matchAll(/class="card news-card" href="\/changelog#day-(\d{4}-\d{2}-\d{2})"/g)].map(
      (match) => match[1],
    ),
    days.slice(0, 3),
  );
});
test("every page links the public repository from its header, menu and footer", async () => {
  for (const path of ["", "download/", "privacy/", "terms/", "changelog/"]) {
    const html = await readFile(new URL(`${path}index.html`, root), "utf8");
    const links = html.match(/href="https:\/\/github\.com\/itscool2b\/o7"/g) ?? [];
    assert.ok(links.length >= 3, `${path}: ${links.length} links to the repository`);
    assert.match(
      html,
      /class="btn btn-quiet btn-sm nav-github"[^>]*aria-label="o7 on GitHub"/,
      path,
    );
  }
});
test("static CSP authorizes only the emitted inline framework scripts", async () => {
  const headers = await readFile(new URL("_headers", root), "utf8");
  assert.match(headers, /frame-ancestors 'none'/);
  assert.doesNotMatch(headers, /script-src[^;]*'unsafe-inline'/);
  // No page may be framed, by anything: no rule lifts the global X-Frame-Options, and every
  // frame-ancestors is 'none'.
  assert.match(headers, /^\/\*\n(?: {2}.*\n)* {2}X-Frame-Options: DENY\n/);
  assert.doesNotMatch(headers, /^ {2}! /m);
  for (const [, ancestors] of headers.matchAll(/frame-ancestors ([^;\n]*)/g))
    assert.equal(ancestors, "'none'");
  // The site loads no third-party script or frame, and frames nothing itself.
  const policy = /^ {2}Content-Security-Policy: (default-src [^\n]*)$/m.exec(headers)?.[1];
  assert.match(policy, /; frame-src 'none';/);
  assert.match(policy, /script-src 'self'(?: 'sha256-[A-Za-z0-9+/]+=*')*;/);
  assert.doesNotMatch(headers, /https?:\/\//);
  assert.ok(headers.split("\n").every((line) => line.length <= 2000));
  for (const path of ["", "download/", "privacy/", "terms/", "changelog/", "remote/"]) {
    const html = await readFile(new URL(`${path}index.html`, root), "utf8");
    for (const match of html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)) {
      if (!/\bsrc\s*=/.test(match[1]) && match[2])
        assert.ok(
          headers.includes(`'sha256-${createHash("sha256").update(match[2]).digest("base64")}'`),
          path,
        );
    }
  }
});

test("the site is for a local app: no account pages, and no page offers one", async () => {
  // The account pages are not built. Sign-in, sync and the account service were removed.
  for (const path of ["signin", "signup", "account", "verify-device"])
    await assert.rejects(access(new URL(`${path}/index.html`, root)), path);
  for (const path of ["", "download/", "privacy/", "terms/", "changelog/"]) {
    const html = await readFile(new URL(`${path}index.html`, root), "utf8");
    assert.doesNotMatch(html, /href="\/(?:signin|signup|account|verify-device)\b/, path);
    assert.doesNotMatch(html, /href="auracode:/, path);
    // The changelog is the project's history, so only its own chrome is held to the wording; the
    // home page's three latest days quote it too.
    const text =
      path === "changelog/"
        ? html.replace(/<main[\s\S]*<\/main>/, "")
        : html.replace(/<ul class="inner news-grid">[\s\S]*?<\/ul>/, "");
    assert.doesNotMatch(text, /Sign in|Sign up|Create (?:your |an )?account|[Ii]nvit/, path);
    assert.doesNotMatch(text, /open.source|Choose a plan|Subscribe now/i, path);
  }
  const home = await readFile(new URL("index.html", root), "utf8");
  assert.match(home, /free/);
  assert.match(home, /There is no o7 account/);
  const privacy = await readFile(new URL("privacy/index.html", root), "utf8");
  assert.match(privacy, /There is no o7 account, no sign-in and no sync/);
});
test("third-party notices are emitted and linked from every page footer", async () => {
  const text = await readFile(new URL("third-party-notices.txt", root), "utf8");
  assert.match(text, /react-router/);
  assert.doesNotMatch(text, /SQLCipher|openssl-src/);
  const page = await readFile(new URL("third-party-notices/index.html", root), "utf8");
  // It follows the system appearance as the site does, without a script.
  assert.match(page, /color-scheme:light dark/);
  assert.match(page, /--notice-bg:#ffffff/);
  assert.match(page, /prefers-color-scheme:dark\)\{html\{--notice-bg:#0a0a0a/);
  assert.doesNotMatch(page, /<script/);
  const home = await readFile(new URL("index.html", root), "utf8");
  assert.match(home, /<a href="\/third-party-notices">Notices<\/a>/);
  const headers = await readFile(new URL("_headers", root), "utf8");
  assert.match(
    headers,
    /\/third-party-notices\*\n {2}Content-Security-Policy: frame-ancestors 'none'/,
  );
});

test("the remote page is a tool of its own, never cached, framed or indexed", async () => {
  const html = await readFile(new URL("remote/index.html", root), "utf8");
  // It fills the screen: no site header or footer around it.
  assert.doesNotMatch(html, /class="site-nav"|<footer/);
  assert.match(html, /<main id="main" class="remote">/);
  // Without JavaScript nothing can be opened or sealed, and the page says so.
  assert.match(html, /<noscript><p>This page needs JavaScript/);
  assert.match(html, /<meta name="robots" content="noindex"\/>/);
  assert.match(html, /<meta name="referrer" content="no-referrer"\/>/);
  const headers = await readFile(new URL("_headers", root), "utf8");
  assert.match(
    headers,
    /\n\/remote\*\n {2}Cache-Control: no-store\n {2}X-Robots-Tag: noindex, nofollow\n/,
  );
  assert.match(headers, /\n\/remote\*\n {2}Content-Security-Policy: frame-ancestors 'none'\n/);
  // The page talks to the relay on this site and nowhere else.
  const connect = /connect-src ([^;]*);/.exec(headers)?.[1];
  assert.equal(connect, "'self' wss://o7.codes wss://preview.o7.codes wss://auracode.si");
});
