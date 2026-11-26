import assert from "node:assert/strict";
import test from "node:test";
import { artifacts, plan, workspaceVersion } from "./release-plan.mjs";

const NAMES = [
  "Ghost_0.2.0_universal.dmg",
  "Ghost.app.tar.gz",
  "Ghost.app.tar.gz.sig",
  "Ghost_0.2.0_x64-setup.exe",
  "Ghost_0.2.0_x64-setup.exe.sig",
  "Ghost_0.2.0_arm64-setup.exe",
  "Ghost_0.2.0_arm64-setup.exe.sig",
  "Ghost_0.2.0_amd64.AppImage",
  "Ghost_0.2.0_amd64.AppImage.sig",
  "Ghost_0.2.0_amd64.deb",
  "Ghost_0.2.0_amd64.deb.sig",
  "Ghost-0.2.0-1.x86_64.rpm",
  "Ghost-0.2.0-1.x86_64.rpm.sig",
];
const CARGO = `[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "0.2.0"\nedition = "2024"\n\n[workspace.dependencies]\nserde = "1"\n`;

test("every role gets its one package, and every updater package its signature", () => {
  const found = artifacts(NAMES);
  assert.deepEqual(
    found.map(({ kind, file, signature }) => [kind, file, signature]),
    [
      ["macos_universal_dmg", "Ghost_0.2.0_universal.dmg", null],
      ["macos_universal_updater", "Ghost.app.tar.gz", "Ghost.app.tar.gz.sig"],
      ["windows_x64_nsis", "Ghost_0.2.0_x64-setup.exe", "Ghost_0.2.0_x64-setup.exe.sig"],
      ["windows_arm64_nsis", "Ghost_0.2.0_arm64-setup.exe", "Ghost_0.2.0_arm64-setup.exe.sig"],
      ["linux_x64_appimage", "Ghost_0.2.0_amd64.AppImage", "Ghost_0.2.0_amd64.AppImage.sig"],
      ["linux_x64_deb", "Ghost_0.2.0_amd64.deb", "Ghost_0.2.0_amd64.deb.sig"],
      ["linux_x64_rpm", "Ghost-0.2.0-1.x86_64.rpm", "Ghost-0.2.0-1.x86_64.rpm.sig"],
    ],
  );
});

test("a missing, doubled, unsigned or unexpected file stops the plan", () => {
  assert.throws(() => artifacts(NAMES.filter((name) => !name.endsWith(".deb"))), /linux_x64_deb/u);
  assert.throws(() => artifacts([...NAMES, "Ghost_0.1.0_amd64.deb"]), /found 2/u);
  assert.throws(
    () => artifacts(NAMES.filter((name) => name !== "Ghost_0.2.0_x64-setup.exe.sig")),
    /no updater signature/u,
  );
  assert.throws(() => artifacts([...NAMES, "notes.txt"]), /Unexpected files/u);
});

test("the version is the workspace package's", () => {
  assert.equal(workspaceVersion(CARGO), "0.2.0");
  assert.throws(() => workspaceVersion('[package]\nversion = "1.0.0"\n'), /workspace\.package/u);
});

test("the plan carries the channel, trimmed notes and the trusted key", () => {
  const result = plan({
    names: NAMES,
    cargoToml: CARGO,
    channel: "staging",
    notes: "  Linux packages.\n",
    publicKey: " public-key-text \n",
    date: "2026-11-06T00:00:00Z",
  });
  assert.equal(result.channel, "staging");
  assert.equal(result.version, "0.2.0");
  assert.equal(result.notes, "Linux packages.");
  assert.equal(result.public_key, "public-key-text");
  assert.equal(result.artifacts.length, 7);
  const base = { names: NAMES, cargoToml: CARGO, notes: "n", publicKey: "k", date: "d" };
  assert.throws(() => plan({ ...base, channel: "beta" }), /staging or stable/u);
  assert.throws(() => plan({ ...base, channel: "stable", notes: " " }), /notes/u);
  assert.throws(() => plan({ ...base, channel: "stable", publicKey: "" }), /PUBLIC_KEY/u);
});

test("a numbered build's plan takes its version, and refuses a package built as another", () => {
  const base = {
    cargoToml: CARGO,
    channel: "stable",
    notes: "n",
    publicKey: "k",
    date: "2026-11-19T05:00:00Z",
  };
  const numbered = NAMES.map((name) => name.replaceAll("0.2.0", "0.1.1570"));
  const result = plan({ ...base, names: numbered, version: "0.1.1570" });
  assert.equal(result.version, "0.1.1570");
  assert.equal(result.publisher_signed, false, "no publisher signed it unless the plan says so");
  assert.equal(
    plan({ ...base, names: numbered, version: "0.1.1570", publisherSigned: true }).publisher_signed,
    true,
  );
  // The workspace's own version when the build was given none.
  assert.equal(plan({ ...base, names: NAMES }).version, "0.2.0");
  assert.equal(plan({ ...base, names: NAMES, version: "  " }).version, "0.2.0");
  // Packages of the workspace version are not this build's.
  assert.throws(
    () => plan({ ...base, names: NAMES, version: "0.1.1570" }),
    /was not built as version 0\.1\.1570/u,
  );
  for (const version of ["v0.1.1570", "0.1", "0.1.1570-beta", "0.1.1570+5", "latest"]) {
    assert.throws(
      () => plan({ ...base, names: numbered, version }),
      /three plain numbers/u,
      version,
    );
  }
});
