// Integration assertions exercise the compiled Rust Worker. No production services are contacted.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readdir, readFile } from "node:fs/promises";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { convertV4MiniflareOptions, Miniflare } from "miniflare";

const root = new URL("../", import.meta.url);
const publicRoot = new URL("../web/build/client/", root);
const routes = ["", "download", "privacy", "terms", "changelog", "remote", "third-party-notices"];
let runtime;
before(() => {
  runtime = new Miniflare({
    ...convertV4MiniflareOptions({
      modules: [
        { type: "ESModule", path: fileURLToPath(new URL("build/index.js", root)) },
        { type: "CompiledWasm", path: fileURLToPath(new URL("build/index_bg.wasm", root)) },
      ],
      compatibilityDate: "2026-10-26",
      outboundService() {
        assert.fail("the static Worker must never make an outbound request");
      },
      d1Databases: { COUNTS: "counts-test" },
    }),
    cf: false,
    host: "127.0.0.1",
    port: 0,
    telemetry: { enabled: false },
  });
});
before(async () => {
  const schema = await readFile(new URL("schema.sql", root), "utf8");
  const db = await runtime.getD1Database("COUNTS");
  await db.exec(schema.replace(/--.*
/g, "").replace(/s+/g, " ").trim());
});
after(async () => runtime?.dispose());
const totals = async () => {
  const db = await runtime.getD1Database("COUNTS");
  const { results } = await db
    .prepare("SELECT event, platform, version, n FROM daily ORDER BY event, platform")
    .all();
  return results.map((row) => `${row.event} ${row.platform} ${row.version} ${row.n}`);
};
const browser =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/131.0 Safari/537.36";
const request = (path, init = {}) =>
  runtime.dispatchFetch(`https://auracode.test${path}`, {
    ...init,
    headers: { "Accept-Encoding": "identity", ...init.headers },
  });
const security = (response) => {
  assert.equal(response.headers.get("x-content-type-options"), "nosniff");
  assert.equal(response.headers.get("referrer-policy"), "no-referrer");
  assert.equal(
    response.headers.get("permissions-policy"),
    "camera=(), microphone=(), geolocation=()",
  );
  assert.match(response.headers.get("content-security-policy"), /default-src 'self'/);
  assert.doesNotMatch(
    response.headers.get("content-security-policy"),
    /script-src[^;]*'unsafe-inline'/,
  );
  assert.equal(response.headers.get("set-cookie"), null);
};

test("prerendered pretty, trailing-slash and index routes serve exact public HTML", async () => {
  for (const route of routes) {
    const path = route ? `/${route}` : "/";
    const expected = await readFile(new URL(`${route ? `${route}/` : ""}index.html`, publicRoot));
    for (const alias of new Set([
      path,
      `${path.replace(/\/$/, "")}/`,
      `${path.replace(/\/$/, "")}/index.html`,
    ])) {
      const response = await request(`${alias}?source=synthetic`);
      assert.equal(response.status, 200, alias);
      assert.equal(response.headers.get("content-type"), "text/html; charset=utf-8");
      assert.equal(response.headers.get("content-length"), String(expected.length));
      assert.deepEqual(Buffer.from(await response.arrayBuffer()), expected, alias);
      security(response);
      // The remote page holds a session: it is never stored or indexed, and has no ETag.
      const noStore = route === "remote";
      assert.equal(
        response.headers.get("cache-control"),
        noStore ? "no-store" : "public, max-age=0, must-revalidate",
      );
      assert.equal(response.headers.get("x-robots-tag"), noStore ? "noindex, nofollow" : null);
      assert.equal(response.headers.get("etag") === null, noStore);
      // No page may be framed.
      const csp = response.headers.get("content-security-policy");
      assert.equal(response.headers.get("x-frame-options"), "DENY");
      assert.match(csp, /frame-ancestors 'none'/);
      assert.match(csp, /frame-src 'none'/);
      for (const script of expected.toString().matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)) {
        if (!/\bsrc\s*=/.test(script[1]) && script[2])
          assert.ok(
            csp.includes(`'sha256-${createHash("sha256").update(script[2]).digest("base64")}'`),
          );
      }
    }
  }
});

test("every public byte is embedded with its MIME and content-addressed cache policy", async () => {
  const mime = {
    js: "text/javascript; charset=utf-8",
    css: "text/css; charset=utf-8",
    png: "image/png",
    svg: "image/svg+xml",
    webp: "image/webp",
    txt: "text/plain; charset=utf-8",
    woff: "font/woff",
    woff2: "font/woff2",
  };
  async function visit(relative = "") {
    for (const entry of await readdir(new URL(relative, publicRoot), { withFileTypes: true })) {
      const path = `${relative}${entry.name}`;
      if (entry.isDirectory()) {
        await visit(`${path}/`);
        continue;
      }
      if (path.endsWith(".html") || path === "_headers") continue;
      const expected = await readFile(new URL(path, publicRoot));
      const response = await request(`/${path}`);
      assert.equal(response.status, 200, path);
      assert.equal(response.headers.get("content-type"), mime[path.split(".").at(-1)], path);
      assert.deepEqual(Buffer.from(await response.arrayBuffer()), expected, path);
      security(response);
      assert.equal(
        response.headers.get("etag"),
        `"${createHash("sha256").update(expected).digest("hex")}"`,
      );
      assert.equal(
        response.headers.get("cache-control"),
        path.startsWith("assets/")
          ? "public, max-age=31536000, immutable"
          : "public, max-age=0, must-revalidate",
        path,
      );
    }
  }
  await visit();
});

test("HEAD and conditional requests retain representation headers without response bytes", async () => {
  for (const path of ["/", "/download", "/remote", "/images/desktop-dark.png", "/missing.js"]) {
    const get = await request(path);
    const head = await request(path, { method: "HEAD" });
    assert.equal(head.status, get.status);
    for (const name of [
      "content-type",
      "content-length",
      "cache-control",
      "etag",
      "content-security-policy",
    ])
      assert.equal(head.headers.get(name), get.headers.get(name), name);
    assert.equal((await head.arrayBuffer()).byteLength, 0);
  }
  const get = await request("/download");
  const cached = await request("/download", {
    headers: { "If-None-Match": `"other", W/${get.headers.get("etag")}` },
  });
  assert.equal(cached.status, 304);
  assert.equal(cached.headers.get("etag"), get.headers.get("etag"));
  assert.equal(cached.headers.get("content-length"), null);
  assert.equal((await cached.arrayBuffer()).byteLength, 0);
  const remote = await request("/remote", { headers: { "If-None-Match": "*" } });
  assert.equal(remote.status, 200);
  assert.equal(remote.headers.get("etag"), null);
  assert.equal(remote.headers.get("cache-control"), "no-store");
});

test("missing files, internal artifacts, API paths and methods do not get a homepage fallback", async () => {
  for (const path of [
    "/missing",
    "/assets/missing.js",
    "/_headers",
    "/__spa-fallback.html",
    "/server/index.js",
    "/Cargo.toml",
    "/api",
    "/api/remote/v1/health",
    "/API/remote/v1/health",
    "/signin",
    "/account",
    "/verify-device",
  ]) {
    const response = await request(path);
    assert.equal(response.status, 404, path);
    assert.equal(response.headers.get("cache-control"), "no-store");
    assert.equal(response.headers.get("content-type"), "text/plain; charset=utf-8");
    assert.equal(await response.text(), "Not found\n");
    security(response);
  }
  assert.equal(
    (await request("/api/remote/v1/mcp/host", { method: "POST", body: "{}" })).status,
    404,
  );
  const post = await request("/download", { method: "POST", body: "synthetic=1" });
  assert.equal(post.status, 405);
  assert.equal(post.headers.get("allow"), "GET, HEAD");
  assert.equal(post.headers.get("cache-control"), "no-store");
});

test("encoded traversal, encoded separators and noncanonical public paths are refused", async () => {
  // WHATWG Request normalizes literal dot segments before user code. Remaining encoded paths
  // are refused; the immutable allowlist never opens a filesystem path from any request.
  for (const path of [
    "/%2e%2e%2findex.html",
    "/%252e%252e/index.html",
    "/assets%2fmissing.js",
    "/assets%5cmissing.js",
    "/%00",
    "/%64ownload",
    "//index.html",
  ]) {
    const response = await request(path);
    assert.equal(response.status, 400, path);
    assert.equal(response.headers.get("cache-control"), "no-store");
    assert.equal(await response.text(), "Invalid path\n");
  }
});

test("the former address keeps the remote page and sends every other page to the site", async () => {
  // auracode.si was the site's address until 19 November 2026. Nothing here leaves the machine:
  // the request is dispatched to the Worker under that name.
  const former = (path, init = {}) =>
    runtime.dispatchFetch(`https://auracode.si${path}`, {
      redirect: "manual",
      ...init,
      headers: { "Accept-Encoding": "identity", ...init.headers },
    });
  for (const path of ["/", "/download", "/changelog/", "/privacy/index.html", "/missing"]) {
    const response = await former(`${path}?source=synthetic`);
    assert.equal(response.status, 308, path);
    assert.equal(response.headers.get("location"), `https://o7.codes${path}`);
    assert.equal(await response.text(), "");
    security(response);
  }
  // The remote page a paired phone opens there, and the files it loads, are the site's own.
  const expected = await readFile(new URL("remote/index.html", publicRoot));
  const page = await former("/remote");
  assert.equal(page.status, 200);
  assert.deepEqual(Buffer.from(await page.arrayBuffer()), expected);
  assert.equal(page.headers.get("cache-control"), "no-store");
  assert.match(
    page.headers.get("content-security-policy"),
    /connect-src 'self' [^;]*wss:\/\/auracode\.si/,
  );
  security(page);
  const asset = /\/assets\/[\w.-]+\.js/.exec(expected.toString())?.[0];
  assert.ok(asset, "the remote page loads a script");
  assert.equal((await former(asset)).status, 200);
  // The relay's path is never a page, there or anywhere.
  assert.equal((await former("/api/remote/v1/health")).status, 404);
});

test("a download is counted once a browser asks, and always sent on to its file", async () => {
  const before = await totals();
  const get = (path, init = {}) =>
    request(path, { redirect: "manual", ...init, headers: { "User-Agent": browser, ...init.headers } });
  const response = await get("/get/v0.1.1748/o7_0.1.1748_x64-setup.exe");
  assert.equal(response.status, 302);
  assert.equal(
    response.headers.get("location"),
    "https://github.com/itscool2b/o7/releases/download/v0.1.1748/o7_0.1.1748_x64-setup.exe",
  );
  assert.equal(response.headers.get("cache-control"), "no-store");
  assert.equal(response.headers.get("set-cookie"), null);
  await get("/get/v0.1.1748/o7_0.1.1748_x64-setup.exe");
  // Robots and HEAD are sent on, not counted.
  assert.equal((await get("/get/v0.1.1748/o7_0.1.1748_amd64.deb", { headers: { "User-Agent": "Googlebot/2.1" } })).status, 302);
  assert.equal((await get("/get/v0.1.1748/o7_0.1.1748_amd64.deb", { method: "HEAD" })).status, 302);
  // Anything that is not one of the installers: nothing counted, nowhere to go.
  for (const path of [
    "/get/v0.1.1748/latest.json",
    "/get/v0.1.1747/o7_0.1.1748_x64-setup.exe",
    "/get/v0.1.1748/o7_0.1.1748_x64-setup.exe%2F..",
  ]) {
    const refused = await get(path);
    assert.ok(refused.status === 404 || refused.status === 400, path);
    assert.equal(refused.headers.get("location"), null, path);
  }
  const after = await totals();
  assert.deepEqual(
    after.filter((row) => !before.includes(row)),
    ["download windows-x64 0.1.1748 2"],
  );
});

test("a copy counts itself with no identifier and is told when the next day starts", async () => {
  const ran = (query) => request(`/api/count?${query}`, { headers: { "User-Agent": "o7/0.1.1748" } });
  const first = await ran("os=linux&arch=x86_64&version=0.1.1748&first=1");
  assert.equal(first.status, 200);
  assert.equal(first.headers.get("cache-control"), "no-store");
  const wait = Number((await first.text()).trim());
  assert.ok(Number.isInteger(wait) && wait > 0 && wait <= 25 * 3600, String(wait));
  assert.equal((await ran("os=linux&arch=x86_64&version=0.1.1748&first=0")).status, 200);
  for (const query of ["os=linux&arch=x86_64", "os=linux&arch=x86_64&version=0.1.1748&id=abc"]) {
    assert.equal((await ran(query)).status, 400, query);
  }
  assert.equal(
    (await request("/api/count?os=linux&arch=x86_64&version=0.1.1748", { method: "POST" })).status,
    405,
  );
  const rows = (await totals()).filter((row) => row.includes("linux-x64"));
  assert.deepEqual(rows, ["installed linux-x64 0.1.1748 1", "ran linux-x64 0.1.1748 2"]);
  // The rest of /api stays nobody's page.
  assert.equal((await request("/api/counts")).status, 404);
});
