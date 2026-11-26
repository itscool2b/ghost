// Writes the ghost-release plan for the packages a release build left in one directory: each file's
// role from its name, the app version from the workspace Cargo.toml, and the channel, notes and
// trusted public key from the environment. ghost-release then verifies every byte and signature;
// this only names what is there and refuses anything missing, doubled or unexpected.
//
// Usage: node .github/scripts/release-plan.mjs <artifact-directory> <plan.json>
// Environment: RELEASE_CHANNEL (staging or stable), RELEASE_NOTES, GHOST_UPDATER_PUBLIC_KEY,
// and optionally RELEASE_VERSION (the version the packages were built as, when the build was
// given one; the workspace's by default), RELEASE_PUBLISHER_SIGNED (`true` only when a publisher
// signed the packages for the operating systems) and RELEASE_DATE (RFC 3339; now by default).
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** Each role, the file names that are its package, and whether it carries an updater signature. */
export const ROLES = [
  { kind: "macos_universal_dmg", matches: (name) => name.endsWith(".dmg"), signed: false },
  {
    kind: "macos_universal_updater",
    matches: (name) => name.endsWith(".app.tar.gz"),
    signed: true,
  },
  { kind: "windows_x64_nsis", matches: (name) => name.endsWith("_x64-setup.exe"), signed: true },
  {
    kind: "windows_arm64_nsis",
    matches: (name) => name.endsWith("_arm64-setup.exe"),
    signed: true,
  },
  { kind: "linux_x64_appimage", matches: (name) => name.endsWith(".AppImage"), signed: true },
  { kind: "linux_x64_deb", matches: (name) => name.endsWith(".deb"), signed: true },
  { kind: "linux_x64_rpm", matches: (name) => name.endsWith(".rpm"), signed: true },
];

/** The release's artifacts from a directory listing; throws on anything missing or ambiguous. */
export function artifacts(names) {
  const packages = names.filter((name) => !name.endsWith(".sig"));
  const claimed = new Set();
  const found = ROLES.map(({ kind, matches, signed }) => {
    const files = packages.filter(matches);
    if (files.length !== 1) {
      throw new Error(`Expected one ${kind} package, found ${files.length}: ${files.join(", ")}`);
    }
    const [file] = files;
    claimed.add(file);
    const signature = signed ? `${file}.sig` : null;
    if (signature && !names.includes(signature)) {
      throw new Error(`${file} has no updater signature (${signature})`);
    }
    if (signature) claimed.add(signature);
    return { kind, file, signature };
  });
  const extra = names.filter((name) => !claimed.has(name));
  if (extra.length > 0) throw new Error(`Unexpected files in the release: ${extra.join(", ")}`);
  return found;
}

/** `[workspace.package] version` from the workspace Cargo.toml. */
export function workspaceVersion(cargoToml) {
  const section = /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/mu.exec(cargoToml);
  const version = section && /^version\s*=\s*"([^"]+)"\s*$/mu.exec(section[1]);
  if (!version) throw new Error("Cargo.toml has no [workspace.package] version");
  return version[1];
}

export function plan({
  names,
  cargoToml,
  channel,
  notes,
  publicKey,
  date,
  version,
  publisherSigned = false,
}) {
  if (channel !== "staging" && channel !== "stable") {
    throw new Error(`The channel must be staging or stable, not ${JSON.stringify(channel)}`);
  }
  if (!notes?.trim()) throw new Error("Release notes are required");
  if (!publicKey?.trim()) throw new Error("GHOST_UPDATER_PUBLIC_KEY is required");
  const built = version?.trim() || workspaceVersion(cargoToml);
  if (!/^\d+\.\d+\.\d+$/u.test(built)) {
    throw new Error(`The version must be three plain numbers, not ${JSON.stringify(built)}`);
  }
  const found = artifacts(names);
  // A package built as another version is another build's: every named file carries the version,
  // except the macOS updater archive, whose name is fixed.
  for (const { kind, file } of found) {
    if (kind !== "macos_universal_updater" && !file.includes(built)) {
      throw new Error(`${file} was not built as version ${built}`);
    }
  }
  return {
    channel,
    version: built,
    pub_date: date,
    notes: notes.trim(),
    public_key: publicKey.trim(),
    publisher_signed: publisherSigned === true,
    artifacts: found,
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [directory, output] = process.argv.slice(2);
  if (!directory || !output) {
    console.error("Usage: node .github/scripts/release-plan.mjs <artifact-directory> <plan.json>");
    process.exit(2);
  }
  const root = fileURLToPath(new URL("../../", import.meta.url));
  const result = plan({
    names: readdirSync(directory),
    cargoToml: readFileSync(resolve(root, "Cargo.toml"), "utf8"),
    channel: process.env.RELEASE_CHANNEL,
    notes: process.env.RELEASE_NOTES,
    publicKey: process.env.GHOST_UPDATER_PUBLIC_KEY,
    version: process.env.RELEASE_VERSION,
    publisherSigned: process.env.RELEASE_PUBLISHER_SIGNED === "true",
    date: process.env.RELEASE_DATE || new Date().toISOString().replace(/\.\d+Z$/u, "Z"),
  });
  writeFileSync(output, `${JSON.stringify(result, null, 2)}\n`);
  console.log(
    `Release plan: ${result.version} on ${result.channel}, ${result.artifacts.length} packages.`,
  );
}
