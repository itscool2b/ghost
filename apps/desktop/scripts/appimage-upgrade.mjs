// `pnpm host-upgrade` on Linux: replaces the installed o7 AppImage with a local build, and
// puts the previous build back when the new one does not work.
//
// The installed app is one file behind a link, `~/Applications/o7.AppImage`, which the
// desktop entry starts. A running AppImage reads its own file, so a new build is a new file next to
// it and the link moves; the running app is untouched and the previous files stay for a rollback.
// The installed app is usually the one running the agent session that built the new one, and an
// agent's shutdown ends every process it started, so `install --after-exit` starts its waiter
// under the user's systemd (`systemd-run --user`), which shares nothing with the app, and returns.
// When the person quits o7 the waiter moves the link, starts o7, and moves the link
// back and starts the previous build if the new one does not stay up. `rollback` moves the link
// back by hand and, with `--restore-database`, restores the databases a newer schema migrated.
// See docs/developing-in-o7.md.

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  chmodSync,
  closeSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  readSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { cleanEnv, EXIT, restoreDatabases, stampOf } from "./host-upgrade.mjs";

const SCRIPT = fileURLToPath(import.meta.url);
const REPO = resolve(dirname(SCRIPT), "..", "..", "..");
/**
 * A kept build: `o7_<anything>.AppImage`, as builds are named since the rename, or
 * `auracode_<anything>.AppImage` from before it; never the link itself.
 */
const BUILD = /^(o7|auracode)_.+\.AppImage$/u;

const USAGE = `Usage: pnpm --filter @o7/desktop host-upgrade <command> [options]   (Linux)

  install [--appimage <file>] [--after-exit [--wait <minutes>]]
      Copies a local AppImage next to the installed one and points the link at it. The newest
      target/**/release/bundle/appimage/*.AppImage is used by default. --after-exit arms it
      instead: it runs once o7 quits (default wait 30 minutes).
  rollback [--to <file>] [--restore-database]
      Points the link back at the previous build (or --to). --restore-database also restores
      each database a newer build migrated since then, from o7-core's pre-migration backup.
  status    What is installed, running, armed and kept.
  cancel    Stops an armed install.

  --link, --data-dir and --state-dir override ~/Applications/o7.AppImage (auracode.AppImage on a
  machine set up before the rename),
  the data folder ($XDG_DATA_HOME/codes.o7.desktop, or si.auracode.desktop from then) and
  $XDG_STATE_HOME/o7-upgrades (or auracode-upgrades).`;

/** The files the commands work on. */
export function folders(env, overrides = {}) {
  const home = env.HOME || homedir();
  const data = env.XDG_DATA_HOME || join(home, ".local", "share");
  const state = env.XDG_STATE_HOME || join(home, ".local", "state");
  return {
    link: resolve(
      overrides.link ??
        inUse(
          join(home, "Applications", "o7.AppImage"),
          join(home, "Applications", "auracode.AppImage"),
        ),
    ),
    dataDir: resolve(
      overrides.dataDir ??
        inUse(join(data, "codes.o7.desktop"), join(data, "si.auracode.desktop"), true),
    ),
    stateDir: resolve(
      overrides.stateDir ?? inUse(join(state, "o7-upgrades"), join(state, "auracode-upgrades")),
    ),
  };
}

/**
 * o7 was auracode until 19 November 2026. A machine set up before then keeps its link, its data
 * and its kept upgrades under the earlier names: `current` when it is used, else `earlier` when
 * that is, else `current`. A data folder is used when it has `device.json` or `profiles`;
 * anything else when it exists.
 */
export function inUse(current, earlier, data = false) {
  const used = (path) =>
    data
      ? existsSync(join(path, "device.json")) || existsSync(join(path, "profiles"))
      : existsSync(path);
  return !used(current) && used(earlier) ? earlier : current;
}

/** Whether `file` is a type 2 AppImage (`AI\x02` at byte 8 of an ELF file). */
export function isAppImage(file) {
  let fd;
  try {
    fd = openSync(file, "r");
    const head = Buffer.alloc(11);
    if (readSync(fd, head, 0, 11, 0) < 11) return false;
    return (
      head.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46])) &&
      head.subarray(8, 11).equals(Buffer.from([0x41, 0x49, 0x02]))
    );
  } catch {
    return false;
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

/** The newest AppImage `tauri build` left in `roots` (`<root>[/<triple>]/release/bundle/appimage`). */
export function newestAppImage(roots) {
  const found = [];
  for (const root of roots.filter(Boolean)) {
    if (!existsSync(root)) continue;
    const releases = [join(root, "release")];
    for (const entry of readdirSync(root, { withFileTypes: true })) {
      if (entry.isDirectory()) releases.push(join(root, entry.name, "release"));
    }
    for (const release of releases) {
      const dir = join(release, "bundle", "appimage");
      if (!existsSync(dir)) continue;
      for (const name of readdirSync(dir)) {
        if (name.endsWith(".AppImage")) {
          const path = join(dir, name);
          found.push({ path, at: statSync(path).mtimeMs });
        }
      }
    }
  }
  found.sort((a, b) => b.at - a.at);
  return found[0]?.path;
}

/** The kept name for `source` staged at `date`: its own name with the time added. */
export function stagedName(source, date) {
  const stem = basename(source).replace(/\.AppImage$/u, "");
  const named = stem.startsWith("o7_") ? stem : `o7_${stem}`;
  return `${named}_${stampOf(date)}.AppImage`;
}

/** What the link points at, as an absolute path, or undefined when it is not a link. */
export function linkTarget(link) {
  try {
    if (!lstatSync(link).isSymbolicLink()) return undefined;
  } catch {
    return undefined;
  }
  return resolve(dirname(link), readlinkSync(link));
}

/** Points `link` at `target` in one rename, so the desktop entry never sees it missing. */
export function pointLink(link, target) {
  const temporary = `${link}.next-${process.pid}`;
  rmSync(temporary, { force: true });
  symlinkSync(dirname(target) === dirname(link) ? basename(target) : target, temporary);
  renameSync(temporary, link);
}

/** `env` without an agent session's variables and without what an AppImage launcher set. */
export function launchEnv(env) {
  const clean = cleanEnv(env);
  const appdir = (clean.APPDIR ?? "").replace(/\/+$/u, "");
  for (const name of ["APPDIR", "APPIMAGE", "ARGV0", "OWD"]) delete clean[name];
  if (!appdir.startsWith("/") || appdir.length < 2) return clean;
  const inside = (entry) => entry === appdir || entry.startsWith(`${appdir}/`);
  if ((clean.GTK_DATA_PREFIX ?? "").split(":").some(inside)) delete clean.GTK_THEME;
  if ((clean.PYTHONHOME ?? "").split(":").some(inside)) delete clean.PYTHONDONTWRITEBYTECODE;
  for (const [name, value] of Object.entries(clean)) {
    const entries = value.split(":");
    if (!entries.some(inside)) continue;
    const kept = [...new Set(entries.filter((entry) => entry && !inside(entry)))];
    if (kept.length) clean[name] = kept.join(":");
    else delete clean[name];
  }
  return clean;
}

/**
 * Which kept build in `dir` a process runs, or undefined. The AppImage runtime's own executable is
 * the build file; the app it starts runs from the image's mount (`/tmp/.mount_…`) with `APPIMAGE`
 * naming the file. Both count: when o7 quits, the runtime can end, and its mount go, while
 * the app is still shutting down, and a build started then would hand over to that app and exit.
 * `environ` is the process's NUL-separated environment, read only for executables in a mount.
 */
export function buildOf(exe, environ, dir) {
  const own = (file) => dirname(file) === dir && BUILD.test(basename(file));
  const file = exe.replace(/ \(deleted\)$/u, "");
  if (own(file)) return file;
  if (!file.includes("/.mount_") || !environ) return undefined;
  const entry = environ
    .toString("utf8")
    .split("\0")
    .find((line) => line.startsWith("APPIMAGE="));
  if (!entry) return undefined;
  let image = entry.slice("APPIMAGE=".length);
  try {
    image = realpathSync(image);
  } catch {}
  return own(image) ? image : undefined;
}

/** The real system: `/proc`, `systemd-run --user`, timers. Tests pass their own. */
export const linux = {
  /**
   * The kept builds running now, one entry per process ([`buildOf`]): the AppImage runtime, and
   * the app inside its mount with its WebKit processes.
   */
  running(dir) {
    const files = [];
    for (const pid of readdirSync("/proc")) {
      if (!/^\d+$/u.test(pid) || Number(pid) === process.pid) continue;
      try {
        const exe = readlinkSync(`/proc/${pid}/exe`);
        const environ = exe.includes("/.mount_") ? readFileSync(`/proc/${pid}/environ`) : undefined;
        const build = buildOf(exe, environ, dir);
        if (build) files.push({ pid: Number(pid), exe: build });
      } catch {}
    }
    return files;
  },
  systemd() {
    return spawnSync("systemd-run", ["--user", "--version"], { stdio: "ignore" }).status === 0;
  },
  /** Starts `program` with `args` outside o7's process tree; returns how to stop it. */
  detach(name, program, args, env) {
    if (this.systemd()) {
      const unit = `${name}-${Date.now()}`;
      const result = spawnSync(
        "systemd-run",
        ["--user", "--collect", "--quiet", `--unit=${unit}`, "--", program, ...args],
        { encoding: "utf8", env },
      );
      if (result.status !== 0) throw new Error(`systemd-run failed: ${result.stderr.trim()}`);
      return { unit };
    }
    // No user systemd: a new session whose parent exits at once, so it reparents to init.
    const child = spawn("setsid", ["-f", program, ...args], {
      detached: true,
      stdio: "ignore",
      env,
    });
    child.unref();
    return { pid: child.pid };
  },
  alive(handle) {
    if (handle.unit) {
      return spawnSync("systemctl", ["--user", "is-active", "--quiet", handle.unit]).status === 0;
    }
    try {
      process.kill(handle.pid, 0);
      return true;
    } catch {
      return false;
    }
  },
  stop(handle) {
    if (handle.unit) spawnSync("systemctl", ["--user", "stop", handle.unit], { stdio: "ignore" });
    else if (handle.pid) process.kill(handle.pid, "SIGTERM");
  },
  now: () => new Date(),
  sleep: (ms) => new Promise((done) => setTimeout(done, ms)),
};

function logger(stateDir, print = console.log) {
  return (message) => {
    print(message);
    mkdirSync(stateDir, { recursive: true });
    appendFileSync(join(stateDir, "upgrade.log"), `${new Date().toISOString()} ${message}\n`);
  };
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function history(stateDir) {
  try {
    return JSON.parse(readFileSync(join(stateDir, "history.json"), "utf8"));
  } catch {
    return [];
  }
}

function remember(stateDir, entry) {
  mkdirSync(stateDir, { recursive: true });
  const all = [...history(stateDir), entry].slice(-20);
  writeFileSync(join(stateDir, "history.json"), `${JSON.stringify(all, null, 2)}\n`);
}

const armedPath = (stateDir) => join(stateDir, "armed.json");

function armedRecord(stateDir) {
  try {
    return JSON.parse(readFileSync(armedPath(stateDir), "utf8"));
  } catch {
    return undefined;
  }
}

/** Kept builds beside the link, newest first. */
function keptBuilds(link) {
  const dir = dirname(link);
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => BUILD.test(name))
    .map((name) => join(dir, name))
    .filter((path) => path !== link && lstatSync(path).isFile())
    .sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
}

/** Deletes kept builds beyond the newest `keep`, never the current, previous or a running one. */
export function prune(paths, keep, system, log) {
  const current = linkTarget(paths.link);
  const previous = history(paths.stateDir).at(-1)?.from;
  const running = new Set(system.running(dirname(paths.link)).map((file) => file.exe));
  const armed = armedRecord(paths.stateDir)?.staged;
  const spare = keptBuilds(paths.link).filter(
    (path) => path !== current && path !== previous && path !== armed && !running.has(path),
  );
  for (const path of spare.slice(Math.max(0, keep - 1))) {
    rmSync(path, { force: true });
    log(`Removed the old build ${basename(path)}.`);
  }
}

/** Copies `source` beside the link under a new name; the worktree may go away after this. */
function stage(source, paths, system) {
  const target = join(dirname(paths.link), stagedName(source, system.now()));
  mkdirSync(dirname(target), { recursive: true });
  copyFileSync(source, `${target}.partial`);
  chmodSync(`${target}.partial`, 0o755);
  renameSync(`${target}.partial`, target);
  return target;
}

/** Whether a process runs `file` after `seconds`. */
async function healthy(system, file, seconds) {
  await system.sleep(seconds * 1000);
  return system.running(dirname(file)).some((process) => process.exe === file);
}

/** Starts the build the link names, outside o7's tree. */
function launch(paths, system, env) {
  system.detach("auracode", paths.link, [], launchEnv(env));
}

/**
 * Points the link at `staged`, starts it, and points it back at the previous build (starting
 * that) when the new one is not running `healthSeconds` later.
 */
export async function activate(staged, options, paths, system, log, env) {
  let previous = linkTarget(paths.link);
  if (!previous && existsSync(paths.link)) {
    // An install that was the file itself: keep it as a build so a rollback has it.
    previous = join(dirname(paths.link), `auracode_previous_${stampOf(system.now())}.AppImage`);
    renameSync(paths.link, previous);
    log(`Kept the installed file as ${basename(previous)}.`);
  }
  pointLink(paths.link, staged);
  const hash = sha256(staged);
  remember(paths.stateDir, {
    at: system.now().getTime(),
    from: previous,
    to: staged,
    sha256: hash,
  });
  log(
    `Installed ${basename(staged)}: sha256 ${hash.slice(0, 12)}…${previous ? ` (previous ${basename(previous)})` : ""}`,
  );
  launch(paths, system, env);
  if (await healthy(system, staged, options.healthSeconds)) {
    log(`o7 is running from ${basename(staged)}.`);
    prune(paths, options.keep, system, log);
    return EXIT.done;
  }
  if (!previous) {
    log(
      `${basename(staged)} is not running ${options.healthSeconds} s after it started, and there is no previous build.`,
    );
    return EXIT.failed;
  }
  pointLink(paths.link, previous);
  remember(paths.stateDir, {
    at: system.now().getTime(),
    from: staged,
    to: previous,
    rollback: true,
  });
  log(
    `${basename(staged)} is not running ${options.healthSeconds} s after it started; the link is back on ${basename(previous)}.`,
  );
  launch(paths, system, env);
  return EXIT.failed;
}

export async function install(options, paths, system, log, env) {
  const source = options.appimage
    ? resolve(options.appimage)
    : newestAppImage([env.CARGO_TARGET_DIR, join(REPO, "target")]);
  if (!source || !existsSync(source)) {
    log(
      source
        ? `There is no file ${source}.`
        : "No AppImage was found. Build one with: pnpm bundle --bundles appimage",
    );
    return EXIT.usage;
  }
  if (!isAppImage(source)) {
    log(`${source} is not an AppImage; nothing was changed.`);
    return EXIT.usage;
  }
  const armed = armedRecord(paths.stateDir);
  if (armed && system.alive(armed.handle)) {
    log("An install is already armed. Cancel it first; nothing was changed.");
    return EXIT.running;
  }
  const running = system.running(dirname(paths.link));
  if (running.length > 0 && !options.afterExit) {
    log("o7 is running. Quit it first, or arm the install with --after-exit; nothing was changed.");
    return EXIT.running;
  }
  const staged = stage(source, paths, system);
  log(`Staged ${basename(source)} as ${staged}.`);
  if (!options.afterExit) return activate(staged, options, paths, system, log, env);
  const args = [
    SCRIPT,
    "waiter",
    "--link",
    paths.link,
    "--data-dir",
    paths.dataDir,
    "--state-dir",
    paths.stateDir,
    "--wait",
    String(options.wait),
    "--keep",
    String(options.keep),
    "--health-seconds",
    String(options.healthSeconds),
  ];
  // Recorded before the waiter starts, which reads it; the handle is added once it exists.
  const record = { staged, created: system.now().getTime(), wait: options.wait };
  const write = () => {
    mkdirSync(paths.stateDir, { recursive: true });
    writeFileSync(armedPath(paths.stateDir), `${JSON.stringify(record, null, 2)}\n`);
  };
  write();
  let handle;
  try {
    handle = system.detach("auracode-upgrade", process.execPath, args, launchEnv(env));
  } catch (error) {
    rmSync(armedPath(paths.stateDir), { force: true });
    throw error;
  }
  record.handle = handle;
  write();
  log(
    `Armed: ${basename(staged)} installs when o7 quits (waiting up to ${options.wait} minutes, ${handle.unit ? `unit ${handle.unit}` : `PID ${handle.pid}`}).`,
  );
  return EXIT.done;
}

/** The armed waiter: waits for o7 to quit, then activates the staged build. */
export async function waiter(options, paths, system, log, env) {
  const armed = armedRecord(paths.stateDir);
  if (!armed) {
    log("The waiter found no armed install.");
    return EXIT.usage;
  }
  try {
    const deadline = system.now().getTime() + options.wait * 60_000;
    while (system.running(dirname(paths.link)).length > 0) {
      if (system.now().getTime() > deadline) {
        log(
          `o7 did not quit within ${options.wait} minutes; the armed install is dropped. ${basename(armed.staged)} stays until the next prune.`,
        );
        return EXIT.running;
      }
      await system.sleep(1000);
    }
    log("o7 quit; installing.");
    return await activate(armed.staged, options, paths, system, log, env);
  } finally {
    rmSync(armedPath(paths.stateDir), { force: true });
  }
}

export async function rollback(options, paths, system, log, env) {
  if (system.running(dirname(paths.link)).length > 0) {
    log("o7 is running. Quit it first; nothing was changed.");
    return EXIT.running;
  }
  const current = linkTarget(paths.link);
  const entries = history(paths.stateDir);
  const last = entries.findLast((entry) => entry.to === current && entry.from);
  const target = options.to
    ? resolve(options.to)
    : (last?.from ?? keptBuilds(paths.link).find((path) => path !== current));
  if (!target || !existsSync(target)) {
    log(target ? `There is no build ${target}.` : "There is no previous build to go back to.");
    return EXIT.usage;
  }
  if (!isAppImage(target)) {
    log(`${target} is not an AppImage; nothing was changed.`);
    return EXIT.usage;
  }
  pointLink(paths.link, target);
  remember(paths.stateDir, {
    at: system.now().getTime(),
    from: current,
    to: target,
    rollback: true,
  });
  log(`The link is back on ${basename(target)}.`);
  if (options.restoreDatabase) {
    const since = last?.at ?? 0;
    const restored = since ? restoreDatabases(paths.dataDir, since, system.now()) : [];
    if (!since) log("No install of the current build is recorded, so no database was restored.");
    else if (restored.length === 0)
      log("No database was migrated since that install; the data is unchanged.");
    for (const restore of restored) {
      log(
        `Restored ${restore.database} to schema ${restore.schema}; the migrated copy is kept as ${restore.kept}.`,
      );
    }
  }
  launch(paths, system, env);
  return EXIT.done;
}

export function status(paths, system, print = console.log) {
  const current = linkTarget(paths.link);
  print(
    `Link: ${paths.link} -> ${current ?? (existsSync(paths.link) ? "(a file, not a link)" : "none")}`,
  );
  if (current && existsSync(current)) {
    const info = statSync(current);
    print(`  built ${info.mtime.toISOString()}, ${info.size} bytes`);
  }
  const running = system.running(dirname(paths.link));
  print(
    `Running: ${running.length ? running.map((p) => `${p.pid} (${basename(p.exe)})`).join(", ") : "no"}`,
  );
  const armed = armedRecord(paths.stateDir);
  print(
    `Armed install: ${armed && system.alive(armed.handle) ? `${basename(armed.staged)} (${armed.handle.unit ?? `PID ${armed.handle.pid}`})` : "none"}`,
  );
  const kept = keptBuilds(paths.link).filter((path) => path !== current);
  print(`Kept builds: ${kept.length ? kept.map((path) => basename(path)).join(", ") : "none"}`);
  const log = join(paths.stateDir, "upgrade.log");
  if (existsSync(log)) {
    print("Last log lines:");
    for (const line of readFileSync(log, "utf8").trimEnd().split("\n").slice(-8))
      print(`  ${line}`);
  }
  return EXIT.done;
}

export function cancel(paths, system, log) {
  const armed = armedRecord(paths.stateDir);
  if (!armed || !system.alive(armed.handle)) {
    log("No install is armed.");
    rmSync(armedPath(paths.stateDir), { force: true });
    return EXIT.done;
  }
  system.stop(armed.handle);
  rmSync(armedPath(paths.stateDir), { force: true });
  log(`Cancelled the armed install of ${basename(armed.staged)}.`);
  return EXIT.done;
}

export async function main(argv, env = process.env, system = linux) {
  let parsed;
  try {
    parsed = parseArgs({
      args: argv,
      allowPositionals: true,
      options: {
        appimage: { type: "string" },
        "after-exit": { type: "boolean", default: false },
        wait: { type: "string" },
        keep: { type: "string", default: "3" },
        "health-seconds": { type: "string", default: "20" },
        to: { type: "string" },
        "restore-database": { type: "boolean", default: false },
        link: { type: "string" },
        "data-dir": { type: "string" },
        "state-dir": { type: "string" },
        help: { type: "boolean", default: false },
      },
    });
  } catch (error) {
    console.error(`${error.message}\n\n${USAGE}`);
    return EXIT.usage;
  }
  const { values, positionals } = parsed;
  const command = positionals[0];
  if (values.help || !command || positionals.length > 1) {
    console.log(USAGE);
    return values.help ? EXIT.done : EXIT.usage;
  }
  const number = (text, name) => {
    const value = Number(text);
    if (!Number.isInteger(value) || value < 0) throw new Error(`--${name} must be a whole number.`);
    return value;
  };
  let options;
  const paths = folders(env, {
    link: values.link,
    dataDir: values["data-dir"],
    stateDir: values["state-dir"],
  });
  try {
    options = {
      appimage: values.appimage,
      afterExit: values["after-exit"],
      wait: number(values.wait ?? "30", "wait"),
      keep: Math.max(1, number(values.keep, "keep")),
      healthSeconds: number(values["health-seconds"], "health-seconds"),
      to: values.to,
      restoreDatabase: values["restore-database"],
    };
  } catch (error) {
    console.error(error.message);
    return EXIT.usage;
  }
  const log = logger(paths.stateDir);
  try {
    switch (command) {
      case "install":
        return await install(options, paths, system, log, env);
      case "waiter":
        return await waiter(options, paths, system, log, env);
      case "rollback":
        return await rollback(options, paths, system, log, env);
      case "status":
        return status(paths, system);
      case "cancel":
        return cancel(paths, system, log);
      default:
        console.error(`Unknown command ${command}.\n\n${USAGE}`);
        return EXIT.usage;
    }
  } catch (error) {
    log(`Failed: ${error.message}`);
    return EXIT.failed;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT) {
  process.exitCode = await main(process.argv.slice(2));
}
