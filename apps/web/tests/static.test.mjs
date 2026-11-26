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
test("public pages are prerendered dark, whole without JavaScript", async () => {
  for (const path of ["", "download/", "privacy/", "terms/", "changelog/"]) {
    const html = await readFile(new URL(`${path}index.html`, root), "utf8");
    // The site is always dark: pinned on the root, so the shared tokens never turn light, and
    // the browser is told the page has one scheme.
    assert.match(html, /<html[^>]*data-theme="dark"/, path);
    assert.match(html, /<meta name="color-scheme" content="dark"/, path);
    assert.doesNotMatch(html, /prefers-color-scheme: light/, path);
    assert.match(html, /<main/);
    assert.match(html, /<h1/);
  }
  // The remote page wears the paired computer's own theme, so nothing is pinned there.
  const remote = await readFile(new URL("remote/index.html", root), "utf8");
  assert.doesNotMatch(remote, /<html[^>]*data-theme=/);
  assert.match(remote, /<meta name="color-scheme" content="dark light"/);
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
  const home = await readFile(new URL("index.html", root), "utf8");
  // The app on the page is drawn in HTML, so the page ships no screenshot of it, and its one
  // picture, the hero's field, is whole without scripts: an SVG drawn when the page was built.
  assert.doesNotMatch(home, /<picture|<img\b/);
  assert.match(home, /<svg class="stage-field"[^>]*aria-hidden="true"/);
  assert.equal((home.match(/<canvas class="stage-canvas" aria-hidden="true">/g) ?? []).length, 3);
  // Every part of the page is in the markup: the six parts of the app with their words, the
  // nine agents and the eight tiles, none of them waiting on a script.
  assert.equal((home.match(/class="explorer-item"/g) ?? []).length, 6);
  assert.equal((home.match(/class="roster-agent"/g) ?? []).length, 9);
  assert.equal((home.match(/class="tile"/g) ?? []).length, 8);
  // No label stands over a heading (no eyebrows): each chapter's head is its heading and one
  // paragraph, nothing else.
  for (const [, head] of home.matchAll(/<div class="chapter-head">([\s\S]*?)<\/div>/g))
    assert.match(head, /^<h2 [^>]*class="title">[\s\S]*<\/h2><p class="lead">[\s\S]*<\/p>$/);
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
  // The home page shows the three newest days on a timeline, each leading to its day on this
  // page.
  const home = await readFile(new URL("index.html", root), "utf8");
  assert.deepEqual(
    [...home.matchAll(/class="timeline-day" href="\/changelog#day-(\d{4}-\d{2}-\d{2})"/g)].map(
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
        : html.replace(/<ol class="timeline">[\s\S]*?<\/ol>/, "");
    assert.doesNotMatch(text, /Sign in|Sign up|Create (?:your |an )?account|[Ii]nvit/, path);
    assert.doesNotMatch(text, /open.source|Choose a plan|Subscribe now/i, path);
  }
  const home = await readFile(new URL("index.html", root), "utf8");
  assert.match(home, /free/);
  assert.match(home, /There is no o7 account/);
  const privacy = await readFile(new URL("privacy/index.html", root), "utf8");
  assert.match(privacy, /There is no o7 account, no sign-in, and no payment/);
  assert.match(privacy, /BASH\.AI LLC/);
  const terms = await readFile(new URL("terms/index.html", root), "utf8");
  assert.match(terms, /Governing law and venue/);
  // No blank of the drafts is published.
  assert.doesNotMatch(terms + privacy, /\[(mailing address|effective date|DMCA agent)/);
});
test("third-party notices are emitted and linked from every page footer", async () => {
  const text = await readFile(new URL("third-party-notices.txt", root), "utf8");
  assert.match(text, /react-router/);
  assert.doesNotMatch(text, /SQLCipher|openssl-src/);
  const page = await readFile(new URL("third-party-notices/index.html", root), "utf8");
  // It is always dark, as the site is, without a script.
  assert.match(page, /color-scheme:dark;/);
  assert.match(page, /--notice-bg:#0a0a0a/);
  assert.doesNotMatch(page, /prefers-color-scheme/);
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
