import assert from "node:assert/strict";
import { test } from "node:test";
import { appVersion, releaseBuildEnv, requireUpdaterSigning } from "./build-env.mjs";

test("Windows paths and spaces remain single rustc arguments", () => {
  const env = releaseBuildEnv({ RUSTFLAGS: "-C debuginfo=1" }, [
    ["C:\\Users\\Fixture Name", "/build-home"],
    ["C:\\Users\\Fixture Name\\project", "/auracode"],
  ]);
  assert.deepEqual(env.CARGO_ENCODED_RUSTFLAGS.split("\x1f"), [
    "-C",
    "debuginfo=1",
    "--remap-path-prefix=C:\\Users\\Fixture Name=/build-home",
    "--remap-path-prefix=C:/Users/Fixture Name=/build-home",
    "--remap-path-prefix=C:\\Users\\Fixture Name\\project=/auracode",
    "--remap-path-prefix=C:/Users/Fixture Name/project=/auracode",
  ]);
});

test("existing encoded flags take precedence and preserve embedded spaces", () => {
  const env = { CARGO_ENCODED_RUSTFLAGS: '--cfg\x1ffeature="a b"', RUSTFLAGS: "ignored" };
  const updated = releaseBuildEnv(env, [["/Users/fixture", "/build-home"]]);
  assert.equal(
    updated.CARGO_ENCODED_RUSTFLAGS,
    '--cfg\x1ffeature="a b"\x1f--remap-path-prefix=/Users/fixture=/build-home',
  );
  assert.equal(env.CARGO_ENCODED_RUSTFLAGS, '--cfg\x1ffeature="a b"');
});

test("empty optional roots are omitted and ambiguous separators fail", () => {
  assert.equal(releaseBuildEnv({}, [[undefined, "/cargo"]]).CARGO_ENCODED_RUSTFLAGS, "");
  for (const path of ["/a=b", "/a\nb", "/a\x1fb"]) {
    assert.throws(() => releaseBuildEnv({}, [[path, "/auracode"]]), /cannot safely remap/u);
  }
});

test("updater channel is explicit and closed", () => {
  for (const channel of ["stable", "staging"]) {
    assert.equal(
      releaseBuildEnv({ GHOST_UPDATER_CHANNEL: channel }, []).GHOST_UPDATER_CHANNEL,
      channel,
    );
  }
  for (const channel of ["", "preview", "https://example.com", "staging "]) {
    assert.throws(
      () => releaseBuildEnv({ GHOST_UPDATER_CHANNEL: channel }, []),
      /must be stable or staging/,
    );
  }
});

test("signed updater builds require both public and private material without printing either", () => {
  for (const env of [
    {},
    { GHOST_UPDATER_PUBLIC_KEY: "fixture-public" },
    { TAURI_SIGNING_PRIVATE_KEY: "fixture-private" },
  ]) {
    assert.throws(
      () => requireUpdaterSigning(env),
      (error) => {
        assert(!error.message.includes("fixture-"));
        return /require/u.test(error.message);
      },
    );
  }
  requireUpdaterSigning({
    GHOST_UPDATER_PUBLIC_KEY: "fixture-public",
    TAURI_SIGNING_PRIVATE_KEY: "fixture-private",
  });
});

test("a build's version is three plain numbers that fit a Windows version field", () => {
  for (const version of ["0.1.1570", "1.0.0", "12.34.65535"]) {
    assert.equal(appVersion(version), version);
  }
  for (const version of [
    undefined,
    "",
    "v0.1.1570",
    "0.1",
    "0.1.1570.2",
    "0.1.1570-beta",
    "0.1.65536",
    "0.01.5",
    "--target",
    "0.1.1570 ",
  ]) {
    assert.throws(() => appVersion(version), /--app-version takes a version/u, String(version));
  }
});
