// `pnpm bundle`: `tauri build` with the local signing identity on macOS.
//
// A bundle signed with the same identity every time keeps the app's designated requirement across
// builds, so macOS keeps its privacy grants (~/Desktop access) instead of asking again. When this
// Mac has no identity yet the bundle is signed ad hoc, as CI does, with a warning: a machine
// without the certificate can still build. APPLE_SIGNING_IDENTITY, when set, always wins (it is
// Tauri's own setting). Elsewhere this is plain `tauri build`. Arguments are passed on to it. See
// ../README.md. Every bundle first fetches dictation's models and read aloud's voice model, or
// verifies the ones already fetched, against their pinned SHA-256 (scripts/dictation-models.mjs),
// since the installer carries them. A Linux AppImage carries GStreamer's base and good plugins, staged from this machine and
// checked in the image afterwards, unless `--without-media` says otherwise
// (scripts/appimage-media.mjs).
//
// `--app-version <x.y.z>` builds the app as that version instead of the workspace's: the Publish
// workflow numbers every build (0.1.N, N the number of commits on main), because an installed copy
// offers an update only when the published version is higher than its own.

import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { builtImages, checkImage, imageDir, prepareMedia } from "./appimage-media.mjs";
import { appVersion, releaseBuildEnv, requireUpdaterSigning } from "./build-env.mjs";
import { ensureModels } from "./dictation-models.mjs";

const signingScript = fileURLToPath(new URL("./macos-signing.sh", import.meta.url));
const tauriCli = createRequire(import.meta.url).resolve("@tauri-apps/cli/tauri.js");
const args = process.argv.slice(2);
const versioned = args.indexOf("--app-version");
if (versioned >= 0) {
  let version;
  try {
    version = appVersion(args[versioned + 1]);
  } catch (error) {
    console.error(`o7: ${error.message}`);
    process.exit(1);
  }
  args.splice(versioned, 2, "--config", JSON.stringify({ version }));
}
const signed = args.indexOf("--signed-updater");
if (signed >= 0) {
  requireUpdaterSigning(process.env);
  args.splice(signed, 1);
  args.push(
    "--config",
    fileURLToPath(new URL("../src-tauri/tauri.updater.conf.json", import.meta.url)),
    // The CLI checks each updater signature it makes against the configured key, which the
    // committed config leaves empty: the app itself embeds the key at compile time (updater.rs).
    "--config",
    JSON.stringify({
      plugins: { updater: { pubkey: process.env.O7_UPDATER_PUBLIC_KEY.trim() } },
    }),
  );
}

const sysroot = spawnSync(process.env.RUSTC ?? "rustc", ["--print", "sysroot"], {
  encoding: "utf8",
  stdio: ["ignore", "pipe", "inherit"],
});
if (sysroot.status !== 0) {
  console.error("o7: could not determine the Rust sysroot for build-path remapping.");
  process.exit(1);
}
const home = homedir();
const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
const env = releaseBuildEnv(process.env, [
  [home, "/build-home"],
  [process.env.CARGO_HOME ?? join(home, ".cargo"), "/cargo"],
  [sysroot.stdout.trim(), "/rust-toolchain"],
  [repoRoot, "/o7/"],
]);
if (process.platform === "darwin" && !env.APPLE_SIGNING_IDENTITY) {
  const lookup = spawnSync("sh", [signingScript, "identity"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (lookup.status === 0) {
    env.APPLE_SIGNING_IDENTITY = lookup.stdout.trim();
  } else if (lookup.status === 1) {
    env.APPLE_SIGNING_IDENTITY = "-";
    console.warn(
      'o7: no "o7 Local Signing" identity on this Mac, so the bundle is signed ad hoc and macOS will treat it as a new app. Create the identity once with: pnpm signing create',
    );
  } else {
    // Ambiguous: the script has said which identities clash. Signing with either would be a guess.
    process.exit(lookup.status ?? 1);
  }
}

// Sound and video in a Linux AppImage: the plugins are staged now, before the long build.
let media;
try {
  media = prepareMedia(args, env);
} catch (error) {
  console.error(error.message);
  process.exit(1);
}

try {
  await ensureModels();
} catch (error) {
  media.cleanup();
  console.error(
    `o7: the dictation and voice models are not ready, so the bundle would lack them: ${error.message}`,
  );
  process.exit(1);
}

const startedAt = Date.now();
const build = spawnSync(process.execPath, [tauriCli, "build", ...media.args], {
  stdio: "inherit",
  env: media.env,
});
media.cleanup();
if (build.error) {
  console.error(`o7: could not run tauri build: ${build.error.message}`);
  process.exit(1);
}
if (build.status === 0 && media.appimage) {
  const images = builtImages(imageDir(media.args, media.env, repoRoot), startedAt);
  if (images.length === 0) {
    console.error("o7: the build made no AppImage to check.");
    process.exit(1);
  }
  for (const image of images) {
    try {
      checkImage(image, media.withMedia);
    } catch (error) {
      console.error(`o7: ${image}: ${error.message}`);
      process.exit(1);
    }
    console.log(
      media.withMedia
        ? `o7: ${image} carries GStreamer's plugins.`
        : `o7: ${image} carries no GStreamer plugins: it plays sound and video only with the system's.`,
    );
  }
}
process.exit(build.status ?? 1);
