// Windows-only external full-app gate. Build the frontend into the native binary first.
// Setup invokes only project_create for our disposable folder. All chat input and assertions use
// W3C WebDriver endpoints against the actual desktop window, never an embedded smoke report.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import { until, verifyRuntime, WebDriver } from "./webdriver-client.mjs";

export function isolatedEnvironment(source, root) {
  assert(isAbsolute(root));
  const env = { ...source };
  for (const name of Object.keys(env)) {
    if (
      /^(GHOST_|O7_|AURACODE_|WEBVIEW2_|TAURI_|CLAUDE_CONFIG_DIR$|CODEX_HOME$|PI_CODING_AGENT_DIR$|HOME$|USERPROFILE$|APPDATA$|LOCALAPPDATA$|XDG_CONFIG_HOME$)/i.test(
        name,
      )
    )
      delete env[name];
  }
  return {
    ...env,
    GHOST_DATA_DIR: join(root, "data"),
    HOME: join(root, "home"),
    USERPROFILE: join(root, "home"),
    APPDATA: join(root, "home", "AppData", "Roaming"),
    LOCALAPPDATA: join(root, "home", "AppData", "Local"),
    XDG_CONFIG_HOME: join(root, "home", ".config"),
    CLAUDE_CONFIG_DIR: join(root, "home", ".claude"),
    CODEX_HOME: join(root, "home", ".codex"),
    PI_CODING_AGENT_DIR: join(root, "home", ".pi"),
    CI: "true",
  };
}

const byTest = (name) => `[data-testid="${name}"]`;
const ECHO = "External native WebDriver hello";
const PROMPT = `/echo ${ECHO}`;

export async function exerciseApp(driver, folder, save, checks = []) {
  const passed = (name) => {
    checks.push(name);
    console.log(`PASS ${name}`);
  };
  const windows = await driver.command("GET", "/window/handles");
  assert.equal(windows.length, 1, "expected exactly one native app window");
  await driver.command("POST", "/window", { handle: windows[0] });
  if (driver.capabilities?.setWindowRect === true)
    await driver.command("POST", "/window/rect", { width: 1200, height: 900 });
  await until("visible native document", () =>
    driver.script(
      "return document.visibilityState === 'visible' && innerWidth > 0 && innerHeight > 0 && !!window.__TAURI_INTERNALS__",
    ),
  );
  const url = await driver.command("GET", "/url");
  assert.match(
    url,
    /^(https?:\/\/tauri\.localhost|tauri:\/\/localhost)(\/|$)/,
    "use an embedded build, not Vite",
  );
  passed("native-window");

  // Real native project validation and persistence, but only within the test's freshly created path.
  const project = await driver.script(
    `const done = arguments[arguments.length - 1];
     window.__TAURI_INTERNALS__.invoke('project_create', {request: {
       project: {name: 'WebDriver project', color: null, default_agent: 'mock', default_model: null},
       folder: arguments[0]
     }}).then(value => done({value}), error => done({error: JSON.stringify(error)}));`,
    [folder],
    true,
  );
  assert(!project.error, `disposable project setup failed: ${project.error}`);
  const folderId = project.value?.folders?.[0]?.id;
  assert.match(folderId ?? "", /^[a-zA-Z0-9-]+$/);
  await driver.command("POST", "/refresh", {});
  await until("setup intro finished", () =>
    driver.script(
      "return !!document.querySelector('.setup-shell') && !document.querySelector('[data-cinematic]')",
    ),
  );
  await driver.click("//button[normalize-space(.)='Skip setup']", "xpath");
  // Launch shows the start screen's aircraft; the sidebar's New chat opens the form.
  await until("start screen", () => driver.visible(byTest("launch-art")));
  await driver.click(byTest("new-chat-button"));
  await until("new chat form", () => driver.visible(byTest("new-chat")));
  // The folder is the app's own dropdown (apps/desktop/src/components/Select.tsx), not a native
  // select: open the field, then press the folder's row. Its list carries the field's name too.
  await driver.click('[role="combobox"][aria-label="Project folder"]');
  await driver.click(`[role="listbox"] [role="option"][data-value="${folderId}"]`);
  // The agent is a menu: open it and choose the Demo agent. A new choice closes the menu (it
  // animates out, marked data-closing); choosing the agent already chosen changes nothing and
  // leaves it open, so close it then. A second click on a closing menu would keep it open.
  const picker = `${byTest("new-chat-agent")} details`;
  const demo = 'input[type="radio"][value="mock"]';
  await driver.click(`${picker} > summary`);
  await driver.click(`${picker} label:has(${demo})`);
  const lingering =
    "const menu = document.querySelector(arguments[0]); return !!menu && menu.open && !menu.hasAttribute('data-closing');";
  if (await driver.script(lingering, [picker])) await driver.click(`${picker} > summary`);
  await until("agent menu closed", () =>
    driver.script("return !document.querySelector(arguments[0])?.open", [picker]),
  );
  const radios = await driver.elements(`${byTest("new-chat-agent")} ${demo}`);
  assert.equal(radios.length, 1, "expected exactly one Demo radio choice");
  assert.equal(
    await driver.element(radios[0], "GET", "property/checked"),
    true,
    "Demo radio must be selected before creating the chat",
  );
  await driver.click(byTest("new-chat-create"));
  await until("ready demo session", () =>
    driver.visible(`${byTest("chat-status")}[data-test-status="ready"]`),
  );
  passed("create-demo-chat");

  await driver.type(byTest("composer-input"), PROMPT);
  await driver.click(byTest("composer-send"));
  await assertTranscript(driver, PROMPT, ECHO, "end_turn");
  await save("echo");
  passed("typed-prompt-visible-reply");

  // The echo turn's receipt can render before the session reports idle; a prompt sent in that gap
  // is queued, and a stop then holds it. Send once the session is ready, stop once the turn runs.
  await until("ready session after the echo turn", () =>
    driver.visible(`${byTest("chat-status")}[data-test-status="ready"]`),
  );
  await driver.type(byTest("composer-input"), "/slow");
  await driver.click(byTest("composer-send"));
  await until("running slow turn", () => driver.visible('[data-test-prompt="/slow"]'));
  await driver.click(byTest("composer-stop"));
  await until("cancelled turn", () =>
    driver.visible('[data-test-prompt="/slow"][data-test-stop-reason="cancelled"]'),
  );
  passed("interrupt-active-turn");
  await driver.click('summary[aria-label="Chat details and controls"]');
  await driver.click(byTest("chat-stop-session"));
  await until("stopped demo session", () =>
    driver.visible(`${byTest("chat-status")}[data-test-status="stopped"]`),
  );
  await save("stopped");

  await driver.command("POST", "/refresh", {});
  await driver.click(byTest("chat-row"));
  await until("reopened stopped chat", () =>
    driver.visible(`${byTest("chat-status")}[data-test-status="stopped"]`),
  );
  await driver.scroll(byTest("transcript"), -2000);
  await assertTranscript(driver, PROMPT, ECHO, "end_turn");
  const prompts = await driver.elements(byTest("prompt-text"));
  assert.equal(prompts.length, 2, "reload must retain both turns without sending another prompt");
  await save("reopened");
  passed("reload-retains-history-without-starting-agent");
  return checks;
}

async function assertTranscript(driver, prompt, answer, reason) {
  await until("visible completed echo", async () => {
    const user = await driver.visible(byTest("prompt-text"));
    const message = await driver.visible('[data-testid="item"][data-test-kind="message"]');
    const ended = await driver.visible(`[data-test-stop-reason="${reason}"]`);
    return (
      user &&
      message &&
      ended &&
      (await driver.element(user, "GET", "text")) === prompt &&
      (await driver.element(message, "GET", "text")) === answer
    );
  });
}

async function freePort() {
  const server = createServer();
  await new Promise((accept, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", accept);
  });
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return port;
}

/** Ends a process this harness started, with every descendant it launched. */
async function stopTree(child) {
  if (child.exitCode !== null || child.signalCode !== null || !child.pid) return;
  const killer = spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
    windowsHide: true,
    stdio: "ignore",
  });
  await new Promise((done) => {
    killer.once("exit", done);
    killer.once("error", done);
  });
  const deadline = Date.now() + 10_000;
  while (child.exitCode === null && child.signalCode === null && Date.now() < deadline)
    await delay(50);
}

/** The runner's visible desktop, to show a dialog or blank window that no session can report. */
export async function captureDesktop(path) {
  const script = [
    "Add-Type -AssemblyName System.Windows.Forms, System.Drawing",
    "$b = [System.Windows.Forms.SystemInformation]::VirtualScreen",
    "$bitmap = New-Object System.Drawing.Bitmap $b.Width, $b.Height",
    "$graphics = [System.Drawing.Graphics]::FromImage($bitmap)",
    "$graphics.CopyFromScreen($b.Left, $b.Top, 0, 0, $bitmap.Size)",
    "$bitmap.Save($env:GHOST_DESKTOP_CAPTURE, [System.Drawing.Imaging.ImageFormat]::Png)",
  ].join("; ");
  const child = spawn("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
    env: { ...process.env, GHOST_DESKTOP_CAPTURE: path },
    windowsHide: true,
    stdio: "ignore",
  });
  const exit = await Promise.race([
    new Promise((done) => {
      child.once("exit", done);
      child.once("error", () => done(-1));
    }),
    delay(20_000).then(() => "timed out"),
  ]);
  await stopTree(child);
  if (exit !== 0) throw new Error(`desktop capture ${exit}`);
  return path;
}

/**
 * While a session is still pending: which WebView2 processes run with which switches, and where
 * any DevTools port file was written. Edge WebDriver reads the port from its own profile folder.
 */
async function webviewEvidence(path, roots) {
  const script = [
    "$names = 'ghost.exe', 'msedgewebview2.exe', 'msedgedriver.exe'",
    "$processes = Get-CimInstance Win32_Process | Where-Object { $names -contains $_.Name } |",
    "  Select-Object ProcessId, ParentProcessId, Name, CommandLine",
    "$ports = $env:GHOST_EVIDENCE_ROOTS -split ';' | Where-Object { $_ -and (Test-Path $_) } |",
    "  ForEach-Object { Get-ChildItem -LiteralPath $_ -Recurse -Force -Filter DevToolsActivePort -ErrorAction SilentlyContinue } |",
    "  Select-Object FullName, LastWriteTimeUtc",
    "@{ processes = @($processes); ports = @($ports) } | ConvertTo-Json -Depth 4 |",
    "  Set-Content -LiteralPath $env:GHOST_EVIDENCE_PATH -Encoding utf8",
  ].join("\n");
  const child = spawn("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], {
    env: {
      ...process.env,
      GHOST_EVIDENCE_PATH: path,
      GHOST_EVIDENCE_ROOTS: roots.join(";"),
    },
    windowsHide: true,
    stdio: "ignore",
  });
  await Promise.race([
    new Promise((done) => {
      child.once("exit", done);
      child.once("error", done);
    }),
    delay(30_000),
  ]);
  await stopTree(child);
}

/**
 * One session attempt straight against the native driver, with its own verbose log. tauri-driver
 * discards that driver's output, so this separates a proxy failure from a WebView2 attach failure.
 * Runs only after the proxied attempt and its processes have ended.
 */
export async function directSession({ application, nativePath, env, cwd, artifacts }) {
  const port = await freePort();
  const log = join(artifacts, "msedgedriver.log");
  const child = spawn(
    nativePath,
    [`--port=${port}`, "--host=127.0.0.1", "--verbose", `--log-path=${log}`],
    // tauri-driver sets these for the native driver and so for the app it starts.
    {
      env: { ...env, TAURI_AUTOMATION: "true", TAURI_WEBVIEW_AUTOMATION: "true" },
      cwd,
      windowsHide: true,
      stdio: "ignore",
    },
  );
  const native = new WebDriver(`http://127.0.0.1:${port}`);
  const started = Date.now();
  let probe = Promise.resolve();
  try {
    await until(
      "direct native driver",
      async () => {
        try {
          return (await native.request("GET", "/status", undefined, 2_000))?.ready;
        } catch {
          return false;
        }
      },
      { timeout: 15_000 },
    );
    // Edge WebDriver gives up and stops the app after 60 seconds, so look while it still runs.
    probe = delay(40_000).then(() =>
      webviewEvidence(join(artifacts, "webview.json"), [
        process.env.TEMP,
        process.env.LOCALAPPDATA,
        env.LOCALAPPDATA,
        cwd,
      ]),
    );
    const result = await native.request(
      "POST",
      "/session",
      {
        capabilities: {
          alwaysMatch: {
            browserName: "webview2",
            "ms:edgeChromium": true,
            "ms:edgeOptions": { binary: application, args: [] },
          },
        },
      },
      60_000,
    );
    native.session = result?.sessionId ?? null;
    return {
      opened: Boolean(native.session),
      seconds: (Date.now() - started) / 1000,
      browserVersion: result?.capabilities?.browserVersion ?? null,
      log: "msedgedriver.log",
    };
  } catch (error) {
    return {
      opened: false,
      seconds: (Date.now() - started) / 1000,
      error: String(error?.message ?? error),
      log: "msedgedriver.log",
    };
  } finally {
    await probe.catch(() => {});
    await native.close().catch(() => {});
    await stopTree(child);
  }
}

export async function run(args) {
  assert.equal(process.platform, "win32", "this gate needs Windows WebView2");
  assert.equal(
    args.length,
    4,
    "usage: webdriver-smoke.mjs <app.exe> <tauri-driver.exe> <msedgedriver.exe> <artifacts>",
  );
  const [application, driverPath, nativePath, artifactRoot] = args.map((path) => resolve(path));
  for (const path of [application, driverPath, nativePath]) {
    assert(
      path.toLowerCase().endsWith(".exe") && (await stat(path)).isFile(),
      `not an executable: ${path}`,
    );
  }
  const expected = process.env.EXPECTED_WEBVIEW_VERSION;
  assert.match(expected ?? "", /^\d+\.\d+\.\d+\.\d+$/, "EXPECTED_WEBVIEW_VERSION is required");
  assert(
    !Object.keys(process.env).some((key) => /^WEBVIEW2_/i.test(key) && process.env[key]),
    "remove inherited WebView2 overrides before this gate",
  );
  await mkdir(artifactRoot, { recursive: true });
  const artifacts = await mkdtemp(join(artifactRoot, "run-"));
  const root = await mkdtemp(join(tmpdir(), "auracode-webdriver-"));
  const folder = join(root, "project with spaces");
  const env = isolatedEnvironment(process.env, root);
  for (const directory of [folder, env.HOME, env.APPDATA, env.LOCALAPPDATA])
    await mkdir(directory, { recursive: true });
  const port = await freePort();
  let nativePort = await freePort();
  while (nativePort === port) nativePort = await freePort();
  const child = spawn(
    driverPath,
    ["--port", String(port), "--native-port", String(nativePort), "--native-driver", nativePath],
    {
      env,
      cwd: root,
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  const report = {
    ok: false,
    application,
    expected_runtime: expected,
    runtime: null,
    checks: [],
    error: null,
    cleanup: null,
  };
  let log = "";
  let spawnError;
  child.on("error", (error) => {
    spawnError = error;
  });
  for (const stream of [child.stdout, child.stderr])
    stream.on("data", (chunk) => {
      log = (log + chunk.toString()).slice(-1_048_576);
    });
  const driver = new WebDriver(`http://127.0.0.1:${port}`);
  const evidence = async (name) => {
    const source = await driver.command("GET", "/source");
    await writeFile(join(artifacts, `${name}.html`), source);
    const screenshot = await driver.command("GET", "/screenshot");
    await writeFile(join(artifacts, `${name}.png`), Buffer.from(screenshot, "base64"));
  };
  // tauri-driver 2.1.0 owns a Windows kill-on-close Job Object. Killing only this still-owned child
  // closes that job and its app/EdgeDriver descendants, including failed session startup.
  const watchdog = setTimeout(() => {
    if (child.exitCode === null) child.kill();
  }, 240_000);
  const stopOnSignal = () => {
    if (child.exitCode === null) child.kill();
  };
  process.once("SIGINT", stopOnSignal);
  process.once("SIGTERM", stopOnSignal);
  try {
    await until("external driver", async () => {
      if (spawnError) throw spawnError;
      assert.equal(child.exitCode, null, "tauri-driver exited before opening a session");
      try {
        return (await driver.request("GET", "/status", undefined, 2_000))?.ready;
      } catch (error) {
        // The proxy may bind before EdgeDriver does. Only startup transport failures retry;
        // a returned WebDriver protocol error remains actionable and fails immediately.
        if (
          ["ECONNREFUSED", "UND_ERR_SOCKET"].includes(error.cause?.code) ||
          error.name === "TimeoutError"
        )
          return false;
        throw error;
      }
    });
    report.runtime = verifyRuntime(await driver.open(application), expected);
    await exerciseApp(driver, folder, evidence, report.checks);
    report.ok = true;
  } catch (error) {
    report.error = error.stack ?? String(error);
    if (driver.session) await evidence("failure").catch(() => {});
    else
      report.desktop = await captureDesktop(join(artifacts, "desktop.png")).then(
        () => "desktop.png",
        (failure) => failure.message,
      );
  } finally {
    try {
      await driver.close();
    } catch (error) {
      report.cleanup = `session close: ${error.message}`;
      report.ok = false;
    }
    stopOnSignal();
    const deadline = Date.now() + 10_000;
    while (
      child.exitCode === null &&
      child.signalCode === null &&
      !spawnError &&
      Date.now() < deadline
    )
      await delay(50);
    if (child.exitCode === null && child.signalCode === null && !spawnError) {
      report.cleanup = "tauri-driver did not confirm exit; its owned job cleanup is unconfirmed";
      report.ok = false;
    }
    clearTimeout(watchdog);
    process.removeListener("SIGINT", stopOnSignal);
    process.removeListener("SIGTERM", stopOnSignal);
    // Diagnosis only. It never turns a failed gate into a pass.
    if (report.runtime === null && !report.cleanup)
      report.direct = await directSession({ application, nativePath, env, cwd: root, artifacts });
    await writeFile(join(artifacts, "driver.log"), log);
    await writeFile(join(artifacts, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  }
  console.log(`WebDriver evidence: ${artifacts}`);
  assert(report.ok, report.error ?? report.cleanup ?? "external smoke failed");
  return report;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  run(process.argv.slice(2)).catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
