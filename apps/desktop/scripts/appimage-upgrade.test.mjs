import assert from "node:assert/strict";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  realpathSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { afterEach, test } from "node:test";
import {
  activate,
  buildOf,
  cancel,
  folders,
  install,
  isAppImage,
  launchEnv,
  linkTarget,
  newestAppImage,
  rollback,
  stagedName,
  status,
  waiter,
} from "./appimage-upgrade.mjs";
import { EXIT } from "./host-upgrade.mjs";

const scratch = [];
afterEach(() => {
  for (const dir of scratch.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function temp() {
  const dir = mkdtempSync(join(tmpdir(), "aura-appimage-upgrade-"));
  scratch.push(dir);
  return dir;
}

/** A file with an AppImage's header and `body` after it. */
function appImage(path, body = "build") {
  mkdirSync(dirname(path), { recursive: true });
  const head = Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0, 0x41, 0x49, 0x02, 0, 0, 0, 0, 0]);
  writeFileSync(path, Buffer.concat([head, Buffer.from(body)]));
  return path;
}

/**
 * A system whose "running" builds are a set the test controls; `starts` decides whether a
 * started build stays up.
 */
function fakeSystem({
  running = [],
  starts = () => true,
  at = new Date(2026, 10, 16, 17, 0, 0),
} = {}) {
  const system = {
    live: new Set(running),
    launched: [],
    detached: [],
    stopped: [],
    clock: at.getTime(),
    running(dir) {
      return [...system.live]
        .filter((exe) => dirname(exe) === dir)
        .map((exe, index) => ({ pid: 100 + index, exe }));
    },
    detach(name, program, args, env) {
      system.detached.push({ name, program, args, env });
      if (name === "auracode") {
        const target = linkTarget(program);
        system.launched.push(target);
        if (starts(target)) system.live.add(target);
        return { unit: `${name}-1` };
      }
      return { unit: `${name}-1` };
    },
    alive: (handle) => Boolean(handle?.unit) && !system.stopped.includes(handle.unit),
    stop: (handle) => system.stopped.push(handle.unit),
    now: () => new Date(system.clock),
    sleep: async (ms) => {
      system.clock += ms;
    },
  };
  return system;
}

function setup() {
  const root = temp();
  const paths = {
    link: join(root, "Applications", "o7.AppImage"),
    dataDir: join(root, "data"),
    stateDir: join(root, "state"),
  };
  const installed = appImage(join(root, "Applications", "o7_0.1.0_amd64.AppImage"), "old");
  symlinkSync(basename(installed), paths.link);
  const built = appImage(
    join(root, "repo", "target", "release", "bundle", "appimage", "o7_0.1.0_amd64.AppImage"),
    "new",
  );
  return { root, paths, installed, built };
}

const options = (extra = {}) => ({
  wait: 30,
  keep: 3,
  healthSeconds: 20,
  afterExit: false,
  restoreDatabase: false,
  ...extra,
});
const quiet = () => {};

test("folders follow the XDG variables and the overrides", () => {
  const paths = folders({ HOME: "/home/me" });
  assert.equal(paths.link, "/home/me/Applications/o7.AppImage");
  assert.equal(paths.dataDir, "/home/me/.local/share/codes.o7.desktop");
  assert.equal(paths.stateDir, "/home/me/.local/state/o7-upgrades");
  const xdg = folders(
    { HOME: "/home/me", XDG_DATA_HOME: "/d", XDG_STATE_HOME: "/s" },
    { link: "/x/a.AppImage" },
  );
  assert.deepEqual(xdg, {
    link: "/x/a.AppImage",
    dataDir: "/d/codes.o7.desktop",
    stateDir: "/s/o7-upgrades",
  });
});

test("only a type 2 AppImage passes the header check", () => {
  const dir = temp();
  assert.equal(isAppImage(appImage(join(dir, "a.AppImage"))), true);
  writeFileSync(join(dir, "elf"), Buffer.from([0x7f, 0x45, 0x4c, 0x46, 2, 1, 1, 0, 0, 0, 0, 0]));
  assert.equal(isAppImage(join(dir, "elf")), false);
  writeFileSync(join(dir, "short"), "AI");
  assert.equal(isAppImage(join(dir, "short")), false);
  assert.equal(isAppImage(join(dir, "missing")), false);
});

test("the newest AppImage under a target folder is found, with or without a triple", () => {
  const dir = temp();
  const older = appImage(join(dir, "release", "bundle", "appimage", "o7_0.1.0_amd64.AppImage"));
  const newer = appImage(
    join(
      dir,
      "x86_64-unknown-linux-gnu",
      "release",
      "bundle",
      "appimage",
      "o7_0.1.1_amd64.AppImage",
    ),
  );
  utimesSync(older, new Date(2026, 0, 1), new Date(2026, 0, 1));
  assert.equal(newestAppImage([undefined, join(dir, "nope"), dir]), newer);
  assert.equal(newestAppImage([join(dir, "nope")]), undefined);
});

test("a staged build keeps its name with the time added", () => {
  const at = new Date(2026, 10, 16, 9, 5, 7);
  assert.equal(
    stagedName("/t/o7_0.1.0_amd64.AppImage", at),
    "o7_0.1.0_amd64_20261116-090507.AppImage",
  );
  assert.equal(stagedName("/t/other.AppImage", at), "o7_other_20261116-090507.AppImage");
});

test("the launch env loses an agent session and an AppImage launcher, and keeps the rest", () => {
  const env = launchEnv({
    HOME: "/home/me",
    WAYLAND_DISPLAY: "wayland-1",
    CLAUDECODE: "1",
    O7_MCP_0_env_1: "secret",
    APPDIR: "/tmp/.mount_x",
    APPIMAGE: "/a.AppImage",
    LD_LIBRARY_PATH: "/tmp/.mount_x/usr/lib/:",
    PATH: "/tmp/.mount_x/usr/bin/:/usr/bin:/usr/bin",
    GTK_DATA_PREFIX: "/tmp/.mount_x",
    GTK_THEME: "Adwaita:dark",
  });
  assert.deepEqual(env, { HOME: "/home/me", WAYLAND_DISPLAY: "wayland-1", PATH: "/usr/bin" });
});

test("install refuses while o7 runs, unless armed", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem({ running: [installed] });
  assert.equal(await install(options({ appimage: built }), paths, system, quiet, {}), EXIT.running);
  assert.equal(linkTarget(paths.link), installed);
  assert.equal(readdirSync(dirname(paths.link)).length, 2, "nothing staged");
});

test("install refuses what is not an AppImage", async () => {
  const { root, paths, installed } = setup();
  writeFileSync(join(root, "setup.exe"), "MZ");
  const system = fakeSystem();
  assert.equal(
    await install(options({ appimage: join(root, "setup.exe") }), paths, system, quiet, {}),
    EXIT.usage,
  );
  assert.equal(linkTarget(paths.link), installed);
});

test("an install moves the link, starts the new build and keeps the previous one", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem();
  const lines = [];
  assert.equal(
    await install(options({ appimage: built }), paths, system, (l) => lines.push(l), {}),
    EXIT.done,
  );
  const current = linkTarget(paths.link);
  assert.notEqual(current, installed);
  assert.equal(readFileSync(current, "utf8").endsWith("new"), true);
  assert.equal(existsSync(installed), true, "the previous build stays");
  assert.deepEqual(system.launched, [current]);
  const history = JSON.parse(readFileSync(join(paths.stateDir, "history.json"), "utf8"));
  assert.equal(history.at(-1).from, installed);
  assert.equal(history.at(-1).to, current);
  assert.ok(lines.some((line) => line.includes("is running from")));
});

test("a build that does not stay up puts the previous one back and starts it", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem({ starts: (file) => file === installed });
  assert.equal(await install(options({ appimage: built }), paths, system, quiet, {}), EXIT.failed);
  assert.equal(linkTarget(paths.link), installed);
  assert.equal(system.launched.length, 2);
  assert.equal(system.launched[1], installed);
});

test("an installed file that is not a link is kept as a build", async () => {
  const { paths, installed, built } = setup();
  rmSync(paths.link);
  appImage(paths.link, "plain");
  rmSync(installed);
  const system = fakeSystem();
  const staged = join(dirname(paths.link), "auracode_new.AppImage");
  appImage(staged, "new");
  assert.equal(await activate(staged, options(), paths, system, quiet, {}), EXIT.done);
  assert.equal(linkTarget(paths.link), staged);
  const kept = readdirSync(dirname(paths.link)).filter((n) => n.startsWith("auracode_previous_"));
  assert.equal(kept.length, 1);
  assert.equal(readFileSync(join(dirname(paths.link), kept[0]), "utf8").endsWith("plain"), true);
  void built;
});

test("an armed install stages at once, and the waiter installs when o7 quits", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem({ running: [installed] });
  assert.equal(
    await install(options({ appimage: built, afterExit: true }), paths, system, quiet, {}),
    EXIT.done,
  );
  assert.equal(linkTarget(paths.link), installed, "the link waits");
  const armed = JSON.parse(readFileSync(join(paths.stateDir, "armed.json"), "utf8"));
  assert.equal(existsSync(armed.staged), true, "staged before the worktree can go");
  assert.deepEqual(armed.handle, { unit: "auracode-upgrade-1" });
  const waiterCall = system.detached.find((call) => call.name === "auracode-upgrade");
  assert.ok(waiterCall.args.includes("waiter"));
  // A second arm is refused while the first waits.
  assert.equal(
    await install(options({ appimage: built, afterExit: true }), paths, system, quiet, {}),
    EXIT.running,
  );
  // The person quits after a while.
  let polls = 0;
  const sleep = system.sleep;
  system.sleep = async (ms) => {
    if (++polls === 3) system.live.clear();
    await sleep(ms);
  };
  assert.equal(await waiter(options(), paths, system, quiet, {}), EXIT.done);
  assert.equal(linkTarget(paths.link), armed.staged);
  assert.equal(existsSync(join(paths.stateDir, "armed.json")), false);
});

test("a waiter gives up when o7 does not quit in time", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem({ running: [installed] });
  await install(options({ appimage: built, afterExit: true, wait: 1 }), paths, system, quiet, {});
  assert.equal(await waiter(options({ wait: 1 }), paths, system, quiet, {}), EXIT.running);
  assert.equal(linkTarget(paths.link), installed);
  assert.equal(existsSync(join(paths.stateDir, "armed.json")), false);
});

test("cancel stops the armed waiter", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem({ running: [installed] });
  await install(options({ appimage: built, afterExit: true }), paths, system, quiet, {});
  assert.equal(cancel(paths, system, quiet), EXIT.done);
  assert.deepEqual(system.stopped, ["auracode-upgrade-1"]);
  assert.equal(existsSync(join(paths.stateDir, "armed.json")), false);
});

test("rollback goes back to the previous build and restores migrated databases", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem();
  await install(options({ appimage: built }), paths, system, quiet, {});
  const installedAt = system.clock;
  // The new build migrated a database after it was installed.
  const backups = join(paths.dataDir, "profiles", "local", "backups");
  mkdirSync(backups, { recursive: true });
  writeFileSync(join(paths.dataDir, "profiles", "local", "aura.db"), "v9");
  writeFileSync(join(backups, `aura.db.v8-${installedAt + 1000}.bak`), "v8");
  // Running: refused.
  assert.equal(await rollback(options(), paths, system, quiet, {}), EXIT.running);
  system.live.clear();
  assert.equal(
    await rollback(options({ restoreDatabase: true }), paths, system, quiet, {}),
    EXIT.done,
  );
  assert.equal(linkTarget(paths.link), installed);
  assert.equal(readFileSync(join(paths.dataDir, "profiles", "local", "aura.db"), "utf8"), "v8");
  assert.equal(system.launched.at(-1), installed);
});

test("status names the link, what runs and what is kept", async () => {
  const { paths, installed, built } = setup();
  const system = fakeSystem();
  await install(options({ appimage: built }), paths, system, quiet, {});
  const lines = [];
  status(paths, system, (line) => lines.push(line));
  assert.match(lines[0], /o7\.AppImage -> .*_20261116-170000\.AppImage/u);
  assert.ok(lines.some((line) => line.startsWith("Running: 100")));
  assert.ok(lines.some((line) => line.includes(`Kept builds: ${basename(installed)}`)));
});

test("old builds beyond --keep are pruned, never the previous one", async () => {
  const { paths, installed } = setup();
  const system = fakeSystem();
  const dir = dirname(paths.link);
  for (const [index, name] of ["a", "b", "c", "d"].entries()) {
    const file = appImage(join(dir, `o7_${name}.AppImage`));
    utimesSync(file, new Date(2026, 0, 1 + index), new Date(2026, 0, 1 + index));
  }
  utimesSync(installed, new Date(2025, 0, 1), new Date(2025, 0, 1));
  const staged = appImage(join(dir, "o7_new.AppImage"), "new");
  assert.equal(await activate(staged, options({ keep: 2 }), paths, system, quiet, {}), EXIT.done);
  const left = readdirSync(dir).sort();
  // Current, the previous (oldest, but the rollback target) and the newest spare one.
  assert.deepEqual(
    left,
    ["o7.AppImage", basename(installed), "o7_d.AppImage", "o7_new.AppImage"].sort(),
  );
});

test("a build runs as its runtime or as the app inside its mount", () => {
  // As /proc names it: resolved, which macOS's temporary folder (/var, a link to /private/var) is not.
  const dir = realpathSync(temp());
  const file = appImage(join(dir, "o7_0.1.0_amd64.AppImage"));
  const env = (image) => Buffer.from(`HOME=/home/me\0APPIMAGE=${image}\0APPDIR=/tmp/.mount_x\0`);
  assert.equal(buildOf(file, undefined, dir), file);
  assert.equal(buildOf(`${file} (deleted)`, undefined, dir), file);
  // The app, still shutting down after its runtime ended.
  assert.equal(buildOf("/tmp/.mount_auracoAbC/usr/bin/auracode", env(file), dir), file);
  // Through the link the desktop entry starts.
  symlinkSync(basename(file), join(dir, "auracode.AppImage"));
  assert.equal(
    buildOf("/tmp/.mount_auracoAbC/usr/bin/auracode", env(join(dir, "auracode.AppImage")), dir),
    file,
  );
  // Another image's app, or a mount process without APPIMAGE, is not ours.
  assert.equal(buildOf("/tmp/.mount_other/usr/bin/x", env("/opt/Other.AppImage"), dir), undefined);
  assert.equal(
    buildOf("/tmp/.mount_auracoAbC/usr/bin/auracode", Buffer.from("HOME=/h\0"), dir),
    undefined,
  );
  // A shell that merely inherited APPIMAGE runs from outside any mount.
  assert.equal(buildOf("/usr/bin/bash", env(file), dir), undefined);
});
