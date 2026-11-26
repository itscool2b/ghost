/**
 * Local native check (not part of any suite): the real debug app, in a data folder of its own,
 * reached through a relay on this machine from a separate browser and from an MCP client.
 *
 *   node apps/relay/tests/native.local.mjs <ghost.exe> [--prompt]
 *
 * `--prompt` also sends one short message to a real agent. Everything else starts no turn.
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { convertV4MiniflareOptions, Miniflare } from "miniflare";

const here = fileURLToPath(new URL(".", import.meta.url));
const [exe, ...flags] = process.argv.slice(2);
const prompt = flags.includes("--prompt");
const { chromium } = createRequire(resolve(here, "../../web/package.json"))("playwright");
const SITE_PORT = 1438;
const RELAY_PORT = 8798;
const CDP_PORT = 9347;
const SITE = `http://localhost:${SITE_PORT}`;
const shots = resolve(here, "../../../target/remote-native");

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
  };
  const server = createServer(async (request, response) => {
    const path = normalize(decodeURIComponent(new URL(request.url, SITE).pathname));
    for (const candidate of [path, join(path, "index.html")]) {
      const file = join(root, candidate);
      try {
        if (file.startsWith(root) && (await stat(file)).isFile()) {
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

const data = await mkdtemp(join(tmpdir(), "ghost-remote-native-data-"));
const project = await mkdtemp(join(tmpdir(), "ghost-remote-native-project-"));
await writeFile(join(project, "README.md"), "# Remote check\n");
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

// A clean environment: nothing of the chat this runs from reaches the app under test.
const env = Object.fromEntries(
  Object.entries(process.env).filter(
    ([key]) => !/^(CLAUDECODE$|CLAUDE_CODE_|GHOST_MCP_)/.test(key),
  ),
);
const app = spawn(exe, [], {
  env: {
    ...env,
    GHOST_DATA_DIR: data,
    GHOST_REMOTE_RELAY: `ws://127.0.0.1:${RELAY_PORT}`,
    GHOST_REMOTE_SITE: SITE,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${CDP_PORT}`,
  },
  stdio: "ignore",
});
console.log(`app pid ${app.pid}`);
let desktop;
let browser;
try {
  // The app's own webview, over DevTools.
  for (let tries = 0; ; tries += 1) {
    try {
      desktop = await chromium.connectOverCDP(`http://127.0.0.1:${CDP_PORT}`);
      break;
    } catch (error) {
      if (tries > 60) throw error;
      await new Promise((done) => setTimeout(done, 500));
    }
  }
  let page;
  for (let tries = 0; !page && tries < 60; tries += 1) {
    page = desktop
      .contexts()
      .flatMap((context) => context.pages())
      .find((candidate) => /tauri\.localhost|localhost:1420/.test(candidate.url()));
    if (!page) await new Promise((done) => setTimeout(done, 500));
  }
  assert.ok(page, "the app's page");
  await page.waitForFunction(() => window.__TAURI_INTERNALS__ !== undefined);
  // A command's failure is an object ({ code, message }); carry its words across.
  const invoke = async (command, args = {}) => {
    const result = await page.evaluate(
      ([name, input]) =>
        window.__TAURI_INTERNALS__.invoke(name, input).then(
          (value) => ({ value }),
          (error) => ({ error: typeof error === "string" ? error : JSON.stringify(error) }),
        ),
      [command, args],
    );
    if ("error" in result) throw new Error(result.error);
    return result.value;
  };
  const skip = page.getByRole("button", { name: "Skip setup" });
  if (await skip.count()) await skip.click();

  // Remote access is off until asked for, and nothing has connected.
  const before = await invoke("remote_status");
  assert.equal(before.enabled, false);
  assert.deepEqual(before.link, { state: "off" });
  assert.deepEqual([before.devices, before.bots], [[], []]);
  await assert.rejects(invoke("remote_pair"), /Turn remote access on/);

  const on = await invoke("remote_set_enabled", { enabled: true });
  assert.equal(on.enabled, true);
  let status = on;
  for (let tries = 0; status.link.state !== "online" && tries < 40; tries += 1) {
    await new Promise((done) => setTimeout(done, 250));
    status = await invoke("remote_status");
  }
  assert.deepEqual(status.link, { state: "online" }, JSON.stringify(status.link));
  console.log(`host "${status.host_name}" is online at the local relay`);

  // A real project and chat in the app. Creating a chat starts its agent; it sends no prompt.
  const agents = await invoke("agents_list");
  const ready = agents.filter((agent) => agent.availability.state === "ready");
  // A development build has a scripted agent with no AI behind it: a real turn that costs nothing.
  const agent = ready.find((entry) => entry.id === "mock") ?? ready[0];
  assert.ok(!prompt || agent.id === "mock", "a prompt is only sent to the scripted agent");
  assert.ok(agent, "an agent is ready on this machine");
  const created = await invoke("chat_create", { agentId: agent.id, folder: project });
  console.log(`created chat ${created.id} with ${agent.name}`);

  // The Settings page, in the real window.
  await page
    .getByRole("button", { name: "Settings" })
    .first()
    .click()
    .catch(() => {});
  const nav = page.getByRole("navigation", { name: "Settings sections" });
  if (await nav.count()) {
    await nav.getByRole("button", { name: "Remote access" }).click();
    await page
      .getByText("On. This computer can be reached by the devices and bots below.")
      .waitFor();
    await page.screenshot({ path: join(shots, "settings-remote.png") });
  }

  // Pair a separate browser through the relay.
  const pairing = await invoke("remote_pair");
  assert.match(pairing.url, /^http:\/\/localhost:1438\/remote#/);
  browser = await chromium.launch();
  const phone = await browser.newContext({
    viewport: { width: 412, height: 915 },
    deviceScaleFactor: 2,
  });
  const tab = await phone.newPage();
  const errors = [];
  tab.on("pageerror", (error) => errors.push(error.message));
  await tab.goto(pairing.url.replace("/remote#", `/remote?relay=ws://127.0.0.1:${RELAY_PORT}#`));
  await tab.getByRole("button", { name: "Pair this browser", exact: true }).click();
  await tab.getByRole("heading", { level: 1, name: status.host_name }).waitFor();
  await tab.getByText("Connected", { exact: true }).waitFor();
  const paired = await invoke("remote_status");
  assert.equal(paired.devices.length, 1);
  assert.equal(paired.devices[0].connected, true);
  assert.equal(paired.pairing_expires_at, null);
  console.log(`paired "${paired.devices[0].name}"`);
  const row = tab.locator(".remote-chat-row").first();
  await row.waitFor();
  console.log(`remote list shows: ${(await row.textContent()).replace(/\s+/g, " ")}`);
  await tab.screenshot({ path: join(shots, "phone-list.png") });
  await row.click();
  await tab.getByRole("textbox", { name: "Message" }).waitFor();
  await tab.getByText("Nothing here yet. Send a message.").waitFor();

  // A bot link, over MCP.
  const link = await invoke("remote_create_bot", { name: "Check bot", full: false });
  const local = (url) => url.replace(SITE, `http://127.0.0.1:${RELAY_PORT}`);
  const mcp = local(link.url);
  const ask = async (address, name, args = {}) => {
    for (let tries = 0; ; tries += 1) {
      const response = await fetch(address, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "tools/call",
          params: { name, arguments: args },
        }),
      });
      if (response.status === 404 && tries < 30) {
        await new Promise((done) => setTimeout(done, 100));
        continue;
      }
      assert.equal(response.status, 200);
      return response.json();
    }
  };
  const call = async (name, args = {}) => (await ask(mcp, name, args)).result.content[0].text;
  const listed = await call("chats");
  console.log(listed.trim());
  assert.match(listed, new RegExp(`\\n${created.id} \\| `));
  const projects = await call("projects");
  console.log(projects.trim());
  assert.match(projects, /^Project \S+ "/);
  assert.match(projects, /\n {2}folder \S+ /);
  assert.match(await call("read", { chat: created.id }), /No turns yet\./);

  // A chat link has no tool for the rest of the app; a full control link does.
  const refused = await ask(mcp, "set", { chat: created.id });
  const refusal = JSON.stringify(refused);
  assert.ok(
    refused.error || refused.result?.isError,
    `a chat link reached a full control tool: ${refusal.slice(0, 200)}`,
  );
  console.log(`a chat link asking for set: ${refusal.slice(0, 160)}`);
  const wide = await invoke("remote_create_bot", { name: "Full bot", full: true });
  const options = await ask(local(wide.url), "set", { chat: created.id });
  const shown = options.result.content[0].text;
  assert.ok(!options.result.isError, shown);
  assert.match(shown, /^model: /);
  console.log(`a full control link asking for set: ${shown.trim().slice(0, 300)}`);

  if (prompt) {
    // One short message to the real agent, sent from the phone.
    await tab.getByRole("textbox", { name: "Message" }).fill("Say hello from the remote page");
    await tab.getByRole("button", { name: "Send" }).click();
    await tab.locator(".remote-user", { hasText: "Say hello from the remote page" }).waitFor();
    // Whatever the agent asks along the way is answered from the phone.
    const card = tab.locator(".remote-card");
    const answer = tab.locator(".remote-markdown").first();
    for (let waited = 0; waited < 120; waited += 1) {
      if (await card.count()) {
        const asked = (await card.first().textContent()).replace(/\s+/g, " ").slice(0, 160);
        console.log(`the agent asks: ${asked}`);
        await card.first().getByRole("button").first().click();
        await new Promise((done) => setTimeout(done, 400));
        continue;
      }
      const state = (await tab.locator(".remote-link").textContent()) ?? "";
      if ((await answer.count()) && /Idle|Stopped/.test(state)) break;
      await new Promise((done) => setTimeout(done, 500));
    }
    await answer.waitFor();
    await tab.screenshot({ path: join(shots, "phone-chat.png") });
    console.log(
      `the agent said: ${(await answer.textContent()).replace(/\s+/g, " ").slice(0, 200)}`,
    );
    // The app's own record of the chat has the same turn.
    const history = await invoke("chat_history", {
      chatId: created.id,
      beforeSeq: 4294967295,
      maxBytes: null,
    });
    const said = history.items
      .filter((item) => item.event.type === "message_delta")
      .map((item) => item.event.text)
      .join("");
    assert.ok(said.length > 0, "the app recorded the agent's words");
    assert.ok(
      history.items.some(
        (item) =>
          item.event.type === "user_message_delta" &&
          item.event.text.includes("Say hello from the remote page"),
      ),
      "the app recorded the message sent from the phone",
    );
    const read = await call("wait", { chat: created.id, seconds: 30 });
    console.log(read.trim());
    assert.match(read, /User: Say hello from the remote page/);
    assert.match(read, /Agent: /);
  }

  // The phone's pickers against the real app: the chat's own models, one chosen, and what the
  // app then reports as current, read back through a full control link.
  await tab.getByRole("button", { name: "Model and agent" }).click();
  const pickers = tab.getByRole("dialog", { name: "Model and agent" });
  const model = pickers.getByRole("combobox", { name: "Model" });
  await model.waitFor();
  const offered = await model
    .locator("option")
    .evaluateAll((options) =>
      options.map((option) => ({ value: option.value, name: option.textContent })),
    );
  const current = await model.inputValue();
  console.log(
    `the phone's model picker: ${offered.map((entry) => `${entry.name}${entry.value === current ? " [current]" : ""}`).join(" | ")}`,
  );
  const wanted = offered.find((entry) => entry.value && entry.value !== current);
  assert.ok(wanted, "the demo agent offers more than one model");
  await model.selectOption(wanted.value);
  await pickers.getByRole("status").getByText(`Model set to ${wanted.name}.`).waitFor();
  assert.equal(await model.inputValue(), wanted.value, "the app did not keep the model");
  const agentChoices = await pickers
    .getByRole("combobox", { name: "Agent" })
    .locator("option")
    .allTextContents();
  console.log(`the phone's agent picker: ${agentChoices.join(" | ")}`);
  await tab.screenshot({ path: join(shots, "phone-pickers.png") });
  await pickers.getByRole("button", { name: "Done" }).click();
  await pickers.waitFor({ state: "detached" });
  const after = (await ask(local(wide.url), "set", { chat: created.id })).result.content[0].text;
  const models = after.split("\n").find((line) => line.startsWith("model: ")) ?? "";
  assert.ok(models.includes(`${wanted.value} (current)`), models);
  console.log(`the app now reports: ${models}`);

  // Removing the device ends the phone's session at once.
  await invoke("remote_remove_device", { id: paired.devices[0].id });
  await tab.getByRole("heading", { name: "This pairing is over." }).waitFor();
  await invoke("remote_remove_bot", { id: link.id });
  for (let tries = 0; ; tries += 1) {
    const gone = await fetch(mcp, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "ping" }),
    });
    if (gone.status === 404) break;
    assert.ok(tries < 30, "the removed bot link still answered");
    await new Promise((done) => setTimeout(done, 100));
  }
  const off = await invoke("remote_set_enabled", { enabled: false });
  assert.deepEqual(off.link, { state: "off" });
  await invoke("chat_stop", { chatId: created.id }).catch(() => {});
  assert.deepEqual(errors, []);
  console.log("native remote check passed");
} finally {
  await browser?.close().catch(() => {});
  await desktop?.close().catch(() => {});
  app.kill();
  await new Promise((done) => setTimeout(done, 1500));
  await relay.dispose();
  site.close();
  await rm(project, { recursive: true, force: true }).catch(() => {});
  await rm(data, { recursive: true, force: true }).catch(() => {});
}
