// `pnpm host-upgrade`: replaces the installed Ghost on this Windows machine with a local build,
// and puts the previous build back when the new one does not work.
// On Linux, `main` hands over to appimage-upgrade.mjs.
//
// The installed app is often the one running the agent session that built the new one, so the
// install cannot happen while it runs, and an agent cannot wait for it to close: everything an
// agent starts is in the app's job object and ends with it. `install --after-exit` therefore
// starts a waiter through WMI, outside the job, and returns. When the person quits Ghost, the
// waiter snapshots the installed app, runs the installer in update mode (which keeps the data
// folder; see src-tauri/windows/hooks.nsh), starts Ghost again, and restores the snapshot if
// the new build does not stay up. `rollback` restores a snapshot by hand and, with
// `--restore-database`, the databases a newer schema migrated (ghost-core backs each one up first).
// See docs/developing-in-ghost.md.

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const SCRIPT = fileURLToPath(import.meta.url);
const REPO = resolve(dirname(SCRIPT), "..", "..", "..");
const APP_EXE = "ghost.exe";
const UNINSTALLER = "uninstall.exe";
const HELPER_FLAG = "--ghost-memory-helper";
const SIDECARS = ["", "-wal", "-shm", "-journal"];
const BACKUP = /^(?<db>.+)\.v(?<from>\d+)-(?<at>\d+)\.bak$/u;

export const EXIT = { done: 0, usage: 2, running: 3, failed: 4 };

const USAGE = `Usage: node apps/desktop/scripts/host-upgrade.mjs <command> [options]

  install [--installer <setup.exe>] [--after-exit [--wait <minutes>]]
      Installs a local build over the installed Ghost, after a snapshot of the installed app.
      --after-exit arms it instead: it runs once Ghost quits (default wait 30 minutes).
  rollback [--snapshot <name>] [--restore-database]
      Puts a snapshot back (the newest by default). --restore-database also restores each
      database a newer build migrated since that snapshot, from ghost-core's pre-migration backup.
  status    What is installed, running, armed and kept.
  cancel    Stops an armed install.

  --install-dir, --data-dir and --state-dir override %LOCALAPPDATA%\\Ghost, the data folder
  (%LOCALAPPDATA%\\dev.onghost.desktop, or codes.o7.desktop or si.auracode.desktop on a machine
  updated from o7 or from auracode) and %LOCALAPPDATA%\\ghost-upgrades (or o7-upgrades or
  auracode-upgrades from then).`;

/** The folders the commands work on, from `%LOCALAPPDATA%` unless overridden. */
export function folders(env, overrides = {}) {
  const local = env.LOCALAPPDATA;
  if (!local && !(overrides.installDir && overrides.dataDir && overrides.stateDir)) {
    throw new Error("LOCALAPPDATA is not set, so the installed app cannot be found.");
  }
  return {
    installDir: resolve(overrides.installDir ?? join(local, "Ghost")),
    dataDir: resolve(
      overrides.dataDir ??
        inUse(
          [
            join(local, "dev.onghost.desktop"),
            join(local, "codes.o7.desktop"),
            join(local, "si.auracode.desktop"),
          ],
          true,
        ),
    ),
    stateDir: resolve(
      overrides.stateDir ??
        inUse([
          join(local, "ghost-upgrades"),
          join(local, "o7-upgrades"),
          join(local, "auracode-upgrades"),
        ]),
    ),
  };
}

/**
 * Ghost was o7 from 19 to 26 November 2026 and auracode before that. A machine updated from then
 * keeps its data, and the upgrades kept for it, under the earlier names. So of the names given,
 * newest first: the first when it was used, else the newest earlier one that was, else the
 * first. A data folder is used when it has `device.json` or `profiles`; any other folder when it
 * exists.
 */
export function inUse([current, ...earlier], data = false) {
  const used = (path) =>
    data
      ? existsSync(join(path, "device.json")) || existsSync(join(path, "profiles"))
      : existsSync(path);
  return used(current) ? current : (earlier.find(used) ?? current);
}

function inside(path, folder) {
  const a = resolve(path).toLowerCase();
  const b = resolve(folder).toLowerCase();
  return a.startsWith(b.endsWith(sep) ? b : b + sep);
}

/**
 * The installed app's processes among `processes` (`{ ProcessId, ExecutablePath, CommandLine }`):
 * `app` for the app and its cleanup, `helpers` for the project-memory helpers agents start, which
 * end with their agent.
 */
export function hostProcesses(processes, installDir) {
  const host = processes.filter(
    (process) => process.ExecutablePath && inside(process.ExecutablePath, installDir),
  );
  const helper = (process) => (process.CommandLine ?? "").includes(HELPER_FLAG);
  return { app: host.filter((p) => !helper(p)), helpers: host.filter(helper) };
}

/** The newest NSIS installer in `roots` (`<root>[/<target triple>]/release/bundle/nsis`). */
export function newestInstaller(roots) {
  const found = [];
  for (const root of roots.filter(Boolean)) {
    if (!existsSync(root)) continue;
    const releases = [join(root, "release")];
    for (const entry of readdirSync(root, { withFileTypes: true })) {
      if (entry.isDirectory()) releases.push(join(root, entry.name, "release"));
    }
    for (const release of releases) {
      const nsis = join(release, "bundle", "nsis");
      if (!existsSync(nsis)) continue;
      for (const name of readdirSync(nsis)) {
        if (name.endsWith("-setup.exe")) {
          const path = join(nsis, name);
          found.push({ path, at: statSync(path).mtimeMs });
        }
      }
    }
  }
  found.sort((a, b) => b.at - a.at);
  return found[0]?.path;
}

/** A sortable name for a snapshot taken at `date`, in local time. */
export function stampOf(date) {
  const pad = (n) => String(n).padStart(2, "0");
  return `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(
    date.getHours(),
  )}${pad(date.getMinutes())}${pad(date.getSeconds())}`;
}

/** Snapshot names to delete so the newest `keep` stay. */
export function toPrune(names, keep) {
  return [...names].sort().reverse().slice(keep);
}

/**
 * For each database in `profilesDir/<profile>/`, the first pre-migration backup ghost-core made
 * after `sinceMs`: the database as it was before a newer build first migrated it.
 */
export function backupsSince(profilesDir, sinceMs) {
  const restores = [];
  if (!existsSync(profilesDir)) return restores;
  for (const profile of readdirSync(profilesDir, { withFileTypes: true })) {
    if (!profile.isDirectory()) continue;
    const backups = join(profilesDir, profile.name, "backups");
    if (!existsSync(backups)) continue;
    const first = new Map();
    for (const name of readdirSync(backups)) {
      const match = BACKUP.exec(name);
      if (!match) continue;
      const at = Number(match.groups.at);
      if (at <= sinceMs) continue;
      const known = first.get(match.groups.db);
      if (!known || at < known.at) {
        first.set(match.groups.db, { at, from: Number(match.groups.from), name });
      }
    }
    for (const [db, backup] of first) {
      restores.push({
        profile: profile.name,
        database: join(profilesDir, profile.name, db),
        backup: join(backups, backup.name),
        schema: backup.from,
      });
    }
  }
  return restores;
}

/** `env` without the variables an agent session or a development launch sets. */
export function cleanEnv(env) {
  return Object.fromEntries(
    Object.entries(env).filter(
      ([name]) =>
        !/^(GHOST_|O7_|AURACODE_|AURA_|CLAUDECODE$|CLAUDE_CODE_|CLAUDE_PID$)/iu.test(name),
    ),
  );
}

/** The command line for the armed waiter, every argument quoted for CreateProcess. */
export function waiterCommandLine(node, script, args) {
  for (const arg of [node, script, ...args]) {
    if (arg.includes('"')) throw new Error(`An argument contains a quote: ${arg}`);
  }
  return [node, script, ...args].map((arg) => `"${arg}"`).join(" ");
}

function powershell(script) {
  const encoded = Buffer.from(script, "utf16le").toString("base64");
  const run = spawnSync(
    "powershell.exe",
    ["-NoProfile", "-NonInteractive", "-EncodedCommand", encoded],
    { encoding: "utf8", windowsHide: true },
  );
  if (run.status !== 0) {
    throw new Error(`PowerShell failed: ${(run.stderr || run.stdout || "").trim()}`);
  }
  return run.stdout.trim();
}

const quoted = (text) => `'${text.replaceAll("'", "''")}'`;

/** The real machine: processes, the installer, launches. Tests pass their own. */
export const windows = {
  processes() {
    const json = powershell(
      "@(Get-CimInstance Win32_Process -Filter \"Name='ghost.exe' OR Name='node.exe'\" | Select-Object ProcessId, ExecutablePath, CommandLine) | ConvertTo-Json -Compress",
    );
    if (!json) return [];
    const parsed = JSON.parse(json);
    return Array.isArray(parsed) ? parsed : [parsed];
  },
  stop(pid) {
    spawnSync("taskkill.exe", ["/PID", String(pid), "/T", "/F"], { windowsHide: true });
  },
  install(installer, log = () => {}) {
    // What the updater runs (tauri.conf.json: plugins > updater > windows > installMode).
    const run = spawnSync(installer, ["/P", "/UPDATE"], { stdio: "inherit" });
    if (run.error) log(`The installer could not start: ${run.error.message}`);
    return run.error ? -1 : (run.status ?? -1);
  },
  launch(exe) {
    // A policy block is an asynchronous spawn error. Await it so the waiter can restore the
    // snapshot instead of terminating on an unhandled ChildProcess error before the health check.
    return new Promise((resolve, reject) => {
      const child = spawn(exe, [], { detached: true, stdio: "ignore", env: cleanEnv(process.env) });
      child.once("error", reject);
      child.once("spawn", () => {
        child.unref();
        resolve();
      });
    });
  },
  /** Starts `commandLine` through WMI, so it is outside this process's job, with no window. */
  startOutsideJob(commandLine, cwd) {
    const out = powershell(
      [
        "$startup = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly -Property @{ ShowWindow = [uint16]0 }",
        `$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = ${quoted(commandLine)}; CurrentDirectory = ${quoted(cwd)}; ProcessStartupInformation = $startup }`,
        '"$($r.ReturnValue) $($r.ProcessId)"',
      ].join("; "),
    );
    const [code, pid] = out.split(/\s+/u).map(Number);
    if (code !== 0) throw new Error(`WMI could not start the waiter (code ${code}).`);
    return pid;
  },
  sleep: (ms) => new Promise((done) => setTimeout(done, ms)),
  now: () => new Date(),
};

function logger(stateDir, print = console.log) {
  return (message) => {
    print(message);
    mkdirSync(stateDir, { recursive: true });
    appendFileSync(join(stateDir, "upgrade.log"), `${new Date().toISOString()} ${message}\n`);
  };
}

function isInstall(dir) {
  return existsSync(join(dir, APP_EXE)) && existsSync(join(dir, UNINSTALLER));
}

/**
 * Whether a restore may replace `dir`: it is missing, empty (the folder an installer was handed),
 * or Ghost's own, including the partial install a failed installer leaves. A folder with only
 * other things in it is somebody else's and stays.
 */
function replaceable(dir) {
  if (!existsSync(dir)) return true;
  const entries = readdirSync(dir).map((name) => name.toLowerCase());
  return entries.length === 0 || entries.includes(APP_EXE) || entries.includes(UNINSTALLER);
}

function snapshots(stateDir) {
  const root = join(stateDir, "snapshots");
  if (!existsSync(root)) return [];
  return readdirSync(root)
    .filter((name) => existsSync(join(root, name, "snapshot.json")))
    .sort();
}

function readSnapshot(stateDir, name) {
  const dir = join(stateDir, "snapshots", name);
  return { dir, ...JSON.parse(readFileSync(join(dir, "snapshot.json"), "utf8")) };
}

/**
 * Moves `from` to `to`, or copies it when they are on different drives. Moving keeps the very
 * files: Smart App Control checks a copied executable again and may block it, even when it allowed
 * the original.
 */
function move(from, to) {
  // Windows can hold a just-closed app's files for a moment, so a busy move is tried again.
  for (let attempt = 1; ; attempt++) {
    try {
      renameSync(from, to);
      return;
    } catch (error) {
      if (error.code === "EXDEV") {
        cpSync(from, to, { recursive: true });
        rmSync(from, { recursive: true, force: true });
        return;
      }
      if (!["EPERM", "EBUSY", "EACCES"].includes(error.code) || attempt >= 20) throw error;
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 500);
    }
  }
}

/** The SHA-256 of a file, in hex. */
function fileHash(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

/**
 * Keeps the installed app aside, with when and what it was, and leaves an empty install folder
 * for the installer. The original files move into the snapshot, so a restore puts back files
 * Windows already allowed. Nothing is copied back first: an installer that finds a binary in
 * place can leave it there and still exit 0 (seen on 2026-11-01), while one that finds an empty
 * folder either writes its binary or leaves none, which `install` checks.
 */
export function takeSnapshot(paths, installer, now) {
  if (!isInstall(paths.installDir)) {
    throw new Error(`${paths.installDir} holds no installed Ghost to keep.`);
  }
  let name = stampOf(now);
  for (let n = 2; existsSync(join(paths.stateDir, "snapshots", name)); n++) {
    name = `${stampOf(now)}-${n}`;
  }
  const dir = join(paths.stateDir, "snapshots", name);
  const app = join(dir, "app");
  mkdirSync(dir, { recursive: true });
  // A file in use stops the move before anything changes.
  move(paths.installDir, app);
  mkdirSync(paths.installDir, { recursive: true });
  const exe = statSync(join(app, APP_EXE));
  const record = {
    name,
    created: now.getTime(),
    installDir: paths.installDir,
    dataDir: paths.dataDir,
    replacedBy: installer,
    app: {
      bytes: exe.size,
      modified: exe.mtime.toISOString(),
      sha256: fileHash(join(app, APP_EXE)),
    },
  };
  writeFileSync(join(dir, "snapshot.json"), `${JSON.stringify(record, null, 2)}\n`);
  return { dir, ...record };
}

/**
 * Puts a snapshot's app back in place. The current files move aside first (a file in use stops the
 * move before anything changes) and come back if the restore fails. The snapshot's files move,
 * not copy (see `move`), and the snapshot keeps a copy so it can be restored again.
 */
export function restoreApp(snapshot, installDir, now = new Date()) {
  const app = join(snapshot.dir, "app");
  if (!isInstall(app)) throw new Error(`${snapshot.dir} holds no copy of the app.`);
  if (!replaceable(installDir)) {
    throw new Error(`${installDir} is not a Ghost install; nothing was changed.`);
  }
  const aside = `${installDir}.replaced-${now.getTime()}`;
  if (existsSync(installDir)) renameSync(installDir, aside);
  try {
    move(app, installDir);
  } catch (error) {
    rmSync(installDir, { recursive: true, force: true });
    if (existsSync(aside)) renameSync(aside, installDir);
    throw error;
  }
  rmSync(aside, { recursive: true, force: true });
  cpSync(installDir, app, { recursive: true });
}

/** Puts each migrated database back as it was before the migration; the migrated one is kept. */
export function restoreDatabases(dataDir, sinceMs, now) {
  const restores = backupsSince(join(dataDir, "profiles"), sinceMs);
  for (const restore of restores) {
    const aside = join(
      dirname(restore.backup),
      `${basename(restore.database)}.rolled-back-${now.getTime()}`,
    );
    for (const suffix of SIDECARS) {
      if (existsSync(restore.database + suffix)) {
        renameSync(restore.database + suffix, aside + suffix);
      }
    }
    copyFileSync(restore.backup, restore.database);
    restore.kept = aside;
  }
  return restores;
}

async function waitForExit(system, paths, minutes, log) {
  const deadline = Date.now() + minutes * 60_000;
  let announced = false;
  for (;;) {
    const { app, helpers } = hostProcesses(system.processes(), paths.installDir);
    if (app.length === 0) {
      if (helpers.length === 0) return true;
      // Helpers end with their agent; one left behind would keep the exe locked.
      await system.sleep(5_000);
      for (const helper of hostProcesses(system.processes(), paths.installDir).helpers) {
        log(`Stopping a project-memory helper left behind (PID ${helper.ProcessId}).`);
        system.stop(helper.ProcessId);
      }
      return true;
    }
    if (Date.now() >= deadline) return false;
    if (!announced && minutes > 0) {
      log(`Waiting up to ${minutes} minutes for Ghost to quit.`);
      announced = true;
    }
    if (minutes === 0) return false;
    await system.sleep(2_000);
  }
}

async function healthy(system, installDir, seconds) {
  await system.sleep(seconds * 1000);
  return hostProcesses(system.processes(), installDir).app.length > 0;
}

async function launchApp(system, exe, description, log) {
  try {
    await system.launch(exe);
    return true;
  } catch (error) {
    log(`The ${description} could not start: ${error.message}`);
    return false;
  }
}

function prune(stateDir, keep, log) {
  for (const name of toPrune(snapshots(stateDir), keep)) {
    rmSync(join(stateDir, "snapshots", name), { recursive: true, force: true });
    log(`Removed the old snapshot ${name}.`);
  }
}

function armedRecord(stateDir) {
  const path = join(stateDir, "armed.json");
  return existsSync(path) ? { path, ...JSON.parse(readFileSync(path, "utf8")) } : undefined;
}

function armedAlive(system, record) {
  return system
    .processes()
    .some(
      (process) =>
        process.ProcessId === record.pid && (process.CommandLine ?? "").includes(basename(SCRIPT)),
    );
}

export async function install(options, paths, system, log) {
  const installer = options.installer
    ? resolve(options.installer)
    : newestInstaller([process.env.CARGO_TARGET_DIR, join(REPO, "target")]);
  if (!installer || !existsSync(installer)) {
    log(
      installer
        ? `There is no installer at ${installer}.`
        : "No installer was found under target\\**\\release\\bundle\\nsis; build one with `pnpm bundle` or pass --installer.",
    );
    return EXIT.usage;
  }
  const keep = options.keep;

  if (options.afterExit) {
    const armed = armedRecord(paths.stateDir);
    if (armed && armedAlive(system, armed)) {
      log(`An install is already armed (PID ${armed.pid}); run cancel first.`);
      return EXIT.running;
    }
    // A later bundle build, or the removal of the worktree this runs from, must not change what
    // gets installed or stop the waiter: both run from copies. The script needs only Node.
    const pending = join(paths.stateDir, "pending");
    mkdirSync(pending, { recursive: true });
    const staged = join(pending, `${stampOf(system.now())}-${basename(installer)}`);
    copyFileSync(installer, staged);
    const runner = join(paths.stateDir, basename(SCRIPT));
    copyFileSync(SCRIPT, runner);
    const args = [
      "install",
      "--installer",
      staged,
      "--wait",
      String(options.wait),
      "--keep",
      String(keep),
      "--install-dir",
      paths.installDir,
      "--data-dir",
      paths.dataDir,
      "--state-dir",
      paths.stateDir,
    ];
    const pid = system.startOutsideJob(
      waiterCommandLine(process.execPath, runner, args),
      paths.stateDir,
    );
    writeFileSync(
      join(paths.stateDir, "armed.json"),
      `${JSON.stringify({ pid, installer: staged, armed: system.now().toISOString() }, null, 2)}\n`,
    );
    log(
      `Armed (PID ${pid}): ${basename(installer)} installs when Ghost quits, within ${options.wait} minutes. Quit Ghost to start it; it opens again by itself. Log: ${join(paths.stateDir, "upgrade.log")}`,
    );
    return EXIT.done;
  }

  if (!(await waitForExit(system, paths, options.wait, log))) {
    log(
      options.wait > 0
        ? `Ghost did not quit within ${options.wait} minutes; nothing was installed.`
        : "Ghost is running. Quit it first, or arm the install with --after-exit.",
    );
    return EXIT.running;
  }
  const snapshot = takeSnapshot(paths, installer, system.now());
  log(`Kept the installed app as snapshot ${snapshot.name}.`);
  log(`Installing ${installer}.`);
  const code = system.install(installer, log);
  const exe = join(paths.installDir, APP_EXE);
  if (code !== 0) {
    log(`The installer exited with code ${code}; restoring snapshot ${snapshot.name}.`);
    restoreApp(snapshot, paths.installDir, system.now());
    await launchApp(system, exe, "restored app", log);
    return EXIT.failed;
  }
  // The exit code alone is not proof: an installer can exit 0 without writing the binary.
  if (!existsSync(exe)) {
    log(
      `The installer exited with code 0 but left no ${APP_EXE} in ${paths.installDir}; restoring snapshot ${snapshot.name}.`,
    );
    restoreApp(snapshot, paths.installDir, system.now());
    await launchApp(system, exe, "restored app", log);
    return EXIT.failed;
  }
  const hash = fileHash(exe);
  const same = hash === snapshot.app.sha256 ? ", the same build as before" : "";
  log(
    `Installed ${APP_EXE}: ${statSync(exe).size} bytes, sha256 ${hash.slice(0, 16)} (previous ${snapshot.app.sha256.slice(0, 16)}${same}).`,
  );
  if (!(await launchApp(system, exe, "new build", log))) {
    log(`Restoring snapshot ${snapshot.name} after the launch failed.`);
    restoreApp(snapshot, paths.installDir, system.now());
    await launchApp(system, exe, "restored app", log);
    return EXIT.failed;
  }
  if (!(await healthy(system, paths.installDir, options.healthSeconds))) {
    log(
      `The new build did not stay up for ${options.healthSeconds} seconds; restoring snapshot ${snapshot.name}. If the restored app reports a newer database, run: rollback --snapshot ${snapshot.name} --restore-database`,
    );
    restoreApp(snapshot, paths.installDir, system.now());
    await launchApp(system, exe, "restored app", log);
    return EXIT.failed;
  }
  log(`Installed and running. Undo with: rollback --snapshot ${snapshot.name}`);
  prune(paths.stateDir, keep, log);
  return EXIT.done;
}

export async function rollback(options, paths, system, log) {
  const names = snapshots(paths.stateDir);
  const name = options.snapshot ?? names.at(-1);
  if (!name || !names.includes(name)) {
    log(name ? `There is no snapshot named ${name}.` : "There is no snapshot to restore.");
    return EXIT.usage;
  }
  if (hostProcesses(system.processes(), paths.installDir).app.length > 0) {
    log("Ghost is running. Quit it first; nothing was changed.");
    return EXIT.running;
  }
  const snapshot = readSnapshot(paths.stateDir, name);
  restoreApp(snapshot, paths.installDir, system.now());
  log(`Restored the app from snapshot ${name}.`);
  if (options.restoreDatabase) {
    const restored = restoreDatabases(paths.dataDir, snapshot.created, system.now());
    if (restored.length === 0) {
      log("No database was migrated since that snapshot; the data is unchanged.");
    }
    for (const restore of restored) {
      log(
        `Restored ${restore.database} to schema ${restore.schema}; the migrated copy is kept as ${restore.kept}.`,
      );
    }
  }
  return (await launchApp(system, join(paths.installDir, APP_EXE), "restored app", log))
    ? EXIT.done
    : EXIT.failed;
}

export function status(paths, system, print = console.log) {
  const { app, helpers } = hostProcesses(system.processes(), paths.installDir);
  const exe = join(paths.installDir, APP_EXE);
  print(`Installed app: ${existsSync(exe) ? exe : "none"}`);
  if (existsSync(exe)) {
    const info = statSync(exe);
    print(`  built ${info.mtime.toISOString()}, ${info.size} bytes`);
  }
  print(
    `Running: ${app.length ? app.map((p) => p.ProcessId).join(", ") : "no"}${helpers.length ? ` (and ${helpers.length} project-memory helpers)` : ""}`,
  );
  const armed = armedRecord(paths.stateDir);
  print(
    `Armed install: ${armed && armedAlive(system, armed) ? `PID ${armed.pid}, ${armed.installer}` : "none"}`,
  );
  const kept = snapshots(paths.stateDir);
  print(`Snapshots: ${kept.length ? kept.join(", ") : "none"}`);
  const log = join(paths.stateDir, "upgrade.log");
  if (existsSync(log)) {
    print("Last log lines:");
    for (const line of readFileSync(log, "utf8").trimEnd().split("\n").slice(-8)) {
      print(`  ${line}`);
    }
  }
  return EXIT.done;
}

export function cancel(paths, system, log) {
  const armed = armedRecord(paths.stateDir);
  if (!armed || !armedAlive(system, armed)) {
    log("No install is armed.");
    if (armed) rmSync(armed.path, { force: true });
    return EXIT.done;
  }
  system.stop(armed.pid);
  rmSync(armed.path, { force: true });
  log(`Cancelled the armed install (PID ${armed.pid}).`);
  return EXIT.done;
}

export async function main(argv, env = process.env, system = windows) {
  if (process.platform === "linux" && system === windows) {
    // The Linux install is an AppImage behind a link: its own script (appimage-upgrade.mjs).
    const linux = await import("./appimage-upgrade.mjs");
    return linux.main(argv, env);
  }
  let parsed;
  try {
    parsed = parseArgs({
      args: argv,
      allowPositionals: true,
      options: {
        installer: { type: "string" },
        "after-exit": { type: "boolean", default: false },
        wait: { type: "string" },
        keep: { type: "string", default: "3" },
        "health-seconds": { type: "string", default: "20" },
        snapshot: { type: "string" },
        "restore-database": { type: "boolean", default: false },
        "install-dir": { type: "string" },
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
  if (process.platform !== "win32" && system === windows) {
    console.error("host-upgrade replaces an NSIS install on Windows or an AppImage on Linux.");
    return EXIT.usage;
  }
  const number = (text, name) => {
    const value = Number(text);
    if (!Number.isInteger(value) || value < 0) throw new Error(`--${name} must be a whole number.`);
    return value;
  };
  let paths;
  let options;
  try {
    paths = folders(env, {
      installDir: values["install-dir"],
      dataDir: values["data-dir"],
      stateDir: values["state-dir"],
    });
    options = {
      installer: values.installer,
      afterExit: values["after-exit"],
      wait: number(values.wait ?? (values["after-exit"] ? "30" : "0"), "wait"),
      keep: Math.max(1, number(values.keep, "keep")),
      healthSeconds: number(values["health-seconds"], "health-seconds"),
      snapshot: values.snapshot,
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
        return await install(options, paths, system, log);
      case "rollback":
        return await rollback(options, paths, system, log);
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
  } finally {
    if (command === "install" && !options.afterExit) {
      const armed = armedRecord(paths.stateDir);
      if (armed && armed.pid === process.pid) rmSync(armed.path, { force: true });
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT) {
  // Not a top-level await: on Linux `main` imports appimage-upgrade.mjs, which imports this module,
  // and that import would wait forever on a module still awaiting.
  main(process.argv.slice(2)).then((code) => {
    process.exitCode = code;
  });
}
