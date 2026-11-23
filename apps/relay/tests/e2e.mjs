/**
 * Remote sessions end to end, on this machine: the relay in the Workers runtime, the desktop's
 * real connection task in front of a scripted agent (`crates/o7-remote/examples/demo_host.rs`),
 * the built website's remote page in real browsers, and a bot speaking MCP over HTTP.
 *
 *   corepack pnpm --filter @o7/web build
 *   cargo build -p o7-remote --example demo_host
 *   corepack pnpm --filter @o7/relay worker:build
 *   node apps/relay/tests/e2e.mjs [--host <demo_host executable>] [--engines chromium,webkit]
 *     [--shots <folder for screenshots>] [--scheme light|dark]
 *
 * Nothing leaves the machine: every address is localhost.
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { convertV4MiniflareOptions, Miniflare } from "miniflare";

const here = fileURLToPath(new URL(".", import.meta.url));
const { values } = parseArgs({
  options: {
    host: { type: "string" },
    engines: { type: "string", default: "chromium" },
    shots: { type: "string" },
    // The system appearance the phone reports: the demo computer follows the system.
    scheme: { type: "string", default: "light" },
  },
});
const hostBinary =
  values.host ??
  resolve(
    here,
    "../../../target/debug/examples",
    process.platform === "win32" ? "demo_host.exe" : "demo_host",
  );
// Playwright is the website's and the desktop's test dependency, not the relay's.
const { chromium, webkit, devices } = createRequire(resolve(here, "../../web/package.json"))(
  "playwright",
);
const SITE_PORT = 1437;
const RELAY_PORT = 8799;
const SITE = `http://localhost:${SITE_PORT}`;

/** The built website, as its Worker serves it: files, and each route's own page. */
function serveSite() {
  const root = resolve(here, "../../web/build/client");
  const types = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".woff2": "font/woff2",
    ".json": "application/json",
    ".webp": "image/webp",
  };
  const server = createServer(async (request, response) => {
    const path = normalize(decodeURIComponent(new URL(request.url, SITE).pathname));
    for (const candidate of [path, join(path, "index.html")]) {
      const file = join(root, candidate);
      if (!file.startsWith(root)) break;
      try {
        if ((await stat(file)).isFile()) {
          response.writeHead(200, {
            "Content-Type": types[extname(file)] ?? "application/octet-stream",
          });
          response.end(await readFile(file));
          return;
        }
      } catch {
        // Try the next candidate.
      }
    }
    response.writeHead(404).end("Not found");
  });
  return new Promise((done) => server.listen(SITE_PORT, "127.0.0.1", () => done(server)));
}

/** The stand-in desktop: its pairing links and bot address as it prints them. */
function startHost(folder) {
  const child = spawn(hostBinary, [`ws://127.0.0.1:${RELAY_PORT}`, SITE, folder], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  const lines = [];
  const waiting = [];
  let rest = "";
  child.stdout.on("data", (chunk) => {
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
      const timer = setTimeout(() => fail(new Error(`the host printed no ${prefix}`)), 10_000);
      const take = (text) => {
        clearTimeout(timer);
        assert.ok(text.startsWith(`${prefix} `), text);
        done(text.slice(prefix.length + 1));
      };
      const ready = lines.shift();
      if (ready) take(ready);
      else waiting.push(take);
    });
  return {
    child,
    line,
    /** Asks for a fresh pairing link. */
    async pair() {
      child.stdin.write("pair\n");
      return line("PAIR");
    },
  };
}

const page = (link) => `${link.replace("/remote#", `/remote?relay=ws://127.0.0.1:${RELAY_PORT}#`)}`;

async function browserFlow(engine, launcher, host, first, shots) {
  const browser = await launcher.launch();
  const errors = [];
  try {
    const phone = await browser.newContext({
      ...(engine === "webkit" ? devices["iPhone 14"] : devices["Pixel 7"]),
      colorScheme: values.scheme === "dark" ? "dark" : "light",
    });
    const tab = await phone.newPage();
    // Every socket to the relay passes through here, so one can be made to go quiet: open as far
    // as the page can tell, with nothing crossing it, as after a phone slept.
    const sockets = [];
    let quiet = null;
    await tab.routeWebSocket(new RegExp(`:${RELAY_PORT}/`), (socket) => {
      const relay = socket.connectToServer();
      sockets.push(socket);
      socket.onMessage((message) => {
        if (quiet !== socket) relay.send(message);
      });
      relay.onMessage((message) => {
        if (quiet !== socket) socket.send(message);
      });
    });
    tab.on("pageerror", (error) => errors.push(error.message));
    tab.on("console", (entry) => {
      if (entry.type() === "error") errors.push(entry.text());
    });

    // Pairing: the link opens the computer's chats, and its key leaves the address bar at once.
    await tab.goto(page(first));
    // Opening the link pairs nothing: the page asks first, and the computer has heard nothing.
    await tab.getByRole("heading", { name: "Pair this browser?" }).waitFor();
    await tab.getByRole("button", { name: "Pair this browser", exact: true }).click();
    await tab.getByRole("heading", { level: 1, name: "Demo computer" }).waitFor();
    assert.equal(new URL(tab.url()).hash, "", "the pairing key stayed in the address");
    await tab.getByText("Connected", { exact: true }).waitFor();
    const row = tab.getByRole("button", { name: /Fix the login form/ });
    await row.waitFor();
    assert.match(await row.textContent(), /Demo agent · Idle/);
    if (shots) await tab.screenshot({ path: join(shots, `${engine}-list.png`) });

    // The same link is spent: a second browser gets told so, and nothing else.
    const other = await browser.newContext();
    const stranger = await other.newPage();
    await stranger.goto(page(first));
    await stranger.getByRole("button", { name: "Pair this browser", exact: true }).click();
    await stranger.getByRole("heading", { name: "This pairing is over." }).waitFor();
    await stranger.getByText(/already used or has expired/).waitFor();
    await other.close();

    // A chat's history, then a turn live: the message, the agent's words, its step, its request.
    await row.click();
    await tab.getByText("Why does the login form submit empty emails?").waitFor();
    await tab.getByText("The submit handler never checks the field. I can add a check.").waitFor();
    const box = tab.getByRole("textbox", { name: "Message" });
    await box.fill("Add the check and run the tests");
    await tab.getByRole("button", { name: "Send" }).click();
    await tab.locator(".remote-user", { hasText: "Add the check and run the tests" }).waitFor();
    // The box empties once the computer has taken the message.
    await tab.waitForFunction(
      () => document.querySelector('textarea[aria-label="Message"]')?.value === "",
    );
    await tab.locator(".remote-markdown strong", { hasText: "login form" }).waitFor();
    await tab.locator(".remote-steps summary", { hasText: "Read src/login.ts" }).waitFor();
    const card = tab.getByRole("region", { name: "The agent asks for permission" });
    await card.getByText("Run `npm test`").waitFor();
    await tab.getByText("Demo agent · Needs you").waitFor();
    if (shots) await tab.screenshot({ path: join(shots, `${engine}-permission.png`) });
    await card.getByRole("button", { name: "Allow once" }).click();
    await tab.getByText("Run `npm test`: Allow once").waitFor();
    await tab.getByText("Done. The form now checks the email before it submits:").waitFor();
    await tab.locator(".remote-markdown pre code", { hasText: 'email.includes("@")' }).waitFor();
    await tab.locator(".remote-markdown li code", { hasText: "a@b" }).waitFor();
    await tab.getByText("Demo agent · Idle").waitFor();
    assert.equal(await card.count(), 0, "an answered request still asked");
    if (shots) await tab.screenshot({ path: join(shots, `${engine}-chat.png`) });

    // A phone's keyboard covers the page's foot without resizing the page. The page follows what
    // is still visible: the message box sits on the keyboard, the header stays, and the newest
    // words stay in view. The keyboard is played by the visible area reporting a smaller height.
    const keyboard = (height, top) =>
      tab.evaluate(
        ([visible, offset]) => {
          const area = window.visualViewport;
          for (const [name, value] of [
            ["height", visible],
            ["offsetTop", offset],
          ])
            Object.defineProperty(area, name, { configurable: true, get: () => value });
          area.dispatchEvent(new Event("resize"));
          const edges = (selector) => document.querySelector(selector).getBoundingClientRect();
          const words = document.querySelector(".remote-transcript");
          return {
            page: edges(".remote"),
            box: edges(".remote-composer"),
            head: edges(".remote-bar"),
            fromEnd: words.scrollHeight - words.scrollTop - words.clientHeight,
            flagged: document.documentElement.hasAttribute("data-remote-keyboard"),
          };
        },
        [height, top],
      );
    const whole = await tab.evaluate(() => window.innerHeight);
    const up = await keyboard(whole - 320, 40);
    assert.equal(up.flagged, true, "the keyboard was not noticed");
    assert.ok(Math.abs(up.page.top - 40) < 1 && Math.abs(up.page.height - (whole - 320)) < 1);
    assert.ok(
      Math.abs(up.box.bottom - (40 + whole - 320)) < 1,
      "the message box left the keyboard",
    );
    assert.ok(up.head.top >= 40 - 1, "the header went under the browser's bar");
    await tab.waitForFunction(() => {
      const words = document.querySelector(".remote-transcript");
      return words.scrollHeight - words.scrollTop - words.clientHeight < 2;
    });
    const down = await keyboard(whole, 0);
    assert.equal(down.flagged, false);
    assert.ok(Math.abs(down.page.height - whole) < 1 && Math.abs(down.box.bottom - whole) < 1);

    // Stopping a turn that waits declines its request and says the turn stopped.
    await box.fill("Now the signup form");
    await box.press("Control+Enter");
    await card.getByText("Run `npm test`").waitFor();
    await tab.getByRole("button", { name: "Stop" }).click();
    await tab.getByText("Stopped", { exact: true }).waitFor();
    await tab.getByText("Run `npm test`: declined").waitFor();
    assert.equal(await tab.getByRole("button", { name: "Stop" }).count(), 0);

    // Reloading keeps the pairing (the key is stored, unreadable) and the history.
    await tab.reload();
    await tab.getByRole("heading", { level: 1, name: "Demo computer" }).waitFor();
    await tab.getByRole("button", { name: /Fix the login form/ }).click();
    await tab.locator(".remote-user", { hasText: "Now the signup form" }).waitFor();
    const stored = await tab.evaluate(
      () =>
        new Promise((done, fail) => {
          const open = indexedDB.open("auracode-remote");
          open.onerror = () => fail(open.error);
          open.onsuccess = () => {
            const all = open.result.transaction("hosts").objectStore("hosts").getAll();
            all.onsuccess = () =>
              done(
                all.result.map((host) => ({
                  name: host.hostName,
                  key: Object.prototype.toString.call(host.key),
                  extractable: host.key.extractable,
                })),
              );
          };
        }),
    );
    assert.deepEqual(stored, [
      { name: "Demo computer", key: "[object CryptoKey]", extractable: false },
    ]);
    // Nothing of the pairing is kept where a script could read it back: the site's router
    // remembers scroll positions there, and that is all.
    assert.deepEqual(
      await tab.evaluate(() => Object.keys({ ...localStorage, ...sessionStorage })),
      ["react-router-scroll-positions"],
    );

    // The phone slept and its socket died without closing. Coming back to the page finds that
    // out within moments and connects again: chats open without a reload.
    await tab.getByRole("button", { name: "All chats" }).click();
    await tab.getByText("Connected", { exact: true }).waitFor();
    const opened = sockets.length;
    quiet = sockets.at(-1);
    await tab.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
    const deadline = Date.now() + 10_000;
    while (sockets.length === opened) {
      assert.ok(Date.now() < deadline, "a quiet socket was kept after the page came back");
      await new Promise((done) => setTimeout(done, 100));
    }
    await tab.getByText("Connected", { exact: true }).waitFor();
    await tab.getByRole("button", { name: /Fix the login form/ }).click();
    await tab.locator(".remote-user", { hasText: "Now the signup form" }).waitFor();
    if (shots) await tab.screenshot({ path: join(shots, `${engine}-back.png`) });

    // A new chat from here opens on its first turn.
    await tab.getByRole("button", { name: "All chats" }).click();
    await tab.getByRole("button", { name: "New chat", exact: true }).click();
    const sheet = tab.getByRole("dialog", { name: "New chat" });
    assert.equal(await sheet.getByLabel("Agent").inputValue(), "");
    await sheet.getByLabel("Message").fill("Write a README");
    await sheet.getByRole("button", { name: "Start chat" }).click();
    await tab.getByRole("heading", { level: 1, name: "New chat" }).waitFor();
    await tab.locator(".remote-user", { hasText: "Write a README" }).waitFor();
    await tab.getByRole("region", { name: "The agent asks for permission" }).waitFor();
    await tab.getByRole("button", { name: "All chats" }).click();
    await tab.getByRole("button", { name: /New chat.*Needs you/s }).waitFor();

    // A second device pairs with its own link and sees the first one's turn arrive live.
    const second = await browser.newContext();
    const laptop = await second.newPage();
    laptop.on("pageerror", (error) => errors.push(error.message));
    await laptop.goto(page(await host.pair()));
    await laptop.getByRole("button", { name: "Pair this browser", exact: true }).click();
    await laptop.getByRole("button", { name: /New chat.*Needs you/s }).click();
    await laptop.getByRole("region", { name: "The agent asks for permission" }).waitFor();
    await tab.getByRole("button", { name: /New chat.*Needs you/s }).click();
    await tab.getByRole("button", { name: "Reject" }).click();
    await laptop.getByText("Run `npm test`: Reject").waitFor();
    await laptop.getByText("Done. The form now checks the email before it submits:").waitFor();

    // Forgetting unpairs: this browser has nothing left, and its key opens nothing.
    await laptop.getByRole("button", { name: "All chats" }).click();
    await laptop.getByRole("button", { name: "This computer" }).click();
    await laptop.getByRole("button", { name: "Forget this computer" }).click();
    await laptop.getByRole("heading", { name: "Reach your computer from here." }).waitFor();
    await laptop.reload();
    await laptop.getByRole("heading", { name: "Reach your computer from here." }).waitFor();
    await second.close();

    // The pickers, in the chat the first device has open: a model and a thinking level apply as
    // they are chosen, and another agent is a handoff into a chat that starts from this one.
    await tab.getByText("Demo agent · Idle").waitFor();
    await tab.getByRole("button", { name: "Model and agent" }).click();
    const pickers = tab.getByRole("dialog", { name: "Model and agent" });
    const model = pickers.getByRole("combobox", { name: "Model" });
    await model.waitFor();
    assert.equal(await model.inputValue(), "fast");
    await model.selectOption("smart");
    await pickers.getByRole("status").getByText("Model set to Smart.").waitFor();
    assert.equal(await model.inputValue(), "smart", "the computer did not keep the model");
    await pickers.getByRole("combobox", { name: "Thinking" }).selectOption("high");
    await pickers.getByRole("status").getByText("Thinking set to High.").waitFor();
    // Only agents that can run here are offered, and nothing switches until asked.
    const agent = pickers.getByRole("combobox", { name: "Agent" });
    assert.deepEqual(await agent.locator("option").allTextContents(), [
      "Demo agent (now)",
      "Other agent",
    ]);
    assert.equal(await pickers.getByRole("button", { name: /^Switch to/ }).count(), 0);
    await agent.selectOption("other");
    await pickers
      .getByText("carries on with Other agent in a new chat", { exact: false })
      .waitFor();
    if (shots) await tab.screenshot({ path: join(shots, `${engine}-pickers.png`) });
    await pickers.getByRole("button", { name: "Switch to Other agent" }).click();
    await pickers.waitFor({ state: "detached" });
    await tab.locator(".remote-link", { hasText: /^Other agent/ }).waitFor();
    await tab.locator(".remote-user", { hasText: "Write a README" }).waitFor();
    // The choice was the first chat's: the new one starts from the agent's own defaults.
    await tab.getByRole("button", { name: "Model and agent" }).click();
    assert.equal(await model.inputValue(), "fast");
    assert.equal(await agent.inputValue(), "other");
    await pickers.getByRole("button", { name: "Done" }).click();
    await pickers.waitFor({ state: "detached" });

    // The first device is untouched by the second one leaving.
    await tab.getByRole("button", { name: "All chats" }).click();
    await tab.getByText("Connected", { exact: true }).waitFor();

    // A link that is not whole is refused before anything connects.
    const broken = await browser.newContext();
    const wrong = await broken.newPage();
    await wrong.goto(`${SITE}/remote?relay=ws://127.0.0.1:${RELAY_PORT}#not.a.link`);
    await wrong.getByRole("button", { name: "Pair this browser", exact: true }).click();
    await wrong.getByText("This pairing link is not complete.").waitFor();
    await broken.close();
    assert.deepEqual(errors, []);
    await phone.close();
  } finally {
    await browser.close();
  }
}

/** A bot, speaking MCP over HTTP to the address the desktop printed. */
async function botFlow(address) {
  const url = address.replace(SITE, `http://127.0.0.1:${RELAY_PORT}`);
  let id = 0;
  const rpc = async (method, params) => {
    id += 1;
    const response = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/json, text/event-stream",
      },
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
    });
    assert.equal(response.status, 200, `${method}: ${response.status}`);
    const body = await response.json();
    assert.equal(body.id, id);
    return body.result;
  };
  /** One call whose answer may be an error of the protocol's own. */
  const rpcRaw = async (method, params) => {
    id += 1;
    const response = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id, method, params }),
    });
    return response.json();
  };
  const tool = async (name, args = {}) => {
    const result = await rpc("tools/call", { name, arguments: args });
    return { text: result.content[0].text, error: result.isError };
  };
  // The link exists at the relay a moment after the desktop made it: wait for that moment.
  for (let tries = 0; tries < 50; tries += 1) {
    const probe = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 0, method: "ping" }),
    });
    if (probe.status === 200) break;
    await new Promise((done) => setTimeout(done, 100));
  }
  const hello = await rpc("initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "e2e", version: "1" },
  });
  assert.equal(hello.serverInfo.name, "o7");
  // The demo's link has the chat tools: the wider ones are not shown to it, and neither is
  // what only a full-control link may choose when it sends.
  const listed = (await rpc("tools/list")).tools;
  assert.deepEqual(
    listed.map((entry) => entry.name),
    ["inbox", "chats", "projects", "read", "task", "send", "wait", "answer", "stop"],
  );
  assert.deepEqual(
    Object.keys(listed.find((entry) => entry.name === "send").inputSchema.properties),
    ["chat", "folder", "agent", "text", "wait"],
  );

  const chats = await tool("chats");
  assert.equal(chats.error, false);
  assert.match(chats.text, /^\d+ chats? on Demo computer, newest first/);
  assert.match(chats.text, /\nc1 \| idle \| Fix the login form \| demo \| Demo agent\n/);
  const projects = await tool("projects");
  assert.match(projects.text, /\n {2}folder f1 demo\n/);

  // One call starts a chat and comes back when the agent asks; one more allows it and comes
  // back with what the turn came to. Two calls where the first tools took five.
  const asking = await tool("send", { folder: "f1", text: "Tidy the tests", wait: 20 });
  assert.equal(asking.error, false, asking.text);
  const chat = /^Started chat (\S+) with Demo agent\./.exec(asking.text)?.[1];
  assert.ok(chat, asking.text);
  assert.match(asking.text, /: working, waiting for an answer\./);
  const request = /Waiting request (\S+) \(permission\): Run `npm test`/.exec(asking.text)?.[1];
  assert.ok(request, asking.text);
  assert.match(asking.text, /Options for answer: allow = Allow once; reject = Reject/);
  // While it asks, one call says so for every chat, with the request to answer.
  const asks = await tool("inbox");
  assert.equal(asks.error, false, asks.text);
  assert.match(asks.text, /^Asking \(1\):\n/);
  assert.ok(asks.text.includes(`${chat} "`) && asks.text.includes(request), asks.text);
  assert.match(asks.text, /Options for answer: allow = Allow once; reject = Reject/);
  const finished = await tool("answer", { chat, request, option: "allow", wait: 20 });
  assert.equal(finished.error, false, finished.text);
  assert.match(finished.text, /^Answered\. Chat /);
  assert.match(finished.text, /: idle\./);
  assert.match(finished.text, /User: Tidy the tests\n/);
  assert.match(finished.text, /Steps: Read src\/login\.ts\n/);
  assert.match(finished.text, /Agent: [\s\S]*Done\. The form now checks the email/);
  assert.match(finished.text, /The turn finished\.\n/);
  assert.doesNotMatch(finished.text, /Waiting request/);
  // The result carries its proof: what ran and how each ended, and what still fails.
  assert.match(finished.text, /Ran:\n- exit 0 \| npm test\n- exit 1 \| npm run lint\n/);
  assert.match(
    finished.text,
    /Still failing: exit 1 \| npm run lint\nFirst error:\n {2}3:7 {2}error {2}'old' is assigned/,
  );
  // Afterwards the inbox has nothing asking and names the finished chat with what it came to.
  const after = await tool("inbox");
  assert.doesNotMatch(after.text, /Asking/);
  assert.match(after.text, /Finished in the last day \(\d+\):\n/);
  assert.match(
    after.text,
    new RegExp(
      `${chat} "[^"]*" \\(demo, Demo agent\\), just now: finished, still failing: exit 1 \\| npm run lint\\. Done\\.`,
    ),
  );
  // Reading it again is the same turn, and a wait on a chat at rest returns at once.
  assert.match((await tool("read", { chat })).text, /The turn finished\.\n/);
  assert.match((await tool("wait", { chat, seconds: 5 })).text, /: idle\./);

  // What a bot gets wrong comes back as words it can act on.
  const twice = await tool("answer", { chat, request, option: "allow" });
  assert.equal(twice.error, true);
  assert.match(twice.text, /not waiting any more/);
  assert.equal((await tool("read", { chat: "nope" })).error, true);
  // A chats link cannot choose how the agent runs, and a tool of the first set is not there.
  const chosen = await tool("send", { chat, text: "Again", access: "full_auto" });
  assert.equal(chosen.error, true);
  assert.match(chosen.text, /chat tools only/);
  assert.equal(
    (await rpcRaw("tools/call", { name: "list_chats", arguments: {} })).error.code,
    -32602,
  );

  // A link with another token does not exist.
  const forged = url.replace(/\.[A-Za-z0-9_-]{43}$/, `.${"A".repeat(43)}`);
  const refused = await fetch(forged, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
  });
  assert.equal(refused.status, 404);
}

const site = await serveSite();
const relay = new Miniflare(
  convertV4MiniflareOptions({
    modules: [
      {
        type: "ESModule",
        path: "index.js",
        contents: await readFile(resolve(here, "../build/index.js"), "utf8"),
      },
    ],
    compatibilityDate: "2026-10-26",
    durableObjects: { ROOMS: { className: "Room", useSQLite: true } },
    bindings: { APP_ORIGIN: SITE },
    host: "127.0.0.1",
    port: RELAY_PORT,
  }),
);
await relay.ready;

/** A desktop of its own for each run, so one run's chats are not in the next one's way. */
async function withHost(work) {
  const folder = await mkdtemp(join(tmpdir(), "o7-remote-e2e-"));
  const host = startHost(folder);
  try {
    const first = await host.line("PAIR");
    const bot = await host.line("BOT");
    assert.match(first, /^http:\/\/localhost:1437\/remote#[\w-]{22}\.[\w-]{11}\.[\w-]{43}$/);
    await work(host, first, bot);
  } finally {
    host.child.stdin.end();
    host.child.kill();
    await rm(folder, { recursive: true, force: true });
  }
}

try {
  await withHost(async (_host, _first, bot) => {
    await botFlow(bot);
    console.log("bot: MCP tools drove a chat through the relay");
  });
  for (const engine of values.engines.split(",")) {
    await withHost(async (host, first) => {
      await browserFlow(engine, engine === "webkit" ? webkit : chromium, host, first, values.shots);
      console.log(`${engine}: paired, followed a turn live, answered, stopped, started and forgot`);
    });
  }
} finally {
  await relay.dispose();
  site.close();
}
