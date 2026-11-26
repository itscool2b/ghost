// Sound and video in the Linux AppImage. WebKitGTK plays media through GStreamer, and a page that
// needs a GStreamer element the image cannot find takes its own process down (17 November 2026:
// any page with sound or video, in an image that carried GStreamer's libraries but no plugins).
// So an AppImage build stages GStreamer's base and good plugins from this machine for Tauri's
// AppImage bundler, whose linuxdeploy GStreamer plugin copies every file of
// GSTREAMER_PLUGINS_DIR into the image (with the libraries they need) and the plugin scanner from
// GSTREAMER_HELPERS_DIR, and adds the launch hook that points GStreamer at them. Only an allowlist
// is staged: no codec packs with patent issues (gst-plugins-ugly, gst-libav), no
// gst-plugins-bad. After the build, the image itself is checked.
//
// `--without-media` builds without them, on purpose and loudly: the app then uses the system's
// plugins and turns sound and video off where they are missing (src-tauri/src/browser/media.rs).

import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  rmSync,
  statSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join, resolve } from "node:path";

/** The plugins WebKit's media cannot do without (`REQUIRED` in src-tauri/src/browser/media.rs). */
export const REQUIRED_PLUGINS = [
  "coreelements",
  "app",
  "audioconvert",
  "audioresample",
  "autodetect",
  "playback",
  "typefindfunctions",
];

/**
 * What else common sound and video need, from GStreamer's core, base and good sets only. Staged
 * when this machine has them; plugin names differ between GStreamer releases (videoconvertscale
 * replaced videoconvert and videoscale in 1.22).
 */
export const OPTIONAL_PLUGINS = [
  // Base: conversion, volume, the GL video path, Ogg, Opus, Vorbis, Theora, ALSA output.
  "audiorate",
  "videoconvertscale",
  "videoconvert",
  "videoscale",
  "videorate",
  "volume",
  "opengl",
  "rawparse",
  "ogg",
  "opus",
  "vorbis",
  "theora",
  "alsa",
  // Good: PulseAudio output (PipeWire speaks it too), MP4 and WebM, VP8 and VP9, FLAC, MP3, WAV,
  // tempo for playback speed, audio parsers, video orientation.
  "pulseaudio",
  "isomp4",
  "matroska",
  "vpx",
  "audioparsers",
  "interleave",
  "audiofx",
  "wavparse",
  "flac",
  "mpg123",
  "id3demux",
  "apetag",
  "videofilter",
];

/** The packages that hold the plugins (and patchelf, which the bundler's GStreamer step needs). */
export const PACKAGES = [
  ["Arch", "sudo pacman -S --needed gst-plugins-base gst-plugins-good patchelf"],
  [
    "Debian, Ubuntu",
    "sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-gl gstreamer1.0-pulseaudio patchelf",
  ],
  ["Fedora", "sudo dnf install gstreamer1-plugins-base gstreamer1-plugins-good patchelf"],
];

const WITHOUT_MEDIA = "--without-media";

/** Debian's multiarch name for this machine. */
function multiarch(arch = process.arch) {
  return { x64: "x86_64-linux-gnu", arm64: "aarch64-linux-gnu", ia32: "i386-linux-gnu" }[arch];
}

/** The `--bundles` (`-b`) values in `args`, or null when none is given. */
export function bundleNames(args) {
  let names = null;
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    const inline = /^(?:--bundles|-b)=(.*)$/u.exec(arg);
    if (inline) {
      names = [...(names ?? []), ...inline[1].split(",")];
    } else if (arg === "--bundles" || arg === "-b") {
      names ??= [];
      while (index + 1 < args.length && !args[index + 1].startsWith("-")) {
        index += 1;
        names.push(...args[index].split(","));
      }
    }
  }
  return names?.map((name) => name.trim()).filter(Boolean) ?? null;
}

/**
 * Whether `tauri build` with `args` makes an AppImage here: on Linux, unless `--no-bundle`, when
 * `--bundles` names it or is absent (the configured targets include it).
 */
export function bundlesAppImage(args, platform = process.platform) {
  if (platform !== "linux" || args.includes("--no-bundle")) return false;
  const names = bundleNames(args);
  return names === null || names.includes("appimage");
}

/** `args` without `--without-media`, and whether it was there. */
export function takeWithoutMedia(args) {
  const rest = args.filter((arg) => arg !== WITHOUT_MEDIA);
  return { args: rest, withoutMedia: rest.length !== args.length };
}

/** This machine's GStreamer plugin folder: `GSTREAMER_PLUGINS_DIR`, else the system's. */
export function findPluginsDir(env = process.env, root = "/") {
  if (env.GSTREAMER_PLUGINS_DIR) return resolve(env.GSTREAMER_PLUGINS_DIR);
  const triple = multiarch();
  const candidates = [
    triple && join(root, "usr/lib", triple, "gstreamer-1.0"),
    join(root, "usr/lib64/gstreamer-1.0"),
    join(root, "usr/lib/gstreamer-1.0"),
  ].filter(Boolean);
  return candidates.find((dir) => existsSync(dir)) ?? null;
}

/** The folder that holds GStreamer's plugin scanner (Debian, Fedora, Arch keep it elsewhere). */
export function findScannerDir(env = process.env, root = "/") {
  if (env.GSTREAMER_HELPERS_DIR) return resolve(env.GSTREAMER_HELPERS_DIR);
  const triple = multiarch();
  const candidates = [
    triple && join(root, "usr/lib", triple, "gstreamer1.0/gstreamer-1.0"),
    join(root, "usr/libexec/gstreamer-1.0"),
    join(root, "usr/lib64/gstreamer-1.0"),
    join(root, "usr/lib/gstreamer-1.0"),
  ].filter(Boolean);
  return candidates.find((dir) => existsSync(join(dir, "gst-plugin-scanner"))) ?? null;
}

/** Whether `program` is on `PATH`. */
export function onPath(program, env = process.env) {
  return (env.PATH ?? "")
    .split(delimiter)
    .filter(Boolean)
    .some((dir) => {
      try {
        return statSync(join(dir, program)).isFile();
      } catch {
        return false;
      }
    });
}

/**
 * Copies the allowlisted plugins of `pluginsDir`, and the scanner of `scannerDir`, into
 * `stage/plugins` and `stage/helpers`. Answers what was staged and which required plugins are
 * missing.
 */
export function stagePlugins({ pluginsDir, scannerDir, stage }) {
  const plugins = join(stage, "plugins");
  const helpers = join(stage, "helpers");
  mkdirSync(plugins, { recursive: true });
  mkdirSync(helpers, { recursive: true });
  const staged = [];
  const missing = [];
  for (const name of [...REQUIRED_PLUGINS, ...OPTIONAL_PLUGINS]) {
    const file = `libgst${name}.so`;
    const source = pluginsDir ? join(pluginsDir, file) : null;
    if (source && existsSync(source)) {
      copyFileSync(source, join(plugins, file));
      staged.push(name);
    } else if (REQUIRED_PLUGINS.includes(name)) {
      missing.push(name);
    }
  }
  const scanner = Boolean(scannerDir && existsSync(join(scannerDir, "gst-plugin-scanner")));
  if (scanner)
    copyFileSync(join(scannerDir, "gst-plugin-scanner"), join(helpers, "gst-plugin-scanner"));
  return { plugins, helpers, staged, missing, scanner };
}

/** Why a media AppImage cannot be built here, with what to install on each distribution. */
export function missingMessage({ missing = [], patchelf = true }) {
  const lacks = [];
  if (missing.length > 0)
    lacks.push(`GStreamer's base and good plugins (missing: ${missing.join(", ")})`);
  if (!patchelf) lacks.push("patchelf, which the bundler's GStreamer step runs");
  return [
    `Ghost: the AppImage would play no sound or video: this machine lacks ${lacks.join(" and ")}.`,
    "Install them, then build again:",
    ...PACKAGES.map(([family, command]) => `  ${`${family}:`.padEnd(16)}${command}`),
    `Or build without them on purpose with ${WITHOUT_MEDIA}: the app then uses the system's plugins where they are, and turns sound and video off where they are not.`,
  ].join("\n");
}

/**
 * Readies an AppImage build's media from `bundle`'s arguments: the arguments and env for
 * `tauri build`, and what `checkImage` needs afterwards. Throws with `missingMessage` when the
 * plugins cannot be staged.
 */
export function prepareMedia(args, env, { platform = process.platform, root = "/" } = {}) {
  const taken = takeWithoutMedia(args);
  const appimage = bundlesAppImage(taken.args, platform);
  if (!appimage) {
    return { args: taken.args, env, appimage: false, withMedia: false, cleanup() {} };
  }
  if (taken.withoutMedia) {
    console.warn(
      `Ghost: ${WITHOUT_MEDIA}: this AppImage carries no GStreamer plugins. It plays sound and video only with the system's plugins, and turns them off where they are missing.`,
    );
    return {
      args: [
        ...taken.args,
        "--config",
        JSON.stringify({ bundle: { linux: { appimage: { bundleMediaFramework: false } } } }),
      ],
      env,
      appimage: true,
      withMedia: false,
      cleanup() {},
    };
  }
  const stage = mkdtempSync(join(tmpdir(), "auracode-gstreamer-"));
  const cleanup = () => rmSync(stage, { recursive: true, force: true });
  const staged = stagePlugins({
    pluginsDir: findPluginsDir(env, root),
    scannerDir: findScannerDir(env, root),
    stage,
  });
  const patchelf = onPath("patchelf", env);
  if (staged.missing.length > 0 || !patchelf) {
    cleanup();
    throw new Error(missingMessage({ missing: staged.missing, patchelf }));
  }
  if (!staged.scanner)
    console.warn(
      "Ghost: GStreamer's plugin scanner was not found; the image's GStreamer will load its plugins in its own process when it first scans them.",
    );
  console.log(`Ghost: the AppImage carries GStreamer's ${staged.staged.join(", ")}.`);
  return {
    args: taken.args,
    env: { ...env, GSTREAMER_PLUGINS_DIR: staged.plugins, GSTREAMER_HELPERS_DIR: staged.helpers },
    appimage: true,
    withMedia: true,
    cleanup,
  };
}

/** The AppImages a build made: `.AppImage` files in `dir` written at or after `since` (ms). */
export function builtImages(dir, since) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => name.endsWith(".AppImage"))
    .map((name) => join(dir, name))
    .filter((file) => statSync(file).mtimeMs >= since);
}

/** Where `tauri build` with `args` writes its AppImages. */
export function imageDir(args, env, repoRoot) {
  const target = env.CARGO_TARGET_DIR ? resolve(env.CARGO_TARGET_DIR) : join(repoRoot, "target");
  const at = args.indexOf("--target");
  const triple =
    at >= 0
      ? args[at + 1]
      : /^--target=(.+)$/u.exec(args.find((arg) => arg.startsWith("--target=")) ?? "")?.[1];
  const profile = args.includes("--debug") || args.includes("-d") ? "debug" : "release";
  return join(target, ...(triple ? [triple] : []), profile, "bundle", "appimage");
}

/**
 * Checks an unpacked image (`root`, its `squashfs-root`): with media, every required plugin and
 * the GStreamer launch hook are in it; without, it has no plugin folder at all, so the app's own
 * fallback to the system's plugins applies. Throws what is wrong.
 */
export function checkTree(root, withMedia) {
  const folder = join(root, "usr/lib/gstreamer-1.0");
  if (!withMedia) {
    if (existsSync(folder))
      throw new Error(`a ${WITHOUT_MEDIA} image carries a GStreamer plugin folder (${folder}).`);
    return;
  }
  const lacking = REQUIRED_PLUGINS.filter((name) => !existsSync(join(folder, `libgst${name}.so`)));
  if (lacking.length > 0)
    throw new Error(
      `the image lacks GStreamer's ${lacking.join(", ")} (in usr/lib/gstreamer-1.0).`,
    );
  if (!existsSync(join(root, "apprun-hooks/linuxdeploy-plugin-gstreamer.sh")))
    throw new Error("the image lacks the launch hook that points GStreamer at its plugins.");
}

/** Unpacks the GStreamer parts of an AppImage with its own runtime and checks them. */
export function checkImage(image, withMedia) {
  // It runs from a scratch folder, where a relative path would name nothing.
  const program = resolve(image);
  const scratch = mkdtempSync(join(tmpdir(), "auracode-appimage-check-"));
  try {
    for (const pattern of ["usr/lib/gstreamer-1.0/*", "apprun-hooks/*"]) {
      const run = spawnSync(program, ["--appimage-extract", pattern], {
        cwd: scratch,
        stdio: ["ignore", "ignore", "pipe"],
        encoding: "utf8",
      });
      if (run.error || run.status !== 0)
        throw new Error(
          `could not look inside ${image}: ${run.error?.message ?? run.stderr.trim() ?? `exit ${run.status}`}`,
        );
    }
    checkTree(join(scratch, "squashfs-root"), withMedia);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}
