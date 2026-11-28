/**
 * Run the fixture checks that read their server from CHECK_URL (dictation, images, the
 * Dashboard, typing while a turn streams, agent update notices, terminal chats, Canvas, the
 * browser beside the chat, the sidebar while the app opens, the tour, the plan card, reading aloud and the larger dictation model) against one owned Vite
 * server, as CI's lint job does. Each check covers both browser engines itself. `--part 1` runs
 * the first seven and `--part 2` the rest, so each of CI's two steps fits its 15 minutes.
 * Usage: node scripts/check-fixture-suite.mjs [--server URL] [--part 1|2]
 */
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const { values } = parseArgs({
  options: { server: { type: "string" }, part: { type: "string" } },
});
if (values.part !== undefined && values.part !== "1" && values.part !== "2")
  throw new Error(`--part is 1 or 2, not ${values.part}`);
const app = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);
const url = values.server ?? "http://127.0.0.1:1476";
let server;
let output = "";
try {
  if (!values.server) {
    const vite = resolve(dirname(require.resolve("vite/package.json")), "bin/vite.js");
    server = spawn(
      process.execPath,
      [vite, "--host", "127.0.0.1", "--port", "1476", "--strictPort"],
      {
        cwd: app,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    for (const stream of [server.stdout, server.stderr])
      stream.on("data", (data) => {
        output = (output + data).slice(-8192);
      });
  }
  const deadline = Date.now() + 30000;
  for (;;) {
    if (server?.exitCode != null) throw new Error(`Fixture server exited: ${output}`);
    try {
      if ((await fetch(url, { signal: AbortSignal.timeout(2000) })).ok) break;
    } catch {}
    if (Date.now() >= deadline) throw new Error(`Fixture server did not become ready: ${output}`);
    await new Promise((done) => setTimeout(done, 150));
  }
  const scripts = [
    "check-dictation.mjs",
    "check-images.mjs",
    "check-dashboard.mjs",
    "check-composer-follow.mjs",
    "check-agent-updates.mjs",
    "check-terminal-chat.mjs",
    "check-canvas.mjs",
    "check-browser.mjs",
    "check-device.mjs",
    "check-game.mjs",
    "check-opening.mjs",
    "check-tour.mjs",
    "check-plan.mjs",
    "check-read-aloud.mjs",
    "check-dictation-larger.mjs",
  ];
  const part =
    values.part === "1" ? scripts.slice(0, 7) : values.part === "2" ? scripts.slice(7) : scripts;
  for (const script of part) {
    await new Promise((complete, reject) => {
      const check = spawn(process.execPath, [resolve(app, "scripts", script)], {
        cwd: app,
        env: { ...process.env, CHECK_URL: url },
        windowsHide: true,
        stdio: "inherit",
      });
      check.once("error", reject);
      check.once("exit", (code, signal) =>
        code === 0 ? complete() : reject(new Error(`${script} failed (${code ?? signal})`)),
      );
    });
  }
} finally {
  server?.kill();
}
