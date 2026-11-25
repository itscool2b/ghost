// The relay in the Workers runtime: real Durable Object sockets, no network.
import assert from "node:assert/strict";
import { createHash, randomBytes } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { convertV4MiniflareOptions, Miniflare } from "miniflare";

const ORIGIN = "https://auracode.test";
/** The address the site had before: its remote page is still let in. */
const FORMER = "https://former.test";
const BASE = `${ORIGIN}/api/remote/v1`;
const id = (bytes) => randomBytes(bytes).toString("base64url");
/** A computer's id is the start of a hash of its key. */
const idOf = (key) =>
  createHash("sha256")
    .update("auracode remote v1 host id")
    .update(Buffer.from(key, "base64url"))
    .digest()
    .subarray(0, 16)
    .toString("base64url");
const hash = (text) => createHash("sha256").update(text).digest("base64url");
/** Where Apple delivers o7's notifications, and the push key's id and its team's. */
const TOPIC = "codes.o7.mobile";
const KEY_ID = "ABC123DEFG";
const TEAM_ID = "DEF123GHIJ";
let runtime;
/** The relay with Apple's push key, made here as Apple gives one, and Apple played by `apple`. */
let pushing;
/** The push key: its PEM, as the relay is given it, and its public half, to check tokens with. */
let signing;
/** When the tests began, in seconds: no token the relay shows Apple is older. */
let started;

/**
 * Apple, as the relay's requests reach it: each one kept, and each answered with the next answer
 * a test gave (200 when there is none). As a Node handler it can also drop the connection
 * ("lost"), as a network that fails does.
 */
function appleMock() {
  const requests = [];
  const answers = [];
  const next = () => answers.shift() ?? { status: 200 };
  const body = ({ reason, text }) =>
    reason === undefined ? (text ?? "") : JSON.stringify({ reason });
  return {
    requests,
    answer: (...given) => answers.push(...given),
    async fetch(request) {
      requests.push({
        method: request.method,
        url: request.url,
        headers: Object.fromEntries(request.headers),
        body: await request.text(),
      });
      const answer = next();
      return new Response(body(answer) || null, { status: answer.status });
    },
    node(request, response) {
      let text = "";
      request.setEncoding("utf8");
      request.on("data", (chunk) => {
        text += chunk;
      });
      request.on("end", () => {
        requests.push({
          method: request.method,
          url: `https://${request.headers.host}${request.url}`,
          headers: { ...request.headers },
          body: text,
        });
        const answer = next();
        if (answer === "lost") return request.socket.destroy();
        response.writeHead(answer.status).end(body(answer));
      });
    },
  };
}
const apple = appleMock();
/** Apple for the relay without a key: nothing may reach it. */
const offline = appleMock();

/** The relay's built Worker, as the runtime loads it. */
async function built() {
  return [
    {
      type: "ESModule",
      path: "index.js",
      contents: await readFile(
        fileURLToPath(new URL("../build/index.js", import.meta.url)),
        "utf8",
      ),
    },
  ];
}

/** A relay that has Apple's push key, with Apple played by `outboundService`. */
async function withKey(outboundService, bindings = {}, options = {}) {
  const relay = new Miniflare(
    convertV4MiniflareOptions({
      ...options,
      modules: await built(),
      compatibilityDate: "2026-10-26",
      durableObjects: {
        ROOMS: { className: "Room", useSQLite: true },
        APNS: { className: "Apns", useSQLite: true },
      },
      bindings: {
        APP_ORIGIN: ORIGIN,
        APNS_TOPIC: TOPIC,
        APNS_KEY_ID: KEY_ID,
        APNS_TEAM_ID: TEAM_ID,
        APNS_KEY: signing.pem,
        ...bindings,
      },
      outboundService,
    }),
  );
  await relay.ready;
  return relay;
}

before(async () => {
  started = Math.floor(Date.now() / 1000);
  runtime = new Miniflare(
    convertV4MiniflareOptions({
      modules: await built(),
      compatibilityDate: "2026-10-26",
      durableObjects: {
        ROOMS: { className: "Room", useSQLite: true },
        APNS: { className: "Apns", useSQLite: true },
      },
      // As deployed before the owner sets Apple's push key: push is off.
      bindings: {
        APP_ORIGIN: ORIGIN,
        FORMER_ORIGIN: FORMER,
        APNS_TOPIC: TOPIC,
        APNS_KEY_ID: "",
        APNS_TEAM_ID: "",
      },
      outboundService: offline.fetch,
    }),
  );
  await runtime.ready;
  // An ECDSA P-256 key as Apple gives one, a PKCS#8 PEM (a .p8 file).
  const pair = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, [
    "sign",
    "verify",
  ]);
  const der = Buffer.from(await crypto.subtle.exportKey("pkcs8", pair.privateKey)).toString(
    "base64",
  );
  signing = {
    publicKey: pair.publicKey,
    pem: `-----BEGIN PRIVATE KEY-----\n${der.match(/.{1,64}/g).join("\n")}\n-----END PRIVATE KEY-----\n`,
  };
  pushing = await withKey(apple.fetch);
});

after(async () => {
  await runtime?.dispose();
  await pushing?.dispose();
});

/** A socket with its messages queued, so a test reads them in order. */
function reader(socket) {
  const queue = [];
  const waiting = [];
  let closed = null;
  const push = (item) => {
    const next = waiting.shift();
    if (next) next(item);
    else queue.push(item);
  };
  socket.addEventListener("message", (event) => push({ data: event.data }));
  socket.addEventListener("close", (event) => {
    closed = { code: event.code };
    push({ closed });
    // A browser completes the closing handshake by itself; this socket has to be told to.
    try {
      socket.close(event.code, "closing");
    } catch {
      // Already closed from this side.
    }
  });
  socket.accept();
  const next = () => {
    // Made here, so a timeout names the line that was waiting.
    const late = new Error("no message within 3s");
    return new Promise((resolve, reject) => {
      const item = queue.shift();
      if (item) return resolve(item);
      const timer = setTimeout(() => reject(late), 3000);
      waiting.push((value) => {
        clearTimeout(timer);
        resolve(value);
      });
    });
  };
  return {
    socket,
    send: (text) => socket.send(text),
    close: (code = 1000) => socket.close(code, "done"),
    async message() {
      const item = await next();
      assert.ok("data" in item, `socket closed (${item.closed?.code}) instead of a message`);
      return item.data;
    },
    async closing() {
      for (;;) {
        const item = await next();
        if (item.closed) return item.closed.code;
      }
    },
    async quiet(ms = 150) {
      await new Promise((resolve) => setTimeout(resolve, ms));
      assert.deepEqual(queue, [], "unexpected messages");
    },
  };
}

async function upgrade(path, headers = {}, on = runtime) {
  return on.dispatchFetch(`${BASE}/${path}`, {
    headers: { Upgrade: "websocket", ...headers },
  });
}

async function host(hostId, key, on = runtime) {
  const response = await upgrade(`host/${hostId}`, { Authorization: `Bearer ${key}` }, on);
  assert.equal(response.status, 101);
  return reader(response.webSocket);
}

async function client(hostId, on = runtime) {
  const response = await upgrade(`client/${hostId}`, { Origin: ORIGIN }, on);
  assert.equal(response.status, 101);
  return reader(response.webSocket);
}

/** o7 for iPhone's socket: the app's path, with no Origin. */
async function app(hostId, headers = {}) {
  const response = await upgrade(`app/${hostId}`, headers);
  assert.equal(response.status, 101);
  return reader(response.webSocket);
}

const mcp = (hostId, reference, body, init = {}) =>
  runtime.dispatchFetch(`${BASE}/mcp/${hostId}/${reference}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...init.headers },
    body: typeof body === "string" ? body : JSON.stringify(body),
  });

test("reports health and hides everything else", async () => {
  const health = await runtime.dispatchFetch(`${BASE}/health`);
  assert.equal(health.status, 200);
  assert.deepEqual(await health.json(), {
    ok: true,
    protocol: 1,
    tools: 9,
    full_tools: 14,
    second_tools: 7,
    second_full_tools: 11,
    first_tools: 8,
    first_full_tools: 21,
    push: false,
  });
  assert.equal(health.headers.get("cache-control"), "no-store");
  for (const path of ["", "nothing", "host", "host/short", `other/${id(16)}`])
    assert.equal((await runtime.dispatchFetch(`${BASE}/${path}`)).status, 404, path);
  assert.equal((await runtime.dispatchFetch(`${ORIGIN}/api/v1/config`)).status, 404);
});

test("only the desktop that claimed an id can hold its host socket", async () => {
  const key = id(32);
  const hostId = idOf(key);
  // Nothing is behind an id no desktop has claimed.
  assert.equal((await upgrade(`client/${hostId}`, { Origin: ORIGIN })).status, 404);
  assert.equal((await upgrade(`host/${hostId}`)).status, 401);
  assert.equal((await upgrade(`host/${hostId}`, { Authorization: "Bearer short" })).status, 401);
  // A browser cannot open a host socket, and a plain request is not a socket.
  assert.equal(
    (await upgrade(`host/${hostId}`, { Authorization: `Bearer ${key}`, Origin: ORIGIN })).status,
    400,
  );
  assert.equal((await runtime.dispatchFetch(`${BASE}/host/${hostId}`)).status, 400);
  const first = await host(hostId, key);
  assert.equal(
    (await upgrade(`host/${hostId}`, { Authorization: `Bearer ${id(32)}` })).status,
    401,
    "another key took over a claimed id",
  );
  first.close();
  await first.closing();
  // The same key connects again after the socket closed.
  (await host(hostId, key)).close();
});

test("a computer's id is made from its key, so nobody can take it", async () => {
  // Someone who knows a computer's id, but not its key, gets there first with a key of their own.
  const key = id(32);
  const hostId = idOf(key);
  const theirs = id(32);
  assert.equal(
    (await upgrade(`host/${hostId}`, { Authorization: `Bearer ${theirs}` })).status,
    401,
    "an id was claimed with a key it is not made from",
  );
  // Nothing was claimed, and nothing stored: the id still has nothing behind it.
  assert.equal((await upgrade(`client/${hostId}`, { Origin: ORIGIN })).status, 404);
  // An id of their own choosing is no better, whatever key goes with it.
  assert.equal(
    (await upgrade(`host/${id(16)}`, { Authorization: `Bearer ${theirs}` })).status,
    401,
  );
  // The computer itself connects as always.
  (await host(hostId, key)).close();
});

test("a socket that has shown no key gets the handshake's few small messages", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const desk = await host(hostId, key);
  // One large message: closed, and it never reaches the computer.
  const loud = await client(hostId);
  assert.equal(await loud.message(), "h1");
  const opened = await desk.message();
  assert.equal(opened[0], "o");
  loud.send(`m${"x".repeat(3000)}`);
  assert.equal(await loud.closing(), 1008);
  assert.equal(await desk.message(), `c${opened.slice(1)}`);
  // Many small ones: the first four pass, the fifth ends it.
  const chatty = await client(hostId);
  assert.equal(await chatty.message(), "h1");
  const cid = (await desk.message()).slice(1);
  for (let sent = 0; sent < 4; sent += 1) {
    chatty.send("mH{}");
    assert.equal(await desk.message(), `m${cid} H{}`);
  }
  chatty.send("mH{}");
  assert.equal(await chatty.closing(), 1008);
  assert.equal(await desk.message(), `c${cid}`);
  // Once the computer accepts a browser, it is held to the ordinary limits instead.
  const paired = await client(hostId);
  assert.equal(await paired.message(), "h1");
  const accepted = (await desk.message()).slice(1);
  desk.send(`a${accepted}`);
  await desk.quiet(100);
  for (let sent = 0; sent < 8; sent += 1) paired.send(`m${"y".repeat(4000)}`);
  for (let sent = 0; sent < 8; sent += 1)
    assert.equal((await desk.message()).length, `m${accepted} `.length + 4000);
  desk.close();
});

test("a browser socket needs the website's origin", async () => {
  const hostKey = id(32);
  const hostId = idOf(hostKey);
  (await host(hostId, hostKey)).close();
  assert.equal((await upgrade(`client/${hostId}`)).status, 403);
  // Only the site's origin and the one it had before, each exactly: no other site, no name
  // that contains one of them, no missing or empty origin.
  for (const origin of [
    "https://evil.test",
    `${ORIGIN}.evil.test`,
    `${FORMER}.evil.test`,
    ORIGIN.replace("https://", "http://"),
    `${FORMER}/`,
    "null",
    "",
    "undefined",
  ]) {
    assert.equal((await upgrade(`client/${hostId}`, { Origin: origin })).status, 403, origin);
  }
  // The origin the site had before is let in, to the same room: its desktop hears of it.
  const desk = await host(hostId, hostKey);
  const former = await upgrade(`client/${hostId}`, { Origin: FORMER });
  assert.equal(former.status, 101);
  const phone = reader(former.webSocket);
  assert.equal(await phone.message(), "h1");
  assert.match(await desk.message(), /^o[A-Za-z0-9_-]{16}$/);
  phone.close();
  desk.close();
  assert.equal(
    (await runtime.dispatchFetch(`${BASE}/client/${hostId}`, { headers: { Origin: ORIGIN } }))
      .status,
    400,
  );
});

test("the iPhone app's socket comes without an origin, on a path no page can open", async () => {
  const key = id(32);
  const hostId = idOf(key);
  // Nothing is behind an id no desktop has claimed, on this path either.
  assert.equal((await upgrade(`app/${hostId}`)).status, 404);
  (await host(hostId, key)).close();
  // A page always sends an Origin (its own, or "null" from a sandboxed frame or a file), so any
  // Origin at all is refused: the site's own, the one it had before, another site's, "null".
  // An empty one never reaches a Worker: the runtime drops a header with no value, so it arrives
  // as none, and no page can send one.
  for (const origin of [ORIGIN, FORMER, "https://evil.test", "null", "undefined"])
    assert.equal((await upgrade(`app/${hostId}`, { Origin: origin })).status, 403, origin);
  // A plain request is not a socket, with an Origin or without.
  for (const init of [{}, { headers: { Origin: ORIGIN } }, { method: "POST" }])
    assert.equal(
      (await runtime.dispatchFetch(`${BASE}/app/${hostId}`, init)).status,
      400,
      JSON.stringify(init),
    );
  // The browser's path still needs the site's origin: without one it is refused there.
  assert.equal((await upgrade(`client/${hostId}`)).status, 403);

  // Without an Origin the app is let in, and its desktop is told which path it came by: `k`
  // with the same connection id, right after `o`.
  const desk = await host(hostId, key);
  const phone = await app(hostId);
  assert.equal(await phone.message(), "h1");
  const opened = await desk.message();
  assert.match(opened, /^o[A-Za-z0-9_-]{16}$/);
  const cid = opened.slice(1);
  assert.equal(await desk.message(), `k${cid}`);
  // It talks to its desktop as a browser does.
  phone.send('mH{"v":1}');
  assert.equal(await desk.message(), `m${cid} H{"v":1}`);
  desk.send(`m${cid} W{"n":"a b c"}`);
  assert.equal(await phone.message(), 'mW{"n":"a b c"}');
  phone.send("ping");
  assert.equal(await phone.message(), "pong");
  // A browser is not marked.
  const browser = await client(hostId);
  assert.equal(await browser.message(), "h1");
  const other = await desk.message();
  assert.match(other, /^o[A-Za-z0-9_-]{16}$/);
  await desk.quiet();
  phone.close();
  assert.equal(await desk.message(), `c${cid}`);
  browser.close();
  assert.equal(await desk.message(), `c${other.slice(1)}`);
  desk.close();
});

test("a desktop that connects again is told again which sockets came by the app's path", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const desk = await host(hostId, key);
  const phone = await app(hostId);
  assert.equal(await phone.message(), "h1");
  const mine = (await desk.message()).slice(1);
  assert.equal(await desk.message(), `k${mine}`);
  const browser = await client(hostId);
  assert.equal(await browser.message(), "h1");
  const theirs = (await desk.message()).slice(1);
  // The desktop accepted the app's session.
  desk.send(`a${mine}`);
  await desk.quiet(100);

  // A replacing socket and a returning one each hear `o` for every socket again, and `k` right
  // after the app's.
  const told = async (socket) => {
    const frames = [await socket.message(), await socket.message(), await socket.message()];
    assert.deepEqual([...frames].sort(), [`k${mine}`, `o${mine}`, `o${theirs}`].sort());
    assert.equal(frames.indexOf(`k${mine}`), frames.indexOf(`o${mine}`) + 1, String(frames));
    await socket.quiet();
  };
  const replacement = await host(hostId, key);
  assert.equal(await desk.message(), "x");
  for (const socket of [phone, browser]) {
    assert.equal(await socket.message(), "h0");
    assert.equal(await socket.message(), "h1");
  }
  await told(replacement);
  replacement.close();
  for (const socket of [phone, browser]) assert.equal(await socket.message(), "h0");
  const back = await host(hostId, key);
  for (const socket of [phone, browser]) assert.equal(await socket.message(), "h1");
  await told(back);
  // Accepted before, the app keeps its place while it starts again, as a browser does.
  phone.send("mEagain");
  assert.equal(await back.message(), `m${mine} Eagain`);
  back.close();
});

test("the app's socket is held to every limit a browser's is", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const desk = await host(hostId, key);
  const open = async (path, from) => {
    const response = await upgrade(`${path}/${hostId}`, {
      ...(path === "client" ? { Origin: ORIGIN } : {}),
      "CF-Connecting-IP": from,
    });
    assert.equal(response.status, 101);
    const socket = reader(response.webSocket);
    assert.equal(await socket.message(), "h1");
    const cid = (await desk.message()).slice(1);
    if (path === "app") assert.equal(await desk.message(), `k${cid}`);
    return { socket, cid };
  };

  // Before the desktop accepts it: the handshake's four small messages, and nothing more.
  const chatty = await open("app", "192.0.2.1");
  for (let sent = 0; sent < 4; sent += 1) {
    chatty.socket.send("mH{}");
    assert.equal(await desk.message(), `m${chatty.cid} H{}`);
  }
  chatty.socket.send("mH{}");
  assert.equal(await chatty.socket.closing(), 1008);
  assert.equal(await desk.message(), `c${chatty.cid}`);
  const loud = await open("app", "192.0.2.2");
  loud.socket.send(`m${"x".repeat(3000)}`);
  assert.equal(await loud.socket.closing(), 1008);
  assert.equal(await desk.message(), `c${loud.cid}`);

  // Accepted: at most 96 KiB a message and 240 messages in ten seconds.
  const large = await open("app", "192.0.2.3");
  desk.send(`a${large.cid}`);
  await desk.quiet(100);
  large.socket.send(`m${"y".repeat(4000)}`);
  assert.equal((await desk.message()).length, `m${large.cid} `.length + 4000);
  large.socket.send(`m${"a".repeat(96 * 1024)}`);
  assert.equal(await large.socket.closing(), 1009);
  assert.equal(await desk.message(), `c${large.cid}`);
  const flood = await open("app", "192.0.2.4");
  desk.send(`a${flood.cid}`);
  await desk.quiet(100);
  for (let sent = 0; sent < 241; sent += 1) flood.socket.send("mE");
  assert.equal(await flood.socket.closing(), 1008);
  for (let sent = 0; sent < 240; sent += 1) assert.equal(await desk.message(), `m${flood.cid} E`);
  assert.equal(await desk.message(), `c${flood.cid}`);

  // Unproven app and browser sockets share the same few places: three per address, and a
  // newcomer replaces the oldest whichever path either came by.
  const first = await open("client", "198.51.100.7");
  await open("app", "198.51.100.7");
  await open("app", "198.51.100.7");
  const fourth = await upgrade(`app/${hostId}`, { "CF-Connecting-IP": "198.51.100.7" });
  assert.equal(fourth.status, 101);
  assert.equal(await first.socket.message(), "x");
  assert.equal(await desk.message(), `c${first.cid}`);
  const newest = reader(fourth.webSocket);
  assert.equal(await newest.message(), "h1");
  const cid = (await desk.message()).slice(1);
  assert.equal(await desk.message(), `k${cid}`);

  // The desktop leaving tells the app, as it tells a browser.
  desk.close();
  assert.equal(await newest.message(), "h0");
});

test("forwards a browser's messages to its desktop and back, and says who is there", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const desk = await host(hostId, key);
  const phone = await client(hostId);
  assert.equal(await phone.message(), "h1");
  const opened = await desk.message();
  assert.match(opened, /^o[A-Za-z0-9_-]{16}$/);
  const cid = opened.slice(1);

  phone.send('mH{"v":1}');
  assert.equal(await desk.message(), `m${cid} H{"v":1}`);
  desk.send(`m${cid} W{"n":"a b c"}`);
  assert.equal(await phone.message(), 'mW{"n":"a b c"}');

  // A second browser has its own id, and neither sees the other's messages.
  const tablet = await client(hostId);
  assert.equal(await tablet.message(), "h1");
  const other = (await desk.message()).slice(1);
  assert.notEqual(other, cid);
  desk.send(`m${other} Etablet`);
  assert.equal(await tablet.message(), "mEtablet");
  await phone.quiet();

  // Keep-alives are answered by the relay.
  phone.send("ping");
  assert.equal(await phone.message(), "pong");
  desk.send("ping");
  assert.equal(await desk.message(), "pong");

  // The desktop can end one browser's connection.
  desk.send(`c${other}`);
  assert.equal(await tablet.message(), "x");
  tablet.close();
  desk.send(`m${other} Egone`);
  assert.equal(await desk.message(), `c${other}`);

  // A browser that leaves is reported, and messages for it come back as closed.
  phone.close();
  assert.equal(await desk.message(), `c${cid}`);
  desk.send(`m${cid} Elate`);
  assert.equal(await desk.message(), `c${cid}`);
  desk.close();
});

test("tells browsers when the desktop goes and comes back", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const desk = await host(hostId, key);
  const phone = await client(hostId);
  assert.equal(await phone.message(), "h1");
  const cid = (await desk.message()).slice(1);
  desk.close();
  assert.equal(await phone.message(), "h0");
  phone.send("mEwhile away");
  assert.equal(await phone.message(), "h0");

  const back = await host(hostId, key);
  assert.equal(await phone.message(), "h1");
  assert.equal(await back.message(), `o${cid}`);

  // A second host socket replaces the first; the browser starts its session again.
  const replacement = await host(hostId, key);
  assert.equal(await back.message(), "x");
  assert.equal(await phone.message(), "h0");
  assert.equal(await phone.message(), "h1");
  assert.equal(await replacement.message(), `o${cid}`);
  phone.send("mEagain");
  assert.equal(await replacement.message(), `m${cid} Eagain`);
  await phone.quiet();

  // A browser that connects while the desktop is away waits for it.
  replacement.close();
  assert.equal(await phone.message(), "h0");
  const late = await client(hostId);
  assert.equal(await late.message(), "h0");
  const again = await host(hostId, key);
  assert.equal(await late.message(), "h1");
  assert.equal(await phone.message(), "h1");
  again.close();
});

test("a desktop that stops answering is away, not connected", async () => {
  // A room of its own, where a desktop may stay silent for less than a second.
  const quick = new Miniflare(
    convertV4MiniflareOptions({
      modules: [
        {
          type: "ESModule",
          path: "index.js",
          contents: await readFile(
            fileURLToPath(new URL("../build/index.js", import.meta.url)),
            "utf8",
          ),
        },
      ],
      compatibilityDate: "2026-10-26",
      durableObjects: { ROOMS: { className: "Room", useSQLite: true } },
      bindings: { APP_ORIGIN: ORIGIN, HOST_SILENT_MS: "800" },
    }),
  );
  try {
    await quick.ready;
    const open = async (path, headers) => {
      const response = await quick.dispatchFetch(`${BASE}/${path}`, {
        headers: { Upgrade: "websocket", ...headers },
      });
      assert.equal(response.status, 101);
      return reader(response.webSocket);
    };
    const key = id(32);
    const hostId = idOf(key);
    const desk = await open(`host/${hostId}`, { Authorization: `Bearer ${key}` });
    const phone = await open(`client/${hostId}`, { Origin: ORIGIN });
    assert.equal(await phone.message(), "h1");
    const cid = (await desk.message()).slice(1);
    // The desktop keeps saying "ping": it stays.
    for (let beat = 0; beat < 3; beat += 1) {
      await new Promise((resolve) => setTimeout(resolve, 400));
      desk.send("ping");
      assert.equal(await desk.message(), "pong");
    }
    phone.send("mEstill here");
    assert.equal(await desk.message(), `m${cid} Estill here`);
    // No keep-alive for longer than allowed, but the desktop keeps sending: it is plainly there.
    for (let beat = 0; beat < 3; beat += 1) {
      await new Promise((resolve) => setTimeout(resolve, 400));
      desk.send(`m${cid} Ebusy ${beat}`);
      assert.equal(await phone.message(), `mEbusy ${beat}`);
    }
    phone.send("mEand still here");
    assert.equal(await desk.message(), `m${cid} Eand still here`);
    // Silent for longer than allowed: the next browser message finds it gone, and is told so.
    await new Promise((resolve) => setTimeout(resolve, 1000));
    phone.send("mEhello?");
    assert.equal(await phone.message(), "h0");
    assert.equal(await desk.message(), "x");
    // A browser that opens now waits for the desktop rather than talking into its old socket.
    const late = await open(`client/${hostId}`, { Origin: ORIGIN });
    assert.equal(await late.message(), "h0");
    // The desktop connects again: both browsers start over with it.
    const back = await open(`host/${hostId}`, { Authorization: `Bearer ${key}` });
    assert.equal(await phone.message(), "h1");
    assert.equal(await late.message(), "h1");
    back.close();
  } finally {
    await quick.dispose();
  }
});

test("refuses what a browser should not send", async () => {
  const hostKey = id(32);
  const hostId = idOf(hostKey);
  const desk = await host(hostId, hostKey);
  const noisy = await client(hostId);
  await noisy.message();
  await desk.message();
  noisy.send("xnot a message");
  assert.equal(await noisy.closing(), 1008);
  await desk.message();

  const large = await client(hostId);
  await large.message();
  await desk.message();
  large.send(`m${"a".repeat(96 * 1024)}`);
  assert.equal(await large.closing(), 1009);
  await desk.message();

  const flood = await client(hostId);
  await flood.message();
  await desk.message();
  for (let sent = 0; sent < 241; sent += 1) flood.send("mE");
  assert.equal(await flood.closing(), 1008);
  desk.close();
});

// A copy of the app from before the current tools sends its links without a set: its bots are
// listed the first tools, which are the ones it answers.
test("serves a bot's MCP calls through the desktop", async () => {
  const hostKey = id(32);
  const hostId = idOf(hostKey);
  const bot = id(8);
  const token = id(32);
  const reference = `${bot}.${token}`;
  const initialize = {
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: { protocolVersion: "2025-06-18" },
  };

  // No such link until the desktop lists it.
  const desk = await host(hostId, hostKey);
  assert.equal((await mcp(hostId, reference, initialize)).status, 404);
  // A second link, made with full control. Anything but `true` is not full control.
  const wide = id(8);
  const wideToken = id(32);
  desk.send(
    `l${JSON.stringify([
      { id: bot, hash: hash(token), full: "yes" },
      { id: wide, hash: hash(wideToken), full: true },
      { id: "bad", hash: "bad", full: true },
    ])}`,
  );
  await desk.quiet(100);
  assert.equal((await mcp(hostId, `${bot}.${id(32)}`, initialize)).status, 404);
  assert.equal((await mcp(hostId, `${id(8)}.${token}`, initialize)).status, 404);
  assert.equal((await mcp(id(16), reference, initialize)).status, 404);

  const started = await mcp(hostId, reference, initialize);
  assert.equal(started.status, 200);
  assert.equal(started.headers.get("access-control-allow-origin"), "*");
  const hello = await started.json();
  assert.equal(hello.result.protocolVersion, "2025-06-18");
  assert.deepEqual(hello.result.capabilities, { tools: { listChanged: false } });
  assert.equal(hello.result.serverInfo.name, "o7");
  assert.match(hello.result.instructions, /list_chats/);
  // An unknown protocol version gets the newest one this server speaks.
  const newer = await (
    await mcp(hostId, reference, { ...initialize, params: { protocolVersion: "2099-01-01" } })
  ).json();
  assert.equal(newer.result.protocolVersion, "2025-11-25");

  // The token may travel in the Authorization header instead of the URL.
  const byHeader = await mcp(
    hostId,
    bot,
    { jsonrpc: "2.0", id: 2, method: "ping" },
    {
      headers: { Authorization: `Bearer ${token}` },
    },
  );
  assert.deepEqual(await byHeader.json(), { jsonrpc: "2.0", id: 2, result: {} });

  const listed = await (
    await mcp(hostId, reference, { jsonrpc: "2.0", id: "a", method: "tools/list" })
  ).json();
  assert.deepEqual(
    listed.result.tools.map((tool) => tool.name),
    [
      "list_chats",
      "list_projects",
      "read_chat",
      "send_message",
      "wait_for_chat",
      "respond_to_request",
      "stop_chat",
      "start_chat",
    ],
  );
  for (const tool of listed.result.tools) {
    assert.equal(tool.inputSchema.type, "object");
    assert.ok(tool.description.length > 40);
  }
  // A link without full control is not shown the wider tools, and they do not exist for it:
  // nothing reaches the desktop.
  const refused = await (
    await mcp(hostId, reference, {
      jsonrpc: "2.0",
      id: "w",
      method: "tools/call",
      params: { name: "set_access", arguments: { chat_id: "c1", access: "full_auto" } },
    })
  ).json();
  assert.equal(refused.error.code, -32602);
  await desk.quiet(100);
  // The full-control link is shown all of them, each a plain MCP tool with no mark of ours.
  const everything = await (
    await mcp(hostId, `${wide}.${wideToken}`, { jsonrpc: "2.0", id: "b", method: "tools/list" })
  ).json();
  const names = everything.result.tools.map((tool) => tool.name);
  assert.equal(names.length, 21);
  assert.deepEqual(
    names.slice(0, 8),
    listed.result.tools.map((tool) => tool.name),
  );
  for (const name of [
    "create_project",
    "chat_options",
    "set_model",
    "set_thinking",
    "set_access",
    "switch_agent",
    "rename_chat",
    "archive_chat",
    "start_agent",
    "list_changes",
    "read_change",
    "read_memory",
    "write_memory",
  ])
    assert.ok(names.includes(name), name);
  for (const tool of everything.result.tools) {
    assert.equal("full" in tool, false, tool.name);
    assert.equal(tool.inputSchema.additionalProperties, false, tool.name);
    assert.ok(tool.description.length > 20, tool.name);
  }
  const widened = mcp(hostId, `${wide}.${wideToken}`, {
    jsonrpc: "2.0",
    id: "c",
    method: "tools/call",
    params: { name: "set_access", arguments: { chat_id: "c1", access: "ask" } },
  });
  const passed = JSON.parse((await desk.message()).slice(1));
  assert.deepEqual(
    { ...passed, rid: "" },
    {
      rid: "",
      bot: wide,
      token: wideToken,
      tool: "set_access",
      args: { chat_id: "c1", access: "ask" },
    },
  );
  desk.send(`b${JSON.stringify({ rid: passed.rid, text: "Access is now ask.", error: false })}`);
  assert.equal((await (await widened).json()).result.isError, false);

  // Notifications get no body.
  const noted = await mcp(hostId, reference, {
    jsonrpc: "2.0",
    method: "notifications/initialized",
  });
  assert.equal(noted.status, 202);
  assert.equal(await noted.text(), "");

  // A tool call is one request to the desktop, which answers it.
  const calling = mcp(hostId, reference, {
    jsonrpc: "2.0",
    id: 3,
    method: "tools/call",
    params: { name: "send_message", arguments: { chat_id: "c1", text: "Run the tests" } },
  });
  const asked = await desk.message();
  assert.equal(asked[0], "b");
  const call = JSON.parse(asked.slice(1));
  assert.deepEqual(
    { ...call, rid: "" },
    { rid: "", bot, token, tool: "send_message", args: { chat_id: "c1", text: "Run the tests" } },
  );
  desk.send(`b${JSON.stringify({ rid: call.rid, text: "Sent.", error: false })}`);
  assert.deepEqual(await (await calling).json(), {
    jsonrpc: "2.0",
    id: 3,
    result: { content: [{ type: "text", text: "Sent." }], isError: false },
  });

  // A batch answers each request and skips notifications.
  const batch = mcp(hostId, reference, [
    { jsonrpc: "2.0", id: 4, method: "ping" },
    { jsonrpc: "2.0", method: "notifications/cancelled" },
    { jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "list_chats" } },
  ]);
  const second = JSON.parse((await desk.message()).slice(1));
  assert.deepEqual(second.args, {});
  desk.send(`b${JSON.stringify({ rid: second.rid, text: "Nothing is open.", error: true })}`);
  assert.deepEqual(await (await batch).json(), [
    { jsonrpc: "2.0", id: 4, result: {} },
    {
      jsonrpc: "2.0",
      id: 5,
      result: { content: [{ type: "text", text: "Nothing is open." }], isError: true },
    },
  ]);

  // A bot that gives up on a waiting call says so, and the call ends at once instead of holding
  // a place until the desktop answers. Only the id the call was made with ends it.
  const givenUp = mcp(hostId, reference, {
    jsonrpc: "2.0",
    id: 9,
    method: "tools/call",
    params: { name: "list_chats" },
  });
  const held = JSON.parse((await desk.message()).slice(1));
  for (const requestId of ["9", 10]) {
    const ignored = await mcp(hostId, reference, {
      jsonrpc: "2.0",
      method: "notifications/cancelled",
      params: { requestId },
    });
    assert.equal(ignored.status, 202);
  }
  const cancelled = await mcp(hostId, reference, {
    jsonrpc: "2.0",
    method: "notifications/cancelled",
    params: { requestId: 9, reason: "timed out" },
  });
  assert.equal(cancelled.status, 202);
  assert.deepEqual(await (await givenUp).json(), {
    jsonrpc: "2.0",
    id: 9,
    result: {
      content: [{ type: "text", text: "Cancelled. The chat carries on." }],
      isError: true,
    },
  });
  // The desktop's late answer to a call nobody waits for is dropped.
  desk.send(`b${JSON.stringify({ rid: held.rid, text: "Late.", error: false })}`);

  // What the server does not offer.
  const errors = [
    [{ jsonrpc: "2.0", id: 6, method: "resources/list" }, -32601],
    [{ jsonrpc: "2.0", id: 7, method: "tools/call", params: { name: "rm_rf" } }, -32602],
    [
      {
        jsonrpc: "2.0",
        id: 8,
        method: "tools/call",
        params: { name: "list_chats", arguments: [] },
      },
      -32602,
    ],
    [{ id: 9, method: "ping" }, -32600],
  ];
  for (const [message, code] of errors)
    assert.equal((await (await mcp(hostId, reference, message)).json()).error.code, code);
  assert.equal((await mcp(hostId, reference, "{nope")).status, 400);
  assert.equal((await mcp(hostId, reference, [])).status, 400);
  const got = await runtime.dispatchFetch(`${BASE}/mcp/${hostId}/${reference}`);
  assert.equal(got.status, 405);
  const preflight = await runtime.dispatchFetch(`${BASE}/mcp/${hostId}/${reference}`, {
    method: "OPTIONS",
  });
  assert.equal(preflight.status, 204);
  assert.match(preflight.headers.get("access-control-allow-headers"), /Mcp-Protocol-Version/);

  // A call waiting when the desktop leaves cannot know whether it was done; any call after did
  // nothing, and says when the computer was last connected.
  const waiting = mcp(hostId, reference, {
    jsonrpc: "2.0",
    id: 10,
    method: "tools/call",
    params: { name: "list_chats" },
  });
  await desk.message();
  desk.close();
  const dropped = await (await waiting).json();
  assert.equal(dropped.result.isError, true);
  assert.match(dropped.result.content[0].text, /^The computer went away during this call/);
  const away = await (
    await mcp(hostId, reference, {
      jsonrpc: "2.0",
      id: 11,
      method: "tools/call",
      params: { name: "list_chats" },
    })
  ).json();
  assert.match(
    away.result.content[0].text,
    /^Nothing was done: the computer is not reachable\. .* It was last connected less than a minute ago\. Tell your person/,
  );
  // Listing tools still works, so a bot can be connected while the computer is off.
  assert.equal(
    (await mcp(hostId, reference, { jsonrpc: "2.0", id: 12, method: "tools/list" })).status,
    200,
  );
});

test("lists the current tools to a bot whose computer answers them", async () => {
  const hostKey = id(32);
  const hostId = idOf(hostKey);
  const [chats, wide, older, odd, newer, newest, reader, four] = [0, 1, 2, 3, 4, 5, 6, 7].map(
    () => ({
      id: id(8),
      token: id(32),
    }),
  );
  const desk = await host(hostId, hostKey);
  desk.send(
    `l${JSON.stringify([
      { id: chats.id, hash: hash(chats.token), full: false, v: 2 },
      { id: wide.id, hash: hash(wide.token), full: true, v: 2 },
      // No set named, and a set this relay does not know: the first tools.
      { id: older.id, hash: hash(older.token), full: true },
      { id: odd.id, hash: hash(odd.token), full: true, v: "2" },
      // The latest set, and a copy of the app newer than this relay: the latest it knows.
      { id: newer.id, hash: hash(newer.token), full: false, v: 3 },
      { id: newest.id, hash: hash(newest.token), full: true, v: 6 },
      { id: four.id, hash: hash(four.token), full: true, v: 4 },
      // A read-only link: shown what reads, even if it also claims full control.
      { id: reader.id, hash: hash(reader.token), full: true, read: true, v: 3 },
    ])}`,
  );
  await desk.quiet(100);
  const ask = async (link, body) =>
    (await mcp(hostId, `${link.id}.${link.token}`, { jsonrpc: "2.0", id: 1, ...body })).json();
  const tools = async (link) => (await ask(link, { method: "tools/list" })).result.tools;

  const few = await tools(chats);
  assert.deepEqual(
    few.map((tool) => tool.name),
    ["chats", "projects", "read", "send", "wait", "answer", "stop"],
  );
  const all = await tools(wide);
  assert.deepEqual(
    all.map((tool) => tool.name),
    [...few.map((tool) => tool.name), "set", "changes", "memory", "new_project"],
  );
  // What only a full-control link may choose is not offered to another, and no mark of this
  // relay's is in what either is shown.
  const send = (list) =>
    Object.keys(list.find((tool) => tool.name === "send").inputSchema.properties);
  assert.deepEqual(send(few), ["chat", "folder", "agent", "text", "wait"]);
  assert.deepEqual(send(all), [
    "chat",
    "folder",
    "agent",
    "text",
    "wait",
    "model",
    "thinking",
    "access",
  ]);
  for (const tool of all) {
    assert.equal("full" in tool, false, tool.name);
    for (const [name, argument] of Object.entries(tool.inputSchema.properties))
      assert.equal("full" in argument, false, `${tool.name}.${name}`);
    assert.equal(tool.inputSchema.type, "object", tool.name);
    assert.equal(tool.inputSchema.additionalProperties, false, tool.name);
    assert.ok(tool.description.length > 20, tool.name);
  }
  // A copy of the app that answers the latest set is also shown inbox, first; one that does
  // not is never shown a tool it would refuse.
  const latest = await tools(newer);
  assert.deepEqual(
    latest.map((tool) => tool.name),
    ["inbox", ...few.map((tool) => tool.name)],
  );
  // A copy that answers set 4 is also shown task, before send; one that answers set 5 (and one
  // newer than this relay) is shown look too, after changes, with full control.
  const latestAll = await tools(newest);
  const withTask = (names) => {
    const at = names.indexOf("send");
    return [...names.slice(0, at), "task", ...names.slice(at)];
  };
  const withLook = (names) => {
    const at = names.indexOf("changes") + 1;
    return [...names.slice(0, at), "look", ...names.slice(at)];
  };
  assert.deepEqual(
    (await tools(four)).map((tool) => tool.name),
    withTask(["inbox", ...all.map((tool) => tool.name)]),
  );
  assert.deepEqual(
    latestAll.map((tool) => tool.name),
    withLook(withTask(["inbox", ...all.map((tool) => tool.name)])),
  );
  assert.match(
    (await ask(newest, { method: "initialize", params: {} })).result.instructions,
    /call task once/,
  );
  // Set 6 adds what task takes for tonight; an earlier set is never shown it.
  const taskOf = (list) =>
    Object.keys(list.find((tool) => tool.name === "task").inputSchema.properties);
  assert.deepEqual(taskOf(latestAll), [
    "goal",
    "project",
    "agent",
    "review",
    "wait",
    "when",
    "cancel",
  ]);
  assert.deepEqual(taskOf(await tools(four)), ["goal", "project", "agent", "review", "wait"]);
  for (const tool of latestAll)
    for (const [name, argument] of Object.entries(tool.inputSchema.properties))
      assert.equal("since" in argument, false, `${tool.name}.${name}`);
  assert.match(
    (await ask(newest, { method: "initialize", params: {} })).result.instructions,
    /when tonight/,
  );
  for (const tool of [...latestAll, ...all]) assert.equal("since" in tool, false, tool.name);
  assert.match(
    (await ask(newer, { method: "initialize", params: {} })).result.instructions,
    /inbox says what needs you/,
  );
  const reading = await tools(reader);
  assert.deepEqual(
    reading.map((tool) => tool.name),
    ["inbox", "chats", "projects", "read", "wait"],
  );
  assert.match(
    (await ask(reader, { method: "initialize", params: {} })).result.instructions,
    /cannot send/,
  );
  for (const name of ["send", "answer", "stop", "changes", "set"]) {
    const refused = await ask(reader, { method: "tools/call", params: { name, arguments: {} } });
    assert.equal(refused.error.code, -32602, name);
  }
  const refusedInbox = await ask(chats, {
    method: "tools/call",
    params: { name: "inbox", arguments: {} },
  });
  assert.equal(refusedInbox.error.code, -32602, "inbox is not in the second set");

  // The point of the set: it costs a bot well under half of what the first one did to carry.
  const size = (list) => JSON.stringify({ tools: list }).length;
  const first = await tools(older);
  assert.equal(first.length, 21);
  assert.equal((await tools(odd)).length, 21);
  assert.ok(size(all) * 2 < size(first), `${size(all)} of ${size(first)}`);
  assert.ok(size(few) < 3500, String(size(few)));
  assert.ok(size(all) < 5600, String(size(all)));
  assert.ok(size(latest) < 3900, String(size(latest)));
  assert.ok(size(latestAll) < 7900, String(size(latestAll)));

  const hello = await ask(chats, { method: "initialize", params: {} });
  assert.match(hello.result.instructions, /one send/);
  assert.doesNotMatch(hello.result.instructions, /list_chats|inbox/);
  assert.match(
    (await ask(older, { method: "initialize", params: {} })).result.instructions,
    /list_chats/,
  );

  // A tool of the other set, or above the link's level, does not exist for it.
  for (const [link, name] of [
    [chats, "set"],
    [chats, "list_chats"],
    [older, "send"],
  ]) {
    const refused = await ask(link, { method: "tools/call", params: { name, arguments: {} } });
    assert.equal(refused.error.code, -32602, name);
  }
  await desk.quiet(100);
  const calling = ask(chats, {
    method: "tools/call",
    params: { name: "send", arguments: { chat: "c1", text: "Run the tests" } },
  });
  const call = JSON.parse((await desk.message()).slice(1));
  assert.deepEqual(
    { ...call, rid: "" },
    {
      rid: "",
      bot: chats.id,
      token: chats.token,
      tool: "send",
      args: { chat: "c1", text: "Run the tests" },
    },
  );
  desk.send(`b${JSON.stringify({ rid: call.rid, text: "The turn finished.", error: false })}`);
  assert.deepEqual((await calling).result, {
    content: [{ type: "text", text: "The turn finished." }],
    isError: false,
  });

  // look's picture goes to the bot as MCP shows one; anything that is not a PNG or a JPEG in
  // base64, or a picture with any other tool's answer, does not.
  const looked = async (link, name, image) => {
    const asking = ask(link, { method: "tools/call", params: { name, arguments: { chat: "c1" } } });
    const asked = JSON.parse((await desk.message()).slice(1));
    desk.send(
      `b${JSON.stringify({ rid: asked.rid, text: "Chat c1's browser tab.", error: false, image })}`,
    );
    return (await asking).result.content;
  };
  const png = { mime: "image/png", data: "iVBORw0KGgo=" };
  assert.deepEqual(await looked(newest, "look", png), [
    { type: "text", text: "Chat c1's browser tab." },
    { type: "image", data: "iVBORw0KGgo=", mimeType: "image/png" },
  ]);
  for (const bad of [
    { mime: "text/html", data: "PGgxPg==" },
    { mime: "image/png", data: "not base64!" },
    { mime: "image/svg+xml", data: "PHN2Zz4=" },
    "iVBORw0KGgo=",
  ])
    assert.equal((await looked(newest, "look", bad)).length, 1, JSON.stringify(bad));
  assert.equal((await looked(newest, "read", png)).length, 1, "only look shows a picture");
  const notListed = await ask(chats, {
    method: "tools/call",
    params: { name: "look", arguments: { chat: "c1" } },
  });
  assert.equal(notListed.error.code, -32602, "a link without full control has no look");
  desk.close();
});

test("an automation's address takes what a service sends, and is no MCP server", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const hook = { id: id(8), token: id(32) };
  const bot = { id: id(8), token: id(32) };
  const desk = await host(hostId, key);
  desk.send(
    `l${JSON.stringify([
      { id: hook.id, hash: hash(hook.token), full: false, v: 6, hook: true },
      { id: bot.id, hash: hash(bot.token), full: false, v: 6 },
    ])}`,
  );
  await desk.quiet(100);
  const send = (link, body, headers = {}) =>
    runtime.dispatchFetch(`${BASE}/hook/${hostId}/${link.id}.${link.token}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body,
    });

  // What is sent reaches the computer whole, as the automation's one tool.
  const payload = JSON.stringify({ action: "completed", check_run: { conclusion: "failure" } });
  const answered = send(hook, payload);
  const call = JSON.parse((await desk.message()).slice(1));
  assert.deepEqual(
    { ...call, rid: "" },
    { rid: "", bot: hook.id, token: hook.token, tool: "trigger", args: { text: payload } },
  );
  desk.send(`b${JSON.stringify({ rid: call.rid, text: "Started task c2.", error: false })}`);
  const started = await answered;
  assert.equal(started.status, 202);
  assert.deepEqual(await started.json(), { ok: true, text: "Started task c2." });

  // Refused by the computer: 409, with why.
  const refused = send(hook, "again");
  const second = JSON.parse((await desk.message()).slice(1));
  desk.send(
    `b${JSON.stringify({ rid: second.rid, text: "Three of its tasks are still working.", error: true })}`,
  );
  assert.equal((await refused).status, 409);

  // A bot link's address is not an automation's, an automation's is no MCP server, a page is
  // not let in, and what is too large or empty is refused before the computer hears of it.
  assert.equal((await send(bot, "x")).status, 404);
  const asMcp = await runtime.dispatchFetch(`${BASE}/mcp/${hostId}/${hook.id}.${hook.token}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
  });
  assert.equal(asMcp.status, 404);
  assert.equal((await send(hook, "x", { Origin: "https://example.com" })).status, 403);
  assert.equal((await send({ ...hook, token: id(32) }, "x")).status, 404);
  assert.equal((await send(hook, "x".repeat(64 * 1024 + 1))).status, 413);
  assert.equal((await send(hook, "  ")).status, 400);
  const got = await runtime.dispatchFetch(`${BASE}/hook/${hostId}/${hook.id}.${hook.token}`);
  assert.equal(got.status, 405);

  // The computer away: 503, so a sender that retries gets it there later.
  desk.close();
  await new Promise((done) => setTimeout(done, 100));
  const away = await send(hook, "later");
  assert.equal(away.status, 503);
  assert.match((await away.json()).text, /^Nothing was done: the computer is not reachable/);
});

test("a removed bot link stops working", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const bot = id(8);
  const token = id(32);
  const desk = await host(hostId, key);
  desk.send(`l${JSON.stringify([{ id: bot, hash: hash(token) }])}`);
  await desk.quiet(100);
  const ping = { jsonrpc: "2.0", id: 1, method: "ping" };
  assert.equal((await mcp(hostId, `${bot}.${token}`, ping)).status, 200);
  desk.send("l[]");
  await desk.quiet(100);
  assert.equal((await mcp(hostId, `${bot}.${token}`, ping)).status, 404);
  desk.close();
});

test("sockets that prove nothing cannot keep a paired browser out", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const desk = await host(hostId, key);
  const open = async (from) => {
    const response = await upgrade(`client/${hostId}`, {
      Origin: ORIGIN,
      "CF-Connecting-IP": from,
    });
    assert.equal(response.status, 101);
    const socket = reader(response.webSocket);
    assert.equal(await socket.message(), "h1");
    return { socket, cid: (await desk.message()).slice(1) };
  };

  // The desktop accepted this browser's session; nothing below can take its place.
  const paired = await open("192.0.2.1");
  desk.send(`a${paired.cid}`);
  await desk.quiet(100);

  // One address holds three unproven sockets at most: its fourth replaces its oldest.
  const same = [];
  for (let count = 0; count < 3; count += 1) same.push(await open("192.0.2.9"));
  const fourth = await upgrade(`client/${hostId}`, {
    Origin: ORIGIN,
    "CF-Connecting-IP": "192.0.2.9",
  });
  assert.equal(fourth.status, 101);
  assert.equal(await same[0].socket.message(), "x");
  assert.equal(await desk.message(), `c${same[0].cid}`);
  const fourthSocket = reader(fourth.webSocket);
  assert.equal(await fourthSocket.message(), "h1");
  await desk.message();

  // Eight unproven sockets in all: a ninth, from anywhere, replaces the oldest of them.
  const others = [];
  for (let count = 0; count < 5; count += 1) others.push(await open(`198.51.100.${count}`));
  const ninth = await upgrade(`client/${hostId}`, {
    Origin: ORIGIN,
    "CF-Connecting-IP": "203.0.113.5",
  });
  assert.equal(ninth.status, 101);
  assert.equal(await same[1].socket.message(), "x");
  assert.equal(await desk.message(), `c${same[1].cid}`);

  // The paired browser was never touched and still talks to its desktop.
  await paired.socket.quiet(50);
  paired.socket.send("mEstill here");
  assert.equal(
    await desk.message().then((first) => (first.startsWith("o") ? desk.message() : first)),
    `m${paired.cid} Estill here`,
  );

  // Accepting a browser that already left is answered as closed.
  desk.send(`a${same[0].cid}`);
  let closed = "";
  while (closed !== `c${same[0].cid}`) closed = await desk.message();

  // The desktop reconnecting does not make an accepted browser replaceable.
  const again = await upgrade(`host/${hostId}`, { Authorization: `Bearer ${key}` });
  assert.equal(again.status, 101);
  const back = reader(again.webSocket);
  assert.equal(await paired.socket.message(), "h0");
  assert.equal(await paired.socket.message(), "h1");
  for (let count = 0; count < 12; count += 1) {
    const flood = await upgrade(`client/${hostId}`, {
      Origin: ORIGIN,
      "CF-Connecting-IP": `203.0.113.${count}`,
    });
    assert.equal(flood.status, 101);
  }
  await paired.socket.quiet(100);
  paired.socket.send("mEafter the flood");
  let seen = "";
  while (!seen.startsWith(`m${paired.cid}`)) seen = await back.message();
  assert.equal(seen, `m${paired.cid} Eafter the flood`);
  back.close();
});

// Notifications for o7 for iPhone. Apple is played here, through the runtime's outbound fetch,
// so nothing leaves the machine.

/** A phone's address at Apple: 32 bytes in hex, as iOS gives one. */
const deviceToken = () => randomBytes(32).toString("hex");
/** A notification as a desktop asks for one (`p`), with what a test changes. */
const notification = (fields = {}) => ({
  id: id(9),
  to: deviceToken(),
  env: "production",
  type: "alert",
  d: id(8),
  b: id(300),
  ...fields,
});
const p = (fields) => `p${JSON.stringify(fields)}`;

/** The relay's next answer to a notification (`P`). */
async function answer(desk) {
  const frame = await desk.message();
  assert.equal(frame[0], "P", frame);
  return JSON.parse(frame.slice(1));
}

/** The relay's next `count` answers, by the notification each answers: they come as Apple does. */
async function answers(desk, count) {
  const found = new Map();
  for (let at = 0; at < count; at += 1) {
    const said = await answer(desk);
    found.set(said.id, said);
  }
  return found;
}

/** A desktop's socket on the relay with Apple's key: the first it hears is that the relay sends. */
async function pushHost(hostId, key, on = pushing) {
  const desk = await host(hostId, key, on);
  assert.equal(await desk.message(), 'f{"push":1}');
  return desk;
}

/** The headers the relay put on a request to Apple: all but those the runtime adds. */
function appleHeaders(request) {
  const {
    host: _host,
    "content-length": _length,
    "cf-worker": _worker,
    ...headers
  } = request.headers;
  return Object.fromEntries(Object.entries(headers).filter(([name]) => !name.startsWith("mf-")));
}

/**
 * The token on a request to Apple, checked against the push key's public half (a signature that
 * is r and s side by side, as a JWT has it), and opened: its header and claims as written.
 */
async function provider(authorization) {
  const [scheme, jwt, ...more] = authorization.split(" ");
  assert.equal(scheme, "bearer");
  assert.equal(more.length, 0);
  const [header, claims, signature, ...extra] = jwt.split(".");
  assert.equal(extra.length, 0);
  const verified = await crypto.subtle.verify(
    { name: "ECDSA", hash: "SHA-256" },
    signing.publicKey,
    Buffer.from(signature, "base64url"),
    new TextEncoder().encode(`${header}.${claims}`),
  );
  assert.ok(verified, "the token does not verify with the push key");
  const text = (part) => Buffer.from(part, "base64url").toString();
  const { iss, iat } = JSON.parse(text(claims));
  return { header: text(header), claims: text(claims), iss, iat };
}

/** Checks that `value` is from `low` to `high`. */
function within(value, low, high, what) {
  assert.ok(low <= value && value <= high, `${what}: ${value} is not within ${low} and ${high}`);
}

test("a relay without Apple's key tells no desktop it sends notifications, and sends none", async () => {
  assert.equal((await (await runtime.dispatchFetch(`${BASE}/health`)).json()).push, false);
  const key = id(32);
  const hostId = idOf(key);
  const desk = await host(hostId, key);
  await desk.quiet();
  // Nor a socket that replaces it.
  const replacement = await host(hostId, key);
  assert.equal(await desk.message(), "x");
  await replacement.quiet();
  // A desktop that asks anyway is told so, and stays connected.
  const asked = notification();
  replacement.send(p(asked));
  assert.deepEqual(await answer(replacement), {
    id: asked.id,
    ok: false,
    reason: "off",
    drop: false,
  });
  replacement.send("ping");
  assert.equal(await replacement.message(), "pong");
  assert.deepEqual(offline.requests, []);
  replacement.close();
});

test("a relay with Apple's key tells each desktop socket so, and its health says so", async () => {
  assert.equal((await (await pushing.dispatchFetch(`${BASE}/health`)).json()).push, true);
  const key = id(32);
  const hostId = idOf(key);
  const desk = await pushHost(hostId, key);
  const phone = await client(hostId, pushing);
  assert.equal(await phone.message(), "h1");
  const cid = (await desk.message()).slice(1);
  // A socket that replaces it hears so first, then of the browsers there.
  const replacement = await host(hostId, key, pushing);
  assert.equal(await desk.message(), "x");
  assert.equal(await replacement.message(), 'f{"push":1}');
  assert.equal(await replacement.message(), `o${cid}`);
  await replacement.quiet();
  assert.equal(await phone.message(), "h0");
  assert.equal(await phone.message(), "h1");
  // So does one that comes back after the desktop was away.
  replacement.close();
  assert.equal(await phone.message(), "h0");
  const back = await pushHost(hostId, key);
  assert.equal(await back.message(), `o${cid}`);
  assert.equal(await phone.message(), "h1");
  back.close();
});

test("a notification the relay cannot send is answered invalid, and the desktop stays connected", async () => {
  const key = id(32);
  const hostId = idOf(key);
  const desk = await pushHost(hostId, key);
  const phone = await client(hostId, pushing);
  assert.equal(await phone.message(), "h1");
  const cid = (await desk.message()).slice(1);
  const good = notification();
  const sentBefore = apple.requests.length;
  // Each with the id it is answered with: its own when it has one, so the desktop can match it.
  const wrong = [
    ["{nope", ""],
    ["[]", ""],
    ["null", ""],
    ['"text"', ""],
    [JSON.stringify({ ...good, id: undefined }), ""],
    [JSON.stringify({ ...good, id: 7 }), ""],
    [JSON.stringify({ ...good, id: "i".repeat(65) }), ""],
    // Too large to read at all.
    [JSON.stringify({ ...good, b: "b".repeat(9000) }), ""],
    ...[
      { to: undefined },
      { to: "a".repeat(63) },
      { to: "a".repeat(201) },
      { to: `${"a".repeat(63)}g` },
      { to: 12 },
      { env: "development" },
      { env: undefined },
      { type: "background" },
      { type: undefined },
      { collapse: "c".repeat(65) },
      { collapse: "a+b" },
      { collapse: "" },
      { collapse: 5 },
      { d: undefined },
      { d: "" },
      { d: "d".repeat(65) },
      { d: "a/b" },
      { b: undefined },
      { b: "" },
      { b: "b".repeat(3401) },
      { b: "QUJD==" },
      { ttl: -1 },
      { ttl: 86_401 },
      { ttl: 1.5 },
      { ttl: "60" },
      { sound: "yes" },
      { type: "liveactivity", priority: 1 },
      { type: "liveactivity", priority: "10" },
      { type: "liveactivity", event: "start" },
    ].map((change) => [JSON.stringify({ ...good, ...change }), good.id]),
  ];
  for (const [frame, id] of wrong) {
    desk.send(`p${frame}`);
    assert.deepEqual(
      await answer(desk),
      { id, ok: false, reason: "invalid", drop: false },
      frame.slice(0, 120),
    );
  }
  assert.equal(apple.requests.length, sentBefore, "an invalid notification reached Apple");
  // The socket is still there, and so is the browser on it.
  desk.send("ping");
  assert.equal(await desk.message(), "pong");
  desk.send(`m${cid} Estill here`);
  assert.equal(await phone.message(), "mEstill here");
  // A valid one goes to Apple. What a part may be at its longest, and a part left out as null.
  const longest = notification({
    id: "i".repeat(64),
    to: "A".repeat(200),
    collapse: "c".repeat(64),
    d: "d".repeat(64),
    b: "b".repeat(3400),
    ttl: 86_400,
    sound: null,
  });
  for (const asked of [good, longest]) {
    desk.send(p(asked));
    assert.deepEqual(await answer(desk), { id: asked.id, ok: true, status: 200, drop: false });
  }
  assert.equal(apple.requests.length, sentBefore + 2);
  assert.equal(apple.requests.at(-2).url, `https://api.push.apple.com/3/device/${good.to}`);
  assert.equal(apple.requests.at(-1).url, `https://api.push.apple.com/3/device/${"A".repeat(200)}`);
  assert.match(apple.requests.at(-1).body, /"sound":"default"/);
  await phone.quiet();
  desk.close();
});

test("a notification goes to Apple as Apple takes one, signed with the relay's one token", async () => {
  const key = id(32);
  const desk = await pushHost(idOf(key), key);
  const sent = async (fields) => {
    const asked = notification(fields);
    const from = Math.floor(Date.now() / 1000);
    desk.send(p(asked));
    assert.deepEqual(await answer(desk), { id: asked.id, ok: true, status: 200, drop: false });
    const request = apple.requests.at(-1);
    assert.equal(request.method, "POST");
    return {
      asked,
      request,
      headers: appleHeaders(request),
      from,
      to: Math.floor(Date.now() / 1000),
    };
  };
  const words = '"alert":{"title":"o7","body":"Something changed on your computer."}';

  // An alert, in production: Apple's fixed words, then what the desktop sealed. Never a category.
  const alert = await sent({ collapse: "Q2hhdDE", ttl: 3600 });
  assert.equal(alert.request.url, `https://api.push.apple.com/3/device/${alert.asked.to}`);
  const { authorization, "apns-expiration": expires, ...rest } = alert.headers;
  assert.deepEqual(rest, {
    "apns-topic": TOPIC,
    "apns-push-type": "alert",
    "apns-priority": "10",
    "apns-collapse-id": "Q2hhdDE",
    "content-type": "application/json",
  });
  within(Number(expires), alert.from + 3600, alert.to + 3600, "apns-expiration");
  assert.equal(
    alert.request.body,
    `{"aps":{${words},"sound":"default","mutable-content":1},"o7":{"v":1,"d":"${alert.asked.d}","b":"${alert.asked.b}"}}`,
  );
  // The token: ES256 with the push key's id, the team and when it was made, signed by the key.
  const token = await provider(authorization);
  assert.equal(token.header, `{"alg":"ES256","kid":"${KEY_ID}"}`);
  assert.equal(token.claims, `{"iss":"${TEAM_ID}","iat":${token.iat}}`);
  within(token.iat, started, alert.to, "iat");

  // An alert to a phone that runs a development build, without a sound. What only a Live
  // Activity takes changes nothing; Apple is told to try once, now. Each notification here goes
  // with the same token as the first: the relay made one.
  const quiet = await sent({ env: "sandbox", sound: false, priority: 5, event: "end" });
  assert.equal(quiet.request.url, `https://api.sandbox.push.apple.com/3/device/${quiet.asked.to}`);
  assert.deepEqual(quiet.headers, {
    authorization,
    "apns-topic": TOPIC,
    "apns-push-type": "alert",
    "apns-priority": "10",
    "apns-expiration": "0",
    "content-type": "application/json",
  });
  assert.equal(
    quiet.request.body,
    `{"aps":{${words},"mutable-content":1},"o7":{"v":1,"d":"${quiet.asked.d}","b":"${quiet.asked.b}"}}`,
  );

  // A Live Activity's update: its own topic and push type, and the sealed state. What only an
  // alert takes changes nothing.
  const update = await sent({ type: "liveactivity", collapse: "Q2hhdDE", sound: true });
  assert.equal(update.request.url, `https://api.push.apple.com/3/device/${update.asked.to}`);
  assert.deepEqual(update.headers, {
    authorization,
    "apns-topic": `${TOPIC}.push-type.liveactivity`,
    "apns-push-type": "liveactivity",
    "apns-priority": "10",
    "apns-expiration": "0",
    "content-type": "application/json",
  });
  const updated = JSON.parse(update.request.body).aps.timestamp;
  within(updated, update.from, update.to, "timestamp");
  assert.equal(
    update.request.body,
    `{"aps":{"timestamp":${updated},"event":"update","content-state":{"d":"${update.asked.d}","b":"${update.asked.b}"}}}`,
  );

  // Its end, at the lower priority: Apple takes it off the phone at once.
  const ended = await sent({
    type: "liveactivity",
    env: "sandbox",
    event: "end",
    priority: 5,
    ttl: 600,
  });
  assert.equal(ended.request.url, `https://api.sandbox.push.apple.com/3/device/${ended.asked.to}`);
  const { "apns-expiration": endExpires, ...endHeaders } = ended.headers;
  assert.deepEqual(endHeaders, {
    authorization,
    "apns-topic": `${TOPIC}.push-type.liveactivity`,
    "apns-push-type": "liveactivity",
    "apns-priority": "5",
    "content-type": "application/json",
  });
  within(Number(endExpires), ended.from + 600, ended.to + 600, "apns-expiration");
  const endedAt = JSON.parse(ended.request.body).aps.timestamp;
  within(endedAt, ended.from, ended.to, "timestamp");
  assert.equal(
    ended.request.body,
    `{"aps":{"timestamp":${endedAt},"event":"end","content-state":{"d":"${ended.asked.d}","b":"${ended.asked.b}"},"dismissal-date":${endedAt}}}`,
  );
  desk.close();
});

test("the desktop hears how Apple answered, and when to forget a phone's address", async () => {
  const key = id(32);
  const desk = await pushHost(idOf(key), key);
  const told = async (status, reason, text) => {
    apple.answer({ status, reason, text });
    const asked = notification();
    const sentBefore = apple.requests.length;
    desk.send(p(asked));
    const { id: answered, ...said } = await answer(desk);
    assert.equal(answered, asked.id);
    assert.equal(apple.requests.length, sentBefore + 1, `${status} ${reason}: sent more than once`);
    return said;
  };
  assert.deepEqual(await told(200), { ok: true, status: 200, drop: false });
  assert.deepEqual(await told(410, "Unregistered"), {
    ok: false,
    status: 410,
    reason: "Unregistered",
    drop: true,
  });
  for (const reason of ["BadDeviceToken", "DeviceTokenNotForTopic"])
    assert.deepEqual(await told(400, reason), { ok: false, status: 400, reason, drop: true });
  // Any other refusal keeps the address.
  assert.deepEqual(await told(400, "BadCollapseId"), {
    ok: false,
    status: 400,
    reason: "BadCollapseId",
    drop: false,
  });
  assert.deepEqual(await told(429, "TooManyRequests"), {
    ok: false,
    status: 429,
    reason: "TooManyRequests",
    drop: false,
  });
  assert.deepEqual(await told(503, undefined, "Service Unavailable"), {
    ok: false,
    status: 503,
    drop: false,
  });
  // A token Apple calls expired is not made again while it is young: Apple would refuse that.
  assert.deepEqual(await told(403, "ExpiredProviderToken"), {
    ok: false,
    status: 403,
    reason: "ExpiredProviderToken",
    drop: false,
  });
  // Every notification so far went with one token.
  assert.equal(new Set(apple.requests.map((request) => request.headers.authorization)).size, 1);
  desk.close();
});

test("a desktop is held to its limits, and what is over them goes nowhere", async () => {
  const room = async () => {
    const key = id(32);
    return pushHost(idOf(key), key);
  };
  const delivered = async (desk, list) => {
    for (const asked of list) desk.send(p(asked));
    const said = await answers(desk, list.length);
    for (const asked of list)
      assert.deepEqual(said.get(asked.id), { id: asked.id, ok: true, status: 200, drop: false });
  };
  const limited = async (desk, asked) => {
    const sentBefore = apple.requests.length;
    desk.send(p(asked));
    assert.deepEqual(await answer(desk), {
      id: asked.id,
      ok: false,
      reason: "limited",
      drop: false,
    });
    await desk.quiet(100);
    assert.equal(apple.requests.length, sentBefore, "a notification over a limit reached Apple");
  };

  // Thirty a minute to one phone, however its address is written.
  const one = await room();
  const phone = deviceToken();
  await delivered(
    one,
    Array.from({ length: 30 }, () => notification({ to: phone })),
  );
  await limited(one, notification({ to: phone }));
  await limited(one, notification({ to: phone.toUpperCase() }));
  await delivered(one, [notification()]);
  one.close();

  // Sixty a minute from one computer.
  const busy = await room();
  const three = [deviceToken(), deviceToken(), deviceToken()];
  const sixty = Array.from({ length: 60 }, (_, at) => notification({ to: three[at % 3] }));
  await delivered(busy, sixty);
  await limited(busy, notification({ to: three[0] }));
  await limited(busy, notification());
  // Another computer has limits of its own.
  const other = await room();
  await delivered(other, [notification({ to: three[0] })]);
  other.close();
  busy.close();

  // Thirty-two phones an hour; one already sent to this hour still hears.
  const many = await room();
  const phones = Array.from({ length: 32 }, deviceToken);
  await delivered(
    many,
    phones.map((to) => notification({ to })),
  );
  await limited(many, notification());
  await delivered(many, [notification({ to: phones[0] })]);
  many.close();
});

test("the relay makes one token at a time, and a new one when Apple calls it expired", async () => {
  const played = appleMock();
  // A relay of its own, where a token Apple refused may be made again at once, and where Apple
  // can be made unreachable.
  const relay = await withKey({ node: played.node }, { APNS_RENEW_MS: "1" });
  try {
    const key = id(32);
    const desk = await pushHost(idOf(key), key, relay);
    const tokenOf = (request) => request.headers.authorization;

    // Three at once, before there is any token: all three wait for the same one.
    const first = Array.from({ length: 3 }, () => notification());
    for (const asked of first) desk.send(p(asked));
    const said = await answers(desk, 3);
    for (const asked of first) assert.equal(said.get(asked.id)?.ok, true, asked.id);
    assert.equal(played.requests.length, 3);
    const made = tokenOf(played.requests[0]);
    assert.deepEqual(played.requests.map(tokenOf), [made, made, made]);
    const old = await provider(made);

    // Apple calls it expired: another is made, and the notification goes once more with it.
    played.answer({ status: 403, reason: "ExpiredProviderToken" });
    const again = notification();
    desk.send(p(again));
    assert.deepEqual(await answer(desk), { id: again.id, ok: true, status: 200, drop: false });
    assert.equal(played.requests.length, 5);
    const [refused, retried] = played.requests.slice(3);
    assert.equal(tokenOf(refused), made);
    assert.notEqual(tokenOf(retried), made);
    assert.equal(retried.body, refused.body);
    const renewed = await provider(tokenOf(retried));
    assert.equal(renewed.header, old.header);
    assert.equal(renewed.claims, `{"iss":"${TEAM_ID}","iat":${renewed.iat}}`);
    assert.ok(renewed.iat >= old.iat);
    // The new one is kept for what follows.
    const next = notification();
    desk.send(p(next));
    assert.equal((await answer(desk)).ok, true);
    assert.equal(tokenOf(played.requests[5]), tokenOf(retried));

    // Once only: refused again with a new token, the desktop hears what Apple said.
    played.answer(
      { status: 403, reason: "InvalidProviderToken" },
      { status: 403, reason: "InvalidProviderToken" },
    );
    const twice = notification();
    desk.send(p(twice));
    assert.deepEqual(await answer(desk), {
      id: twice.id,
      ok: false,
      status: 403,
      reason: "InvalidProviderToken",
      drop: false,
    });
    assert.equal(played.requests.length, 8);
    assert.notEqual(tokenOf(played.requests[7]), tokenOf(played.requests[6]));

    // A network that fails: Apple is unreachable.
    played.answer("lost");
    const lost = notification();
    desk.send(p(lost));
    assert.deepEqual(await answer(desk), {
      id: lost.id,
      ok: false,
      reason: "unreachable",
      drop: false,
    });
    desk.close();
  } finally {
    await relay.dispose();
  }
});

test("a room that starts again, as after it slept, keeps its limits and the relay its token", async () => {
  // What the relay stores goes to a folder, so a second runtime starts from it, memory empty.
  const folder = await mkdtemp(join(tmpdir(), "o7-relay-stored-"));
  const played = appleMock();
  const key = id(32);
  const phone = deviceToken();
  const run = async (work) => {
    const relay = await withKey(played.fetch, {}, { resourcePersistencePath: folder });
    try {
      const desk = await pushHost(idOf(key), key, relay);
      await work(desk);
      desk.close();
    } finally {
      await relay.dispose();
    }
  };
  try {
    await run(async (desk) => {
      const thirty = Array.from({ length: 30 }, () => notification({ to: phone }));
      for (const asked of thirty) desk.send(p(asked));
      const said = await answers(desk, 30);
      for (const asked of thirty) assert.equal(said.get(asked.id)?.ok, true, asked.id);
    });
    await run(async (desk) => {
      const over = notification({ to: phone });
      desk.send(p(over));
      assert.deepEqual(await answer(desk), {
        id: over.id,
        ok: false,
        reason: "limited",
        drop: false,
      });
      const other = notification();
      desk.send(p(other));
      assert.equal((await answer(desk)).ok, true);
    });
    assert.equal(played.requests.length, 31);
    assert.equal(
      new Set(played.requests.map((request) => request.headers.authorization)).size,
      1,
      "the relay made another token when it started again",
    );
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
});
