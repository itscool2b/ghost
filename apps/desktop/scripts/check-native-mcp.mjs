/** Actual installed native protocols, isolated empty homes, local MCP, no auth or model prompt. */
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createInterface } from "node:readline";

const arg = (name) => {
  const at = process.argv.indexOf(name);
  return at < 0 ? null : process.argv[at + 1];
};
const output = resolve("../../target/native-mcp-probe");
await mkdir(output, { recursive: true });
const realm = await mkdtemp(resolve(output, "isolated-"));
const env = Object.fromEntries(
  Object.entries(process.env).filter(
    ([key]) => !/TOKEN|KEY|SECRET|PASSWORD|CREDENTIAL|AUTH/i.test(key),
  ),
);
Object.assign(env, {
  HOME: realm,
  USERPROFILE: realm,
  CODEX_HOME: resolve(realm, "codex"),
  CLAUDE_CONFIG_DIR: resolve(realm, "claude"),
  PI_CODING_AGENT_DIR: resolve(realm, "pi"),
  APPDATA: resolve(realm, "roaming"),
  LOCALAPPDATA: resolve(realm, "local"),
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
  DISABLE_TELEMETRY: "1",
  NO_COLOR: "1",
});
for (const folder of [
  env.CODEX_HOME,
  env.CLAUDE_CONFIG_DIR,
  env.PI_CODING_AGENT_DIR,
  env.APPDATA,
  env.LOCALAPPDATA,
])
  await mkdir(folder, { recursive: true });
const server = resolve(realm, "mcp.cjs");
await writeFile(
  server,
  `const {createInterface}=require('node:readline');createInterface({input:process.stdin}).on('line',line=>{let m;try{m=JSON.parse(line);}catch{return;}if(!('id'in m))return;let result=m.method==='initialize'?{protocolVersion:'2025-11-25',capabilities:{tools:{}},serverInfo:{name:'Aura native probe',version:'1.0.0'}}:m.method==='tools/list'?{tools:[{name:'probe',description:'Local synthetic probe, never invoked',inputSchema:{type:'object',properties:{}}}]}:m.method==='resources/list'?{resources:[]}:m.method==='resources/templates/list'?{resourceTemplates:[]}:m.method==='prompts/list'?{prompts:[]}:{};process.stdout.write(JSON.stringify({jsonrpc:'2.0',id:m.id,result})+'\\n');});`,
);
function start(program, args) {
  const child = spawn(program, args, {
    cwd: realm,
    env,
    windowsHide: true,
    stdio: ["pipe", "pipe", "pipe"],
  });
  const frames = [];
  const waiting = [];
  let tail = "";
  child.stderr.on("data", (chunk) => {
    tail = (tail + chunk).slice(-1024);
  });
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    if (line.length > 1024 * 1024) return;
    let value;
    try {
      value = JSON.parse(line);
    } catch {
      return;
    }
    frames.push(value);
    for (const waiter of [...waiting])
      if (waiter.match(value)) {
        waiting.splice(waiting.indexOf(waiter), 1);
        clearTimeout(waiter.timer);
        waiter.resolve(value);
      }
  });
  return {
    child,
    frames,
    send(value) {
      child.stdin.write(`${JSON.stringify(value)}\n`);
    },
    wait(match) {
      const existing = frames.find(match);
      if (existing) return Promise.resolve(existing);
      return new Promise((resolve, reject) => {
        const item = {
          match,
          resolve,
          timer: setTimeout(() => {
            waiting.splice(waiting.indexOf(item), 1);
            reject(
              new Error("Native protocol did not return the expected response before 20 seconds."),
            );
          }, 20000),
        };
        waiting.push(item);
      });
    },
    async stop() {
      child.stdin.end();
      await Promise.race([
        new Promise((done) => child.once("exit", done)),
        new Promise((done) => setTimeout(done, 1500)),
      ]);
      if (child.exitCode === null) {
        if (process.platform === "win32")
          spawnSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], {
            windowsHide: true,
            stdio: "ignore",
          });
        else child.kill("SIGTERM");
      }
      lines.close();
    },
    stderr() {
      return tail;
    },
  };
}
const results = {
  native: true,
  model_prompts: 0,
  oauth_flows: 0,
  credentials_copied: false,
  realm,
  agents: [],
};
for (const [name, program] of [
  ["codex", arg("--codex")],
  ["claude", arg("--claude")],
]) {
  if (!program) continue;
  const version = spawnSync(program, ["--version"], {
    env,
    cwd: realm,
    windowsHide: true,
    encoding: "utf8",
    timeout: 10000,
  });
  const entry = {
    agent: name,
    version: (version.stdout || "").trim(),
    version_exit: version.status,
  };
  results.agents.push(entry);
  const mcpServers = { ghost_native_probe: { command: process.execPath, args: [server] } };
  const peer = start(
    program,
    name === "codex"
      ? ["app-server", "--listen", "stdio://"]
      : [
          "--input-format",
          "stream-json",
          "--output-format",
          "stream-json",
          "--verbose",
          "--permission-mode",
          "default",
          "--mcp-config",
          JSON.stringify({ mcpServers }),
        ],
  );
  try {
    if (name === "codex") {
      peer.send({
        id: 1,
        method: "initialize",
        params: {
          clientInfo: { name: "auracode-native-mcp-probe", version: "1.0.0" },
          capabilities: { experimentalApi: true },
        },
      });
      const init = await peer.wait((v) => v.id === 1);
      assert.ok(init.result);
      entry.initialized = true;
      peer.send({ method: "initialized" });
      peer.send({
        id: 2,
        method: "thread/start",
        params: {
          cwd: realm,
          approvalPolicy: "untrusted",
          sandbox: "read-only",
          config: { mcp_servers: mcpServers },
        },
      });
      const opened = await peer.wait((v) => v.id === 2);
      if (opened.error) {
        entry.opened = false;
        entry.reason = "Native thread/start rejected in an empty unauthenticated home";
        continue;
      }
      const threadId = opened.result.thread.id;
      entry.opened = true;
      for (let attempt = 0; attempt < 6; attempt++) {
        const id = 3 + attempt;
        peer.send({
          id,
          method: "mcpServerStatus/list",
          params: { threadId, serverName: "ghost_native_probe", limit: 1, detail: "full" },
        });
        const reply = await peer.wait((v) => v.id === id);
        const row = reply.result?.data?.find((row) => row.name === "ghost_native_probe");
        entry.mcp_state = row?.runtimeStatus ?? "unknown";
        entry.tool_count = Object.keys(row?.tools ?? {}).length;
        if (entry.mcp_state === "connected") break;
        await new Promise((done) => setTimeout(done, 500));
      }
    } else {
      peer.send({
        type: "control_request",
        request_id: "probe-init",
        request: { subtype: "initialize" },
      });
      const init = await peer.wait(
        (v) => v.type === "control_response" && v.response?.request_id === "probe-init",
      );
      entry.initialized = init.response.subtype === "success";
      for (let attempt = 0; attempt < 6; attempt++) {
        const request_id = `probe-status-${attempt}`;
        peer.send({ type: "control_request", request_id, request: { subtype: "mcp_status" } });
        const reply = await peer.wait(
          (v) => v.type === "control_response" && v.response?.request_id === request_id,
        );
        const row = reply.response?.response?.mcpServers?.find(
          (row) => row.name === "ghost_native_probe",
        );
        entry.mcp_state = row?.status ?? "unknown";
        entry.tool_count = row?.tools?.length ?? 0;
        if (entry.mcp_state === "connected") break;
        await new Promise((done) => setTimeout(done, 500));
      }
    }
  } catch (error) {
    entry.error = error.message;
  } finally {
    await peer.stop();
  }
}
const pi = arg("--pi-launcher");
if (pi) {
  const version = spawnSync(process.execPath, [pi, "--version"], {
    env,
    cwd: realm,
    windowsHide: true,
    encoding: "utf8",
    timeout: 10000,
  });
  const entry = {
    agent: "pi",
    version: (version.stdout || "").trim(),
    version_exit: version.status,
    native_session_tested: true,
  };
  results.agents.push(entry);
  const extension = resolve(realm, "shared-mcp.ts");
  await writeFile(
    extension,
    await readFile(new URL("../../../crates/ghost-drivers/src/pi/shared-mcp.ts", import.meta.url)),
  );
  env.GHOST_SHARED_MCP = JSON.stringify({
    ghost_native_probe: { command: process.execPath, args: [server] },
  });
  const peer = start(process.execPath, [pi, "--mode", "rpc", "--extension", extension]);
  try {
    peer.send({ id: "pi-state", type: "get_state" });
    const state = await peer.wait((v) => v.type === "response" && v.id === "pi-state");
    entry.initialized = state.success === true;
    peer.send({ id: "pi-commands", type: "get_commands" });
    const commands = await peer.wait((v) => v.type === "response" && v.id === "pi-commands");
    entry.extension_registered =
      commands.data?.commands?.some((command) => command.name === "ghost-shared-mcp-ready") ??
      false;
  } catch (error) {
    entry.error = error.message;
  } finally {
    await peer.stop();
  }
}
await writeFile(resolve(output, "results.json"), `${JSON.stringify(results, null, 2)}\n`);
console.log(JSON.stringify(results, null, 2));
