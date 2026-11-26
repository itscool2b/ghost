/**
 * The iPhone app's client end to end, on this machine: the relay in the Workers runtime, the
 * desktop's real connection task in front of a scripted agent (`crates/ghost-remote/examples/
 * demo_host.rs`), and the app's Swift client (`GhostClient`) speaking to them through the relay's
 * app path, as a phone does.
 *
 *   pnpm --filter @ghost/relay worker:build
 *   cargo build -p ghost-remote --example demo_host
 *   node apps/ios/scripts/e2e.mjs [--host <demo_host executable>]
 *
 * Nothing leaves the machine: every address is localhost. A small control server lets the Swift
 * test ask the stand-in desktop for fresh pairing links (`POST /pair`) and read what the relay
 * sent to Apple (`GET /pushes`): the relay holds a test push key made here, and Apple is a stand-in
 * that keeps each notification and answers 200, so the test opens a sealed notification as the
 * phone would and answers it.
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { webcrypto } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = resolve(here, "../../..");
const { values } = parseArgs({ options: { host: { type: "string" } } });
const hostBinary = values.host ?? resolve(root, "target/debug/examples/demo_host");
// Miniflare is the relay's test dependency.
const { Miniflare, convertV4MiniflareOptions } = createRequire(
  resolve(root, "apps/relay/package.json"),
)("miniflare");

const SITE = "http://localhost:1437";
const RELAY_PORT = 8799;
const CONTROL_PORT = 8797;

// A push key for the relay to sign with, as Apple issues one (PKCS#8, P-256).
const signing = await webcrypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
  "sign",
  "verify",
]);
const pkcs8 = Buffer.from(await webcrypto.subtle.exportKey("pkcs8", signing.privateKey));
const APNS_KEY = `-----BEGIN PRIVATE KEY-----\n${pkcs8.toString("base64").replace(/.{64}/g, "$&\n")}\n-----END PRIVATE KEY-----\n`;
/** What the relay sent to Apple, oldest first. */
const pushes = [];

const relay = new Miniflare(
  convertV4MiniflareOptions({
    modules: [
      {
        type: "ESModule",
        path: "index.js",
        contents: await readFile(resolve(root, "apps/relay/build/index.js"), "utf8"),
      },
    ],
    compatibilityDate: "2026-10-26",
    durableObjects: {
      ROOMS: { className: "Room", useSQLite: true },
      APNS: { className: "Apns", useSQLite: true },
    },
    bindings: {
      APP_ORIGIN: SITE,
      APNS_TOPIC: "dev.onghost.mobile",
      APNS_KEY_ID: "E2ETESTKEY",
      APNS_TEAM_ID: "E2ETEAM123",
      APNS_KEY,
    },
    // Apple: every notification is kept for the test and accepted.
    outboundService: async (request) => {
      const url = new URL(request.url);
      if (!url.hostname.endsWith("push.apple.com"))
        return new Response("not here", { status: 502 });
      pushes.push({
        url: request.url,
        headers: Object.fromEntries(request.headers),
        body: await request.text(),
      });
      return new Response("", { status: 200, headers: { "apns-id": webcrypto.randomUUID() } });
    },
    host: "127.0.0.1",
    port: RELAY_PORT,
  }),
);
await relay.ready;

const folder = await mkdtemp(join(tmpdir(), "ghost-ios-e2e-"));
const host = spawn(hostBinary, [`ws://127.0.0.1:${RELAY_PORT}`, SITE, folder, "Studio"], {
  stdio: ["pipe", "pipe", "inherit"],
});
const lines = [];
const waiting = [];
let rest = "";
host.stdout.on("data", (chunk) => {
  rest += chunk;
  for (;;) {
    const end = rest.indexOf("\n");
    if (end < 0) break;
    const line = rest.slice(0, end).trim();
    rest = rest.slice(end + 1);
    const next = waiting.shift();
    if (next) next(line);
    else lines.push(line);
  }
});
const line = (prefix) =>
  new Promise((done, fail) => {
    const timer = setTimeout(() => fail(new Error(`the host printed no ${prefix}`)), 15_000);
    const take = (text) => {
      if (!text.startsWith(`${prefix} `)) return waiting.unshift(take);
      clearTimeout(timer);
      done(text.slice(prefix.length + 1));
    };
    const ready = lines.findIndex((text) => text.startsWith(`${prefix} `));
    if (ready >= 0) take(lines.splice(ready, 1)[0]);
    else waiting.push(take);
  });

const first = await line("PAIR");
assert.match(first, /^http:\/\/localhost:1437\/remote#[\w-]{22}\.[\w-]{11}\.[\w-]{43}$/);

const control = createServer(async (request, response) => {
  if (request.method === "POST" && request.url === "/pair") {
    host.stdin.write("pair\n");
    const link = await line("PAIR");
    response.writeHead(200, { "Content-Type": "text/plain" }).end(link);
    return;
  }
  if (request.method === "GET" && request.url === "/pushes") {
    response.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify(pushes));
    return;
  }
  response.writeHead(404).end();
});
await new Promise((done) => control.listen(CONTROL_PORT, "127.0.0.1", done));

let failed = false;
try {
  const swift = spawn(
    "swift",
    [
      "test",
      "--package-path",
      resolve(root, "apps/ios/GhostKit"),
      "--scratch-path",
      join(tmpdir(), "ghostkit-e2e"),
      "--filter",
      "EndToEnd",
    ],
    {
      stdio: "inherit",
      env: {
        ...process.env,
        DEVELOPER_DIR: process.env.DEVELOPER_DIR ?? "/Applications/Xcode.app/Contents/Developer",
        GHOST_E2E_RELAY: `ws://127.0.0.1:${RELAY_PORT}`,
        GHOST_E2E_CONTROL: `http://127.0.0.1:${CONTROL_PORT}`,
        GHOST_E2E_FIRST: first,
      },
    },
  );
  const code = await new Promise((done) => swift.on("exit", done));
  failed = code !== 0;
} finally {
  control.close();
  host.stdin.end();
  host.kill();
  await relay.dispose();
  await rm(folder, { recursive: true, force: true });
}
if (failed) {
  console.error("ios e2e: the Swift client failed against the relay and the stand-in desktop");
  process.exit(1);
}
console.log("ios e2e: the app's client paired through the app path, followed a turn, answered");
