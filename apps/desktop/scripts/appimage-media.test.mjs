import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, test } from "node:test";
import {
  builtImages,
  bundleNames,
  bundlesAppImage,
  checkTree,
  findPluginsDir,
  findScannerDir,
  imageDir,
  missingMessage,
  OPTIONAL_PLUGINS,
  prepareMedia,
  REQUIRED_PLUGINS,
  stagePlugins,
  takeWithoutMedia,
} from "./appimage-media.mjs";

/** Tests that compare Linux paths as text are skipped on Windows. */
const LINUX_ONLY =
  process.platform === "win32" &&
  "Linux paths: node:path writes Windows paths here, and the AppImage scripts run only on Linux";

const scratch = [];
afterEach(() => {
  for (const dir of scratch.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function folder() {
  const dir = mkdtempSync(join(tmpdir(), "auracode-media-test-"));
  scratch.push(dir);
  return dir;
}

function plugins(dir, names) {
  mkdirSync(dir, { recursive: true });
  for (const name of names) writeFileSync(join(dir, `libgst${name}.so`), "");
}

/** A system with GStreamer's base and good plugins (and some it must never carry), Arch's layout. */
function system({ without = [] } = {}) {
  const root = folder();
  const dir = join(root, "usr/lib/gstreamer-1.0");
  plugins(
    dir,
    [
      ...REQUIRED_PLUGINS,
      "opus",
      "vpx",
      "pulseaudio",
      "isomp4",
      "x264",
      "libav",
      "faad",
      "dav1d",
    ].filter((name) => !without.includes(name)),
  );
  writeFileSync(join(dir, "gst-plugin-scanner"), "");
  writeFileSync(join(dir, "gst-ptp-helper"), "");
  const bin = join(root, "bin");
  mkdirSync(bin);
  writeFileSync(join(bin, "patchelf"), "");
  return { root, dir, path: bin };
}

test("an AppImage is built when the bundles name it or the config's targets apply", () => {
  assert.equal(bundleNames(["--target", "x86_64-unknown-linux-gnu"]), null);
  assert.deepEqual(bundleNames(["--bundles", "deb,rpm,appimage"]), ["deb", "rpm", "appimage"]);
  assert.deepEqual(bundleNames(["-b", "deb", "appimage", "--debug"]), ["deb", "appimage"]);
  assert.deepEqual(bundleNames(["--bundles=appimage"]), ["appimage"]);
  assert.equal(bundlesAppImage(["--bundles", "deb,rpm,appimage"], "linux"), true);
  assert.equal(bundlesAppImage([], "linux"), true, "the configured targets include it");
  assert.equal(bundlesAppImage(["--bundles", "deb"], "linux"), false);
  assert.equal(bundlesAppImage(["--no-bundle"], "linux"), false);
  assert.equal(bundlesAppImage([], "darwin"), false);
  assert.equal(bundlesAppImage(["--bundles", "appimage"], "win32"), false);
});

test("--without-media is the bundle script's own flag, never Tauri's", () => {
  assert.deepEqual(takeWithoutMedia(["--bundles", "appimage", "--without-media"]), {
    args: ["--bundles", "appimage"],
    withoutMedia: true,
  });
  assert.deepEqual(takeWithoutMedia(["--debug"]), { args: ["--debug"], withoutMedia: false });
});

test("the plugin folder is the system's, or the one named", { skip: LINUX_ONLY }, () => {
  const root = folder();
  assert.equal(findPluginsDir({}, root), null);
  mkdirSync(join(root, "usr/lib/gstreamer-1.0"), { recursive: true });
  assert.equal(findPluginsDir({}, root), join(root, "usr/lib/gstreamer-1.0"));
  mkdirSync(join(root, "usr/lib64/gstreamer-1.0"), { recursive: true });
  assert.equal(findPluginsDir({}, root), join(root, "usr/lib64/gstreamer-1.0"), "Fedora's");
  assert.equal(findPluginsDir({ GSTREAMER_PLUGINS_DIR: "/opt/gst" }, root), "/opt/gst");
  // The scanner lives beside the plugins on Arch, in libexec on Fedora.
  assert.equal(findScannerDir({}, root), null);
  writeFileSync(join(root, "usr/lib/gstreamer-1.0/gst-plugin-scanner"), "");
  assert.equal(findScannerDir({}, root), join(root, "usr/lib/gstreamer-1.0"));
  mkdirSync(join(root, "usr/libexec/gstreamer-1.0"), { recursive: true });
  writeFileSync(join(root, "usr/libexec/gstreamer-1.0/gst-plugin-scanner"), "");
  assert.equal(findScannerDir({}, root), join(root, "usr/libexec/gstreamer-1.0"));
});

test("only the allowlist is staged: base and good plugins, never ugly, bad or libav", () => {
  const { dir } = system();
  const stage = folder();
  const staged = stagePlugins({ pluginsDir: dir, scannerDir: dir, stage });
  assert.deepEqual(staged.missing, []);
  assert.equal(staged.scanner, true);
  assert.deepEqual(readdirSync(join(stage, "helpers")), ["gst-plugin-scanner"]);
  const files = readdirSync(join(stage, "plugins")).sort();
  assert.deepEqual(
    files,
    [...REQUIRED_PLUGINS, "opus", "vpx", "pulseaudio", "isomp4"]
      .map((name) => `libgst${name}.so`)
      .sort(),
  );
  for (const name of ["x264", "libav", "faad", "dav1d"]) {
    assert.ok(!OPTIONAL_PLUGINS.includes(name), name);
    assert.ok(!files.includes(`libgst${name}.so`), name);
  }
});

test("a machine without the good plugins is told what to install on each distribution", () => {
  const { dir } = system({ without: ["autodetect"] });
  const staged = stagePlugins({ pluginsDir: dir, scannerDir: null, stage: folder() });
  assert.deepEqual(staged.missing, ["autodetect"]);
  const message = missingMessage({ missing: staged.missing, patchelf: false });
  assert.match(message, /missing: autodetect/);
  assert.match(message, /patchelf/);
  assert.match(message, /pacman -S --needed gst-plugins-base gst-plugins-good patchelf/);
  assert.match(message, /apt install gstreamer1\.0-plugins-base gstreamer1\.0-plugins-good/);
  assert.match(message, /dnf install gstreamer1-plugins-base gstreamer1-plugins-good/);
  assert.match(message, /--without-media/);
});

test("an AppImage build stages the plugins for the bundler, or refuses with what is missing", () => {
  const silence = [console.log, console.warn];
  console.log = console.warn = () => {};
  try {
    const { root, path } = system();
    const ready = prepareMedia(
      ["--bundles", "appimage"],
      { PATH: path },
      { platform: "linux", root },
    );
    assert.equal(ready.appimage, true);
    assert.equal(ready.withMedia, true);
    assert.deepEqual(ready.args, ["--bundles", "appimage"]);
    const files = readdirSync(ready.env.GSTREAMER_PLUGINS_DIR);
    assert.ok(files.includes("libgstautodetect.so"));
    assert.deepEqual(readdirSync(ready.env.GSTREAMER_HELPERS_DIR), ["gst-plugin-scanner"]);
    ready.cleanup();
    assert.throws(() => readdirSync(ready.env.GSTREAMER_PLUGINS_DIR), /ENOENT/);

    const bare = system({ without: ["autodetect"] });
    assert.throws(
      () => prepareMedia([], { PATH: bare.path }, { platform: "linux", root: bare.root }),
      /lacks GStreamer's base and good plugins \(missing: autodetect\)/,
    );
    assert.throws(
      () => prepareMedia([], { PATH: folder() }, { platform: "linux", root }),
      /lacks patchelf/,
    );

    // On purpose, without: Tauri is told not to bundle the media framework, and nothing is staged.
    const without = prepareMedia(
      ["--bundles", "appimage", "--without-media"],
      { PATH: "" },
      { platform: "linux", root: folder() },
    );
    assert.equal(without.withMedia, false);
    assert.deepEqual(without.args.slice(0, 3), ["--bundles", "appimage", "--config"]);
    assert.deepEqual(JSON.parse(without.args[3]), {
      bundle: { linux: { appimage: { bundleMediaFramework: false } } },
    });
    assert.equal(without.env.GSTREAMER_PLUGINS_DIR, undefined);

    // Not an AppImage (a deb only, another platform): nothing to do, and the flag still goes.
    for (const [args, platform] of [
      [["--bundles", "deb", "--without-media"], "linux"],
      [["--without-media"], "darwin"],
    ]) {
      const other = prepareMedia(args, {}, { platform, root: folder() });
      assert.equal(other.appimage, false);
      assert.ok(!other.args.includes("--without-media"));
    }
  } finally {
    [console.log, console.warn] = silence;
  }
});

test("the built image is checked for its plugins, or for none when built without", () => {
  const image = folder();
  plugins(
    join(image, "usr/lib/gstreamer-1.0"),
    REQUIRED_PLUGINS.filter((name) => name !== "app"),
  );
  assert.throws(() => checkTree(image, true), /lacks GStreamer's app/);
  plugins(join(image, "usr/lib/gstreamer-1.0"), ["app"]);
  assert.throws(() => checkTree(image, true), /launch hook/);
  mkdirSync(join(image, "apprun-hooks"));
  writeFileSync(join(image, "apprun-hooks/linuxdeploy-plugin-gstreamer.sh"), "");
  checkTree(image, true);
  assert.throws(() => checkTree(image, false), /carries a GStreamer plugin folder/);
  checkTree(folder(), false);
});

test("the images a build made are found where Tauri writes them", { skip: LINUX_ONLY }, () => {
  assert.equal(
    imageDir(["--bundles", "appimage"], {}, "/repo"),
    "/repo/target/release/bundle/appimage",
  );
  assert.equal(
    imageDir(
      ["--target", "x86_64-unknown-linux-gnu", "--debug"],
      { CARGO_TARGET_DIR: "/t" },
      "/repo",
    ),
    "/t/x86_64-unknown-linux-gnu/debug/bundle/appimage",
  );
  const dir = folder();
  writeFileSync(join(dir, "old.AppImage"), "");
  utimesSync(join(dir, "old.AppImage"), new Date(1000), new Date(1000));
  writeFileSync(join(dir, "Ghost_0.1.0_amd64.AppImage"), "");
  writeFileSync(join(dir, "notes.txt"), "");
  assert.deepEqual(builtImages(dir, Date.now() - 60_000), [
    join(dir, "Ghost_0.1.0_amd64.AppImage"),
  ]);
  assert.deepEqual(builtImages(join(dir, "missing"), 0), []);
});
