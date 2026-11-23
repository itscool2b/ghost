/**
 * A version as people are shown it (owner, 23 November 2026: "i want version numbers to make
 * sense ... keep it at the tenths"). A build's full version is three numbers, `0.1.1787`: the
 * first two are the version people know o7 by, and the third is the build, which only counts
 * up so that every published build is newer than the last. File names, links and checksums keep the full version.
 */

/** "0.1" from "0.1.1787". Anything that is not three plain numbers is shown as it is. */
export function versionName(version: string): string {
  const parts = /^(\d+)\.(\d+)\.(\d+)$/.exec(version.trim());
  return parts ? `${parts[1]}.${parts[2]}` : version;
}

/** "0.1 (build 1787)" from "0.1.1787": the name, and the build where two copies must be told apart. */
export function versionWithBuild(version: string): string {
  const parts = /^(\d+)\.(\d+)\.(\d+)$/.exec(version.trim());
  return parts ? `${parts[1]}.${parts[2]} (build ${parts[3]})` : version;
}
