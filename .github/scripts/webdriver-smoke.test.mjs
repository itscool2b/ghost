import assert from "node:assert/strict";
import { createServer } from "node:http";
import { join, resolve } from "node:path";
import test from "node:test";
import { until, verifyRuntime, WebDriver } from "./webdriver-client.mjs";
import { exerciseApp, isolatedEnvironment } from "./webdriver-smoke.mjs";

async function endpoint(t, handler) {
  const received = [];
  const server = createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    const call = {
      method: request.method,
      path: request.url,
      body: body ? JSON.parse(body) : null,
    };
    received.push(call);
    const answer = handler(call);
    response.writeHead(answer.status ?? 200, { "content-type": "application/json" });
    response.end(JSON.stringify({ value: answer.value }));
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  t.after(() => new Promise((done) => server.close(done)));
  return { driver: new WebDriver(`http://127.0.0.1:${server.address().port}`), received };
}

test("creates a real application capability and types through W3C element commands", async (t) => {
  const { driver, received } = await endpoint(t, ({ path }) => {
    if (path === "/session") return { value: { sessionId: "native id", capabilities: {} } };
    if (path.endsWith("/elements"))
      return { value: [{ "element-6066-11e4-a52e-4f735466cecf": "input" }] };
    if (path.endsWith("/displayed")) return { value: true };
    if (path.endsWith("/property/value")) return { value: "/echo hello" };
    return { value: null };
  });
  await driver.open("C:\\owned\\ghost.exe");
  assert.deepEqual(driver.capabilities, {});
  await driver.type('[data-testid="composer-input"]', "/echo hello");
  await driver.close();
  assert.deepEqual(received[0].body.capabilities.alwaysMatch["tauri:options"], {
    application: "C:\\owned\\ghost.exe",
    args: [],
  });
  assert.deepEqual(received.find((call) => call.path.endsWith("/value")).body, {
    text: "/echo hello",
  });
  assert(received.some((call) => call.path === "/session/native%20id/element/input/clear"));
  assert.equal(received.at(-1).method, "DELETE");
  assert.equal(driver.session, null);
  assert(!received.some((call) => call.path.includes("execute")), "typing must not inject events");
});

test("protocol failures are not mistaken for missing UI, and malformed sessions fail", async (t) => {
  const { driver } = await endpoint(t, ({ path }) => ({
    status: path === "/session" ? 200 : 500,
    value: path === "/session" ? {} : { error: "unknown error", message: "driver failed" },
  }));
  await assert.rejects(driver.open("C:\\owned\\ghost.exe"), /session was not created/);
  await assert.rejects(driver.request("GET", "/status"), /driver failed/);
  await assert.rejects(
    until("broken transport", () => driver.request("GET", "/status")),
    /driver failed/,
  );
  assert.throws(() => new WebDriver("https://external.example"));
});

test("polling tolerates a replaced DOM node but has a deadline", async () => {
  let attempts = 0;
  assert.equal(
    await until(
      "rerender",
      async () => {
        attempts++;
        if (attempts === 1)
          throw Object.assign(new Error("old node"), { code: "stale element reference" });
        return "new node";
      },
      { timeout: 100, interval: 1 },
    ),
    "new node",
  );
  await assert.rejects(
    until("never", async () => false, { timeout: 2, interval: 1 }),
    /Timed out/,
  );
});

test("runtime qualification requires the same major, minor and build", () => {
  assert.equal(
    verifyRuntime({ browserVersion: "140.0.3485.54" }, "140.0.3485.81"),
    "140.0.3485.54",
  );
  assert.throws(() => verifyRuntime({ browserVersion: "140.0.3484.54" }, "140.0.3485.81"));
  assert.throws(() => verifyRuntime({}, "140.0.3485.81"));
  assert.throws(() => verifyRuntime({ browserVersion: "140.0.3485.54" }, undefined));
});

test("the child cannot inherit a real profile, embedded smoke or agent configuration override", () => {
  const root = resolve("synthetic-owned-data");
  const source = {
    Path: "system-path",
    SystemRoot: "C:\\Windows",
    UserProfile: "real-profile",
    GHOST_DATA_DIR: "real-data",
    GHOST_SMOKE: "chat",
    GHOST_SMOKE_AGENT: "claude",
    GHOST_STRESS: "1",
    GHOST_EXPORT_BINDINGS: "1",
    WEBVIEW2_BROWSER_EXECUTABLE_FOLDER: "other-runtime",
    WEBVIEW2_USER_DATA_FOLDER: "real-webview",
    CODEX_HOME: "real-codex",
    CLAUDE_CONFIG_DIR: "real-claude",
    PI_CODING_AGENT_DIR: "real-pi",
  };
  const env = isolatedEnvironment(source, root);
  assert.equal(env.GHOST_DATA_DIR, join(root, "data"));
  // Edge WebDriver gives the WebView its own fresh profile and reads its debugging port there.
  assert.equal(env.WEBVIEW2_USER_DATA_FOLDER, undefined);
  assert.equal(env.USERPROFILE, join(root, "home"));
  assert.equal(env.CODEX_HOME, join(root, "home", ".codex"));
  assert.equal(env.CLAUDE_CONFIG_DIR, join(root, "home", ".claude"));
  assert.equal(env.PI_CODING_AGENT_DIR, join(root, "home", ".pi"));
  for (const key of [
    "UserProfile",
    "GHOST_SMOKE",
    "GHOST_SMOKE_AGENT",
    "GHOST_STRESS",
    "GHOST_EXPORT_BINDINGS",
    "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
  ])
    assert.equal(env[key], undefined);
  assert.equal(env.SystemRoot, source.SystemRoot);
  assert.equal(source.GHOST_DATA_DIR, "real-data", "parent environment is unchanged");
});

function scenarioDriver() {
  const calls = [];
  let demoSelected = false;
  const driver = {
    async command(method, path, body) {
      calls.push({ method, path, body });
      if (path === "/window/handles") return ["native-window"];
      if (path === "/url") return "http://tauri.localhost/";
      return null;
    },
    async script(script, args, asynchronous) {
      calls.push({ script, args, asynchronous });
      if (asynchronous) return { value: { folders: [{ id: "owned-folder" }] } };
      return true;
    },
    async click(selector, using) {
      calls.push({ click: selector, using });
      if (selector.includes('[data-testid="new-chat-agent"]')) {
        assert.ok(
          [
            '[data-testid="new-chat-agent"] details > summary',
            '[data-testid="new-chat-agent"] details label:has(input[type="radio"][value="mock"])',
          ].includes(selector),
          `the agent menu opens, and its choices are radio labels, not select options: ${selector}`,
        );
        if (selector.includes("label:has")) demoSelected = true;
      }
    },
    async scroll(selector, deltaY) {
      calls.push({ scroll: selector, deltaY });
    },
    async visible(selector) {
      return selector;
    },
    async type(selector, text) {
      calls.push({ type: selector, text });
    },
    async elements(selector) {
      if (selector === '[data-testid="new-chat-agent"] input[type="radio"][value="mock"]')
        return ["demo-radio"];
      return ["echo-prompt", "slow-prompt"];
    },
    async element(id, _method, action) {
      if (id === "demo-radio" && action === "property/checked") return demoSelected;
      if (id.includes("prompt-text")) return "/echo External native WebDriver hello";
      return "External native WebDriver hello";
    },
  };
  return { driver, calls };
}

test("the scenario drives create, typing, Stop and reload externally; only folder setup uses IPC", async () => {
  const { driver, calls } = scenarioDriver();
  const evidence = [];
  const checks = await exerciseApp(driver, "C:\\disposable project", async (name) =>
    evidence.push(name),
  );
  assert.equal(checks.length, 5);
  const native = calls.filter((call) => call.asynchronous);
  assert.equal(native.length, 1);
  assert.match(native[0].script, /invoke\('project_create'/);
  assert.deepEqual(native[0].args, ["C:\\disposable project"]);
  assert.deepEqual(
    calls.filter((call) => call.type).map((call) => call.text),
    ["/echo External native WebDriver hello", "/slow"],
  );
  const clicks = calls.filter((call) => call.click).map((call) => call.click);
  assert(
    clicks.indexOf('[data-testid="new-chat-button"]') <
      clicks.findIndex((click) => click.startsWith('[data-testid="new-chat-agent"]')),
    "New chat opens from the start screen before an agent is chosen",
  );
  assert(clicks.includes('[data-testid="new-chat-button"]'));
  const menu = clicks.indexOf('[data-testid="new-chat-agent"] details > summary');
  assert(
    menu >= 0 && menu < clicks.findIndex((click) => click.includes("label:has")),
    "the agent menu opens before the Demo agent is chosen",
  );
  assert(calls.some((call) => call.click === '[data-testid="composer-stop"]'));
  assert(calls.some((call) => call.click === '[data-testid="chat-stop-session"]'));
  assert.equal(calls.filter((call) => call.path === "/refresh").length, 2);
  assert(
    !calls.some((call) => call.path === "/window/rect"),
    "resize needs advertised driver support",
  );
  assert.deepEqual(evidence, ["echo", "stopped", "reopened"]);
  assert(!calls.some((call) => /smoke_report|chat_send|runSmoke/.test(call.script ?? "")));
});

test("a failed create check retains earlier evidence and cannot report success", async () => {
  const { driver } = scenarioDriver();
  driver.type = async () => {
    throw new Error("native typing failed");
  };
  const checks = [];
  await assert.rejects(
    exerciseApp(driver, "owned", async () => {}, checks),
    /native typing failed/,
  );
  assert.deepEqual(checks, ["native-window", "create-demo-chat"]);
});

test("an unselected Demo radio stops before chat creation and retains native-window evidence", async () => {
  const { driver, calls } = scenarioDriver();
  const element = driver.element;
  driver.element = async (id, method, action) =>
    id === "demo-radio" && action === "property/checked" ? false : element(id, method, action);
  const checks = [];
  await assert.rejects(
    exerciseApp(driver, "owned", async () => {}, checks),
    /Demo radio must be selected/,
  );
  assert.deepEqual(checks, ["native-window"]);
  assert(!calls.some((call) => call.click === '[data-testid="new-chat-create"]'));
});

test("resizes the native window only when its driver advertises support", async () => {
  const { driver, calls } = scenarioDriver();
  driver.capabilities = { setWindowRect: true };
  await exerciseApp(driver, "owned", async () => {});
  assert.deepEqual(calls.find((call) => call.path === "/window/rect").body, {
    width: 1200,
    height: 900,
  });
});
