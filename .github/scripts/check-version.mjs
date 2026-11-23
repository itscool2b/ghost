// `VERSION` is the version o7 is published as: three numbers, `major.minor.patch`, chosen by the
// pull request that changes the app (owner, 23 November 2026). A big feature raises the minor
// number and sets the patch to 0 (0.2.4 to 0.3.0); a small change or a fix raises the patch
// (0.2.4 to 0.2.5). Every published version must be newer than the last, or installed copies
// would never be offered it (docs/developing-in-o7.md, "Version").
//
//   node .github/scripts/check-version.mjs [folder]   the format, and against main when the
//                                                      branch changes the app (pnpm lint)
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

/** What ends up in the app: a change to any of these is published, so it needs a new version. */
export const APP_PATHS = [
  "apps/desktop/",
  "crates/",
  "packages/ui/",
  "Cargo.toml",
  "Cargo.lock",
  "package.json",
  "pnpm-lock.yaml",
  "pnpm-workspace.yaml",
  "rust-toolchain.toml",
  ".cargo/",
];

const SHAPE = /^(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})\.(0|[1-9]\d{0,4})$/u;

/** The three numbers of a version, or null when it is not three plain numbers. */
export function parse(version) {
  const parts = SHAPE.exec(version.trim());
  if (!parts || parts.slice(1).some((part) => Number(part) > 65535)) return null;
  return parts.slice(1).map(Number);
}

/** Whether `a` is a newer version than `b`. */
export function newer(a, b) {
  for (let i = 0; i < 3; i += 1) if (a[i] !== b[i]) return a[i] > b[i];
  return false;
}

/** Whether a changed path is part of the app. */
export function inApp(path) {
  return APP_PATHS.some((app) => (app.endsWith("/") ? path.startsWith(app) : path === app));
}

/**
 * What is wrong with `version`, given main's version and the paths this branch changed (null
 * when main could not be read: only the shape is checked then).
 */
export function checkVersion(version, mainVersion, changed) {
  const mine = parse(version);
  if (!mine)
    return [
      `VERSION is ${JSON.stringify(version.trim())}: it must be three plain numbers, like 0.2.1`,
    ];
  if (mainVersion === null || changed === null) return [];
  const main = parse(mainVersion);
  if (!main) return [];
  const app = changed.filter(inApp);
  if (app.length === 0 || newer(mine, main)) return [];
  const [major, minor, patch] = main;
  return [
    `This branch changes the app (${app.slice(0, 3).join(", ")}${app.length > 3 ? ", ..." : ""}) but VERSION is not newer than main's ${main.join(".")}. ` +
      `Raise it: ${major}.${minor}.${patch + 1} for a small change or a fix, ${major}.${minor + 1}.0 for a big feature.`,
  ];
}

function git(folder, ...args) {
  const run = spawnSync("git", args, { cwd: folder, encoding: "utf8" });
  return run.status === 0 ? run.stdout : null;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const folder = resolve(process.argv[2] ?? ".");
  const version = readFileSync(resolve(folder, "VERSION"), "utf8");
  // Against main where it can be read: a shallow checkout (CI) checks the shape only, and the
  // Publish workflow refuses an unraised version when it is merged.
  const base = git(folder, "merge-base", "origin/main", "HEAD")?.trim() || null;
  const onMain = base !== null && git(folder, "rev-parse", "HEAD")?.trim() === base;
  const mainVersion = base && !onMain ? git(folder, "show", "origin/main:VERSION") : null;
  const changed =
    base && !onMain
      ? (git(folder, "diff", "--name-only", base, "HEAD") ?? "").split("\n").filter(Boolean)
      : null;
  const problems = checkVersion(version, mainVersion, changed);
  if (problems.length > 0) {
    console.error(`VERSION needs fixing:\n${problems.map((p) => `  ${p}`).join("\n")}`);
    process.exit(1);
  }
  console.log(`VERSION is ${version.trim()}.`);
}
