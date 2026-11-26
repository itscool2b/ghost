import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, test } from "node:test";
import {
  backupsSince,
  cancel,
  cleanEnv,
  EXIT,
  folders,
  hostProcesses,
  install,
  newestInstaller,
  restoreApp,
  rollback,
  stampOf,
  toPrune,
  waiterCommandLine,
  windows,
} from "./host-upgrade.mjs";

const temps = [];
afterEach(() => {
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function temp() {
  const dir = mkdtempSync(join(tmpdir(), "host-upgrade-"));
  temps.push(dir);
  return dir;
}

function write(path, text) {
  mkdirSync(join(path, ".."), { recursive: true });
  writeFileSync(path, text);
}

/** An installed app, its data folder and the state folder, all under a temporary folder. */
function machine(build = "old") {
  const root = temp();
  const paths = {
    installDir: join(root, "Ghost"),
    dataDir: join(root, "si.auracode.desktop"),
    stateDir: join(root, "auracode-upgrades"),
  };
  write(join(paths.installDir, "ghost.exe"), build);
  write(join(paths.installDir, "uninstall.exe"), "uninstaller");
  write(join(paths.installDir, "resources", "notices.txt"), build);
  const installer = join(root, "bundle", "Ghost_0.1.0_x64-setup.exe");
  write(installer, "installer");
  return { root, paths, installer };
}

/**
 * A fake machine. `running` says whether the app runs; `install` stands for the installer, and
 * `launch` for starting the app (which runs unless `staysUp` is false).
 */
function fakeSystem(
  paths,
  { running = false, installCode = 0, staysUp = true, helpers = [], writes = true } = {},
) {
  const calls = [];
  const state = { running, helpers: [...helpers], times: 0, emptyAtInstall: undefined };
  const exe = join(paths.installDir, "ghost.exe");
  return {
    calls,
    state,
    processes() {
      const list = state.helpers.map((pid) => ({
        ProcessId: pid,
        ExecutablePath: exe,
        CommandLine: `"${exe}" --ghost-memory-helper --mcp`,
      }));
      if (state.running)
        list.push({ ProcessId: 100, ExecutablePath: exe, CommandLine: `"${exe}"` });
      return list.concat(state.extra ?? []);
    },
    stop(pid) {
      calls.push(["stop", pid]);
      state.helpers = state.helpers.filter((helper) => helper !== pid);
      if (state.extra) state.extra = state.extra.filter((p) => p.ProcessId !== pid);
    },
    install(installer) {
      calls.push(["install", installer]);
      // The installer gets an empty install folder, as the real one does.
      state.emptyAtInstall = existsSync(paths.installDir) && !existsSync(exe);
      if (!writes) return installCode;
      if (installCode === 0) {
        mkdirSync(join(paths.installDir, "resources"), { recursive: true });
        writeFileSync(exe, "new");
        writeFileSync(join(paths.installDir, "resources", "notices.txt"), "new");
      } else {
        writeFileSync(exe, "half");
      }
      return installCode;
    },
    launch(path) {
      calls.push(["launch", path, readFileSync(path, "utf8")]);
      state.running = staysUp || readFileSync(path, "utf8") === "old";
    },
    startOutsideJob(commandLine, cwd) {
      calls.push(["startOutsideJob", commandLine, cwd]);
      state.extra = [{ ProcessId: 4242, ExecutablePath: "node.exe", CommandLine: commandLine }];
      return 4242;
    },
    sleep: async () => {
      state.times++;
      // The app quits once, while the waiter waits.
      if (state.times === state.quitAfter) state.running = false;
    },
    now: () => new Date(2026, 10, 1, 13, 5, 54),
  };
}

const options = (extra = {}) => ({
  installer: undefined,
  afterExit: false,
  wait: 0,
  keep: 3,
  healthSeconds: 20,
  snapshot: undefined,
  restoreDatabase: false,
  ...extra,
});

function recorder() {
  const lines = [];
  const log = (line) => lines.push(line);
  log.lines = lines;
  return log;
}

// Absolute on the system running the tests (C:\Users\a\AppData\Local on Windows): the script
// resolves paths by that system's rules, and CI runs these tests on macOS.
const LOCAL = resolve("/", "Users", "a", "AppData", "Local");

test("the folders come from LOCALAPPDATA unless overridden", () => {
  const paths = folders({ LOCALAPPDATA: LOCAL });
  assert.equal(paths.installDir, join(LOCAL, "Ghost"));
  assert.equal(paths.dataDir, join(LOCAL, "dev.onghost.desktop"));
  assert.equal(paths.stateDir, join(LOCAL, "ghost-upgrades"));
  // GHOST_DATA_DIR belongs to development launches, never to the installed app here.
  assert.equal(
    folders({ LOCALAPPDATA: LOCAL, GHOST_DATA_DIR: resolve("/", "scratch") }).dataDir,
    join(LOCAL, "dev.onghost.desktop"),
  );
  assert.throws(() => folders({}), /LOCALAPPDATA/u);
});

test("only the installed app's processes count, and helpers apart", () => {
  const install = join(LOCAL, "Ghost");
  const debug = resolve("/", "src", "target", "debug", "ghost.exe");
  const { app, helpers } = hostProcesses(
    [
      { ProcessId: 1, ExecutablePath: join(install, "ghost.exe"), CommandLine: "ghost.exe" },
      {
        ProcessId: 2,
        // Windows paths ignore case.
        ExecutablePath: join(install.toUpperCase(), "AURACODE.EXE"),
        CommandLine: "ghost.exe --ghost-memory-helper --mcp",
      },
      // A development build is never the host.
      { ProcessId: 3, ExecutablePath: debug, CommandLine: "" },
      { ProcessId: 4, ExecutablePath: join(`${install}-old`, "ghost.exe"), CommandLine: "" },
      { ProcessId: 5, ExecutablePath: null, CommandLine: null },
    ],
    install,
  );
  assert.deepEqual(
    app.map((p) => p.ProcessId),
    [1],
  );
  assert.deepEqual(
    helpers.map((p) => p.ProcessId),
    [2],
  );
});

test("the newest installer wins across target layouts", () => {
  const root = temp();
  const older = join(root, "release", "bundle", "nsis", "Ghost_0.1.0_x64-setup.exe");
  const newer = join(
    root,
    "x86_64-pc-windows-msvc",
    "release",
    "bundle",
    "nsis",
    "Ghost_0.1.0_x64-setup.exe",
  );
  write(older, "a");
  write(newer, "b");
  write(join(root, "release", "bundle", "nsis", "notes.txt"), "c");
  utimesSync(older, new Date(2026, 0, 1), new Date(2026, 0, 1));
  assert.equal(newestInstaller([undefined, join(root, "missing"), root]), newer);
  assert.equal(newestInstaller([join(root, "missing")]), undefined);
});

test("snapshot names sort by time and the newest are kept", () => {
  assert.equal(stampOf(new Date(2026, 10, 1, 9, 5, 4)), "20261101-090504");
  assert.deepEqual(toPrune(["20261101-090504", "20261102-000000", "20261031-235959"], 2), [
    "20261031-235959",
  ]);
  assert.deepEqual(toPrune(["a"], 3), []);
});

test("the backup to restore is the first made after the snapshot, per database", () => {
  const profiles = join(temp(), "profiles");
  const backups = join(profiles, "local", "backups");
  mkdirSync(backups, { recursive: true });
  for (const name of [
    "aura.db.v13-1000.bak",
    "aura.db.v15-3000.bak",
    "aura.db.v18-5000.bak",
    "aura.encrypted.db.v4-4000.bak",
    "aura.db.corrupt-6000",
    "notes.txt",
  ]) {
    writeFileSync(join(backups, name), name);
  }
  mkdirSync(join(profiles, "empty"));
  const restores = backupsSince(profiles, 2000).sort((a, b) => a.backup.localeCompare(b.backup));
  assert.deepEqual(
    restores.map((r) => [r.database, r.backup, r.schema]),
    [
      [join(profiles, "local", "aura.db"), join(backups, "aura.db.v15-3000.bak"), 15],
      [
        join(profiles, "local", "aura.encrypted.db"),
        join(backups, "aura.encrypted.db.v4-4000.bak"),
        4,
      ],
    ],
  );
  assert.deepEqual(backupsSince(profiles, 9000), []);
  assert.deepEqual(backupsSince(join(profiles, "missing"), 0), []);
});

test("the relaunch and the waiter never inherit an agent session's variables", () => {
  assert.deepEqual(
    cleanEnv({
      PATH: "p",
      LOCALAPPDATA: "l",
      GHOST_DATA_DIR: "x",
      GHOST_SMOKE: "1",
      GHOST_MCP_0_env_1: "token",
      CLAUDECODE: "1",
      CLAUDE_CODE_SESSION_ID: "s",
      CLAUDE_PID: "1",
      CLAUDE_CONFIG_DIR: "kept",
    }),
    { PATH: "p", LOCALAPPDATA: "l", CLAUDE_CONFIG_DIR: "kept" },
  );
});

test("the waiter's command line quotes every argument", () => {
  assert.equal(
    waiterCommandLine("C:\\Program Files\\nodejs\\node.exe", "C:\\s\\host-upgrade.mjs", [
      "install",
      "--wait",
      "30",
    ]),
    '"C:\\Program Files\\nodejs\\node.exe" "C:\\s\\host-upgrade.mjs" "install" "--wait" "30"',
  );
  assert.throws(() => waiterCommandLine("node", "s", ['a"b']), /quote/u);
});

test("an install keeps the old app, installs, starts the new one and keeps it", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths);
  const log = recorder();
  assert.equal(await install(options({ installer }), paths, system, log), EXIT.done);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "new");
  const [name] = readdirSync(join(paths.stateDir, "snapshots"));
  assert.equal(name, "20261101-130554");
  const kept = join(paths.stateDir, "snapshots", name);
  assert.equal(readFileSync(join(kept, "app", "ghost.exe"), "utf8"), "old");
  assert.equal(readFileSync(join(kept, "app", "resources", "notices.txt"), "utf8"), "old");
  const record = JSON.parse(readFileSync(join(kept, "snapshot.json"), "utf8"));
  assert.equal(record.replacedBy, installer);
  assert.equal(record.created, new Date(2026, 10, 1, 13, 5, 54).getTime());
  assert.deepEqual(
    system.calls.map((call) => call[0]),
    ["install", "launch"],
  );
  assert.equal(system.state.emptyAtInstall, true, "the installer found an empty folder");
  assert.match(
    log.lines.join("\n"),
    /Installed ghost\.exe: 3 bytes, sha256 [0-9a-f]{16} \(previous [0-9a-f]{16}\)\./u,
  );
  assert.match(log.lines.at(-1), /Undo with: rollback --snapshot 20261101-130554/u);
});

/** The installer exited 0 and wrote its uninstaller, yet the previous binary stayed in place. */
test("an installer that exits 0 without writing the binary is undone", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths, { writes: false });
  const log = recorder();
  assert.equal(await install(options({ installer }), paths, system, log), EXIT.failed);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
  assert.match(log.lines.join("\n"), /exited with code 0 but left no ghost\.exe/u);
  assert.deepEqual(
    system.calls.map((call) => call[0]),
    ["install", "launch"],
  );
});

test("an install never runs while the app runs", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths, { running: true });
  const log = recorder();
  assert.equal(await install(options({ installer }), paths, system, log), EXIT.running);
  assert.deepEqual(system.calls, []);
  assert.equal(existsSync(join(paths.stateDir, "snapshots")), false);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
  assert.match(log.lines.at(-1), /Quit it first, or arm the install with --after-exit/u);
});

test("a waiting install starts when the app quits and stops helpers it left", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths, { running: true, helpers: [7] });
  system.state.quitAfter = 3;
  const log = recorder();
  assert.equal(await install(options({ installer, wait: 30 }), paths, system, log), EXIT.done);
  assert.deepEqual(system.calls[0], ["stop", 7]);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "new");
});

test("a waiting install gives up when the app does not quit in time", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths, { running: true });
  // A sleep that lets the wait run out at once.
  system.sleep = async () => {
    const now = Date.now;
    Date.now = () => now() + 31 * 60_000;
  };
  const realNow = Date.now;
  try {
    const log = recorder();
    assert.equal(await install(options({ installer, wait: 30 }), paths, system, log), EXIT.running);
    assert.match(log.lines.at(-1), /did not quit within 30 minutes; nothing was installed/u);
  } finally {
    Date.now = realNow;
  }
  assert.deepEqual(system.calls, []);
});

test("a build that does not stay up is replaced by the snapshot", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths, { staysUp: false });
  const log = recorder();
  assert.equal(await install(options({ installer }), paths, system, log), EXIT.failed);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
  assert.equal(readFileSync(join(paths.installDir, "resources", "notices.txt"), "utf8"), "old");
  assert.deepEqual(
    system.calls.map((call) => call.slice(0, 1).concat(call[2] ?? [])),
    [["install"], ["launch", "new"], ["launch", "old"]],
  );
  assert.match(log.lines.join("\n"), /did not stay up for 20 seconds; restoring snapshot/u);
  assert.equal(
    readdirSync(join(paths.installDir, "..")).filter((n) => n.includes(".replaced-")).length,
    0,
  );
});

test("the real launcher rejects spawn errors without an unhandled process error", async () => {
  await assert.rejects(windows.launch(join(temp(), "missing.exe")), { code: "ENOENT" });
  // Invalid arguments throw synchronously inside spawn, unlike a missing or blocked executable.
  await assert.rejects(windows.launch(null), { code: "ERR_INVALID_ARG_TYPE" });
});

for (const failure of ["throw", "reject"]) {
  test(`a new build launch ${failure} restores and starts the previous app`, async () => {
    const { paths, installer } = machine();
    const system = fakeSystem(paths);
    const originalLaunch = system.launch;
    system.launch = (path) => {
      if (readFileSync(path, "utf8") === "old") return originalLaunch(path);
      system.calls.push(["launch", path, "new"]);
      const error = new Error("new executable was blocked");
      if (failure === "throw") throw error;
      return Promise.reject(error);
    };
    const log = recorder();
    assert.equal(await install(options({ installer }), paths, system, log), EXIT.failed);
    assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
    assert.equal(readFileSync(join(paths.installDir, "resources", "notices.txt"), "utf8"), "old");
    assert.equal(system.state.running, true);
    assert.equal(system.state.times, 0, "spawn failure does not wait for a health check");
    assert.deepEqual(
      system.calls.filter(([call]) => call === "launch").map((call) => call[2]),
      ["new", "old"],
    );
    assert.match(log.lines.join("\n"), /new build could not start: new executable was blocked/u);
    assert.match(log.lines.join("\n"), /Restoring snapshot .* after the launch failed/u);
    assert.ok(!log.lines.some((line) => line.includes("Installed and running")));
  });

  test(`a restored app launch ${failure} is reported after an upgrade fails`, async () => {
    const { paths, installer } = machine();
    const system = fakeSystem(paths, { installCode: 2 });
    system.launch = (path) => {
      assert.equal(readFileSync(path, "utf8"), "old", "restore completed before the launch");
      const error = new Error("restored executable was blocked");
      if (failure === "throw") throw error;
      return Promise.reject(error);
    };
    const log = recorder();
    assert.equal(await install(options({ installer }), paths, system, log), EXIT.failed);
    assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
    assert.match(log.lines.join("\n"), /installer exited with code 2/u);
    assert.match(
      log.lines.join("\n"),
      /restored app could not start: restored executable was blocked/u,
    );
    assert.ok(!log.lines.some((line) => line.includes("Installed and running")));
  });
}

test("manual rollback reports a restored app launch failure", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths);
  await install(options({ installer }), paths, system, recorder());
  system.state.running = false;
  system.launch = async () => {
    throw new Error("restored executable was blocked");
  };
  const log = recorder();
  assert.equal(await rollback(options(), paths, system, log), EXIT.failed);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
  assert.match(
    log.lines.join("\n"),
    /restored app could not start: restored executable was blocked/u,
  );
});

test("a failed installer is undone", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths, { installCode: 2 });
  const log = recorder();
  assert.equal(await install(options({ installer }), paths, system, log), EXIT.failed);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
  assert.match(log.lines.join("\n"), /installer exited with code 2/u);
});

test("an install without an installer or an install to keep changes nothing", async () => {
  const { paths, root } = machine();
  const log = recorder();
  const missing = join(root, "nope-setup.exe");
  assert.equal(
    await install(options({ installer: missing }), paths, fakeSystem(paths), log),
    EXIT.usage,
  );
  rmSync(join(paths.installDir, "uninstall.exe"));
  await assert.rejects(
    install(
      options({ installer: join(root, "bundle", "Ghost_0.1.0_x64-setup.exe") }),
      paths,
      fakeSystem(paths),
      log,
    ),
    /holds no installed Ghost/u,
  );
});

test("arming starts one waiter outside the job, from copies", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths, { running: true });
  const log = recorder();
  assert.equal(
    await install(options({ installer, afterExit: true, wait: 30 }), paths, system, log),
    EXIT.done,
  );
  const [call] = system.calls;
  assert.equal(call[0], "startOutsideJob");
  const staged = join(paths.stateDir, "pending", "20261101-130554-Ghost_0.1.0_x64-setup.exe");
  const runner = join(paths.stateDir, "host-upgrade.mjs");
  assert.ok(existsSync(staged) && existsSync(runner));
  assert.ok(call[1].includes(`"${runner}" "install" "--installer" "${staged}" "--wait" "30"`));
  assert.ok(call[1].includes(`"--install-dir" "${paths.installDir}"`));
  assert.equal(call[2], paths.stateDir);
  assert.equal(JSON.parse(readFileSync(join(paths.stateDir, "armed.json"), "utf8")).pid, 4242);
  // Nothing was installed yet, and the app was not touched.
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");

  assert.equal(
    await install(options({ installer, afterExit: true, wait: 30 }), paths, system, log),
    EXIT.running,
  );
  assert.match(log.lines.at(-1), /already armed \(PID 4242\)/u);
  assert.equal(cancel(paths, system, log), EXIT.done);
  assert.deepEqual(system.calls.at(-1), ["stop", 4242]);
  assert.equal(existsSync(join(paths.stateDir, "armed.json")), false);
});

test("a rollback restores the app and, when asked, the migrated databases", async () => {
  const { paths, installer } = machine();
  const system = fakeSystem(paths);
  await install(options({ installer }), paths, system, recorder());
  const snapshot = new Date(2026, 10, 1, 13, 5, 54).getTime();
  const local = join(paths.dataDir, "profiles", "local");
  write(join(local, "aura.db"), "migrated");
  write(join(local, "aura.db-wal"), "migrated wal");
  write(join(local, "backups", `aura.db.v15-${snapshot - 1000}.bak`), "older");
  write(join(local, "backups", `aura.db.v18-${snapshot + 1000}.bak`), "before the new build");

  system.state.running = true;
  const log = recorder();
  assert.equal(
    await rollback(options({ restoreDatabase: true }), paths, system, log),
    EXIT.running,
  );
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "new");

  system.state.running = false;
  assert.equal(await rollback(options({ restoreDatabase: true }), paths, system, log), EXIT.done);
  assert.equal(readFileSync(join(paths.installDir, "ghost.exe"), "utf8"), "old");
  assert.equal(readFileSync(join(local, "aura.db"), "utf8"), "before the new build");
  assert.equal(existsSync(join(local, "aura.db-wal")), false);
  const kept = readdirSync(join(local, "backups")).filter((n) => n.includes(".rolled-back-"));
  assert.deepEqual(kept.sort(), [
    `aura.db.rolled-back-${snapshot}`,
    `aura.db.rolled-back-${snapshot}-wal`,
  ]);
  assert.match(log.lines.join("\n"), /Restored .*aura\.db to schema 18/u);
  assert.deepEqual(system.calls.at(-1).slice(0, 1), ["launch"]);

  assert.equal(await rollback(options({ snapshot: "nope" }), paths, system, log), EXIT.usage);
});

/**
 * Smart App Control checks a copied executable again and may block it, so a copy of an approved
 * app could fail to start after a rollback. The snapshot keeps, and a restore puts back, the very
 * files that were installed.
 */
test("the snapshot and the restore keep the original files, not copies", async () => {
  const { paths, installer } = machine();
  const id = (path) => statSync(path, { bigint: true }).ino;
  const original = id(join(paths.installDir, "ghost.exe"));
  assert.ok(original > 0n, "the file system reports file ids");
  const system = fakeSystem(paths, { staysUp: false });
  assert.equal(await install(options({ installer }), paths, system, recorder()), EXIT.failed);
  assert.equal(id(join(paths.installDir, "ghost.exe")), original, "the restored file");
  const [name] = readdirSync(join(paths.stateDir, "snapshots"));
  const kept = join(paths.stateDir, "snapshots", name, "app", "ghost.exe");
  assert.equal(readFileSync(kept, "utf8"), "old", "the snapshot can be restored again");

  const again = machine();
  const before = id(join(again.paths.installDir, "ghost.exe"));
  await install(
    options({ installer: again.installer }),
    again.paths,
    fakeSystem(again.paths),
    recorder(),
  );
  const [taken] = readdirSync(join(again.paths.stateDir, "snapshots"));
  assert.equal(id(join(again.paths.stateDir, "snapshots", taken, "app", "ghost.exe")), before);
});

test("a rollback never deletes a folder that is not an install", () => {
  const { paths, root } = machine();
  const snapshot = { dir: join(root, "snap") };
  write(join(snapshot.dir, "app", "ghost.exe"), "old");
  write(join(snapshot.dir, "app", "uninstall.exe"), "u");
  const elsewhere = join(root, "Documents");
  write(join(elsewhere, "thesis.docx"), "keep me");
  assert.throws(() => restoreApp(snapshot, elsewhere), /not a Ghost install/u);
  assert.equal(readFileSync(join(elsewhere, "thesis.docx"), "utf8"), "keep me");
  assert.throws(() => restoreApp({ dir: join(root, "empty") }, paths.installDir), /no copy/u);
});

test("a machine updated from an earlier name keeps its data folder and its kept upgrades", () => {
  const local = mkdtempSync(join(tmpdir(), "ghost-folders-"));
  try {
    // Only empty folders under the later names, as the platform may make them: the oldest data
    // is still the one.
    mkdirSync(join(local, "dev.onghost.desktop", "EBWebView"), { recursive: true });
    mkdirSync(join(local, "codes.o7.desktop", "EBWebView"), { recursive: true });
    mkdirSync(join(local, "si.auracode.desktop", "profiles"), { recursive: true });
    mkdirSync(join(local, "auracode-upgrades"), { recursive: true });
    const paths = folders({ LOCALAPPDATA: local });
    assert.equal(paths.dataDir, join(local, "si.auracode.desktop"));
    assert.equal(paths.stateDir, join(local, "auracode-upgrades"));
    assert.equal(paths.installDir, join(local, "Ghost"));
    // o7's names come before auracode's, as they did while the app was o7.
    writeFileSync(join(local, "codes.o7.desktop", "device.json"), "{}");
    mkdirSync(join(local, "o7-upgrades"));
    const then = folders({ LOCALAPPDATA: local });
    assert.equal(then.dataDir, join(local, "codes.o7.desktop"));
    assert.equal(then.stateDir, join(local, "o7-upgrades"));
    // Once the new names are in use, they win.
    writeFileSync(join(local, "dev.onghost.desktop", "device.json"), "{}");
    mkdirSync(join(local, "ghost-upgrades"));
    const now = folders({ LOCALAPPDATA: local });
    assert.equal(now.dataDir, join(local, "dev.onghost.desktop"));
    assert.equal(now.stateDir, join(local, "ghost-upgrades"));
  } finally {
    rmSync(local, { recursive: true, force: true });
  }
});
