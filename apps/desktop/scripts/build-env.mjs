// Build tooling only. Keep private build-machine paths out of distributed Rust binaries.
export function releaseBuildEnv(env, roots) {
  if (
    env.O7_UPDATER_CHANNEL !== undefined &&
    !["stable", "staging"].includes(env.O7_UPDATER_CHANNEL)
  ) {
    throw new Error("O7_UPDATER_CHANNEL must be stable or staging.");
  }
  const flags = env.CARGO_ENCODED_RUSTFLAGS
    ? env.CARGO_ENCODED_RUSTFLAGS.split("\x1f")
    : (env.RUSTFLAGS ?? "").split(/\s+/u).filter(Boolean);
  // rustc applies the last matching mapping, so specific roots follow the home directory.
  for (const [source, destination] of roots) {
    if (!source) continue;
    if (/[=\r\n]/u.test(source) || source.includes("\x1f")) {
      throw new Error("A build path contains a character rustc cannot safely remap.");
    }
    for (const path of new Set([source, source.replaceAll("\\", "/")])) {
      flags.push(`--remap-path-prefix=${path}=${destination}`);
    }
  }
  return { ...env, CARGO_ENCODED_RUSTFLAGS: flags.join("\x1f") };
}

/**
 * A build's version as `--app-version` gives it: three plain numbers, each small enough for a
 * Windows version field. Anything else (a tag, a pre-release, a stray flag) stops the build.
 */
export function appVersion(value) {
  const parts = /^(\d{1,5})\.(\d{1,5})\.(\d{1,5})$/u.exec(value ?? "");
  if (!parts || parts.slice(1).some((part) => Number(part) > 65535 || /^0\d/u.test(part))) {
    throw new Error(`--app-version takes a version like 0.2.1, not ${JSON.stringify(value)}.`);
  }
  return parts[0];
}

export function requireUpdaterSigning(env) {
  if (!env.O7_UPDATER_PUBLIC_KEY?.trim() || !env.TAURI_SIGNING_PRIVATE_KEY?.trim()) {
    throw new Error(
      "Signed updater builds require O7_UPDATER_PUBLIC_KEY and TAURI_SIGNING_PRIVATE_KEY from the release secret store.",
    );
  }
}
