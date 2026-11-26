import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

/**
 * The signing script's decisions, run against stand-ins for `security`, `codesign` and friends:
 * no keychain is touched, so this runs on any machine with a POSIX shell. The `create` case uses
 * the real `openssl` (macOS's LibreSSL, or OpenSSL elsewhere) to check the certificate it makes.
 */

const SCRIPT = fileURLToPath(new URL("./macos-signing.sh", import.meta.url));
const README = fileURLToPath(new URL("../README.md", import.meta.url));
/** The shell to run it with: `sh` is bash on macOS and dash on Ubuntu; set this to try another. */
const SHELL = process.env.SIGNING_TEST_SHELL ?? "sh";
const HASH = "E0BAFFD34BF15181B26CA3608C909DB87E2B9D94";
/** The entitlements the script signs a dev build with (dictation's microphone), as it names them. */
const ENTITLEMENTS = `${dirname(SCRIPT)}/../src-tauri/Entitlements.plist`;
const OTHER = "0A36EDAE78CC3FA61AF1A1C5FE1E1C15CDEEE4B9";

/** `security find-identity -p codesigning` output listing `lines` as matching identities. */
function listing(lines: readonly string[], valid: readonly string[] = []): string {
  const numbered = (entries: readonly string[]) =>
    entries.map((entry, index) => `  ${index + 1}) ${entry}`).join("\n");
  return [
    "",
    "Policy: Code Signing",
    "  Matching identities",
    numbered(lines),
    `     ${lines.length} identities found`,
    "",
    "  Valid identities only",
    numbered(valid),
    `     ${valid.length} valid identities found`,
    "",
  ].join("\n");
}

const UNTRUSTED = `${HASH} "Ghost Local Signing" (CSSMERR_TP_NOT_TRUSTED)`;

let dir: string;
let bin: string;
let log: string;

/** Writes an executable shell script named `name` into the fake bin directory. */
function tool(name: string, body: string): void {
  const path = join(bin, name);
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
}

/** A stand-in that records its arguments, one call per line, and runs `body`. */
function recordingTool(name: string, body = "exit 0"): void {
  tool(name, `printf '%s %s\\n' "${name}" "$*" >> "${log}"\n${body}`);
}

function calls(): string[] {
  try {
    return readFileSync(log, "utf8").trim().split("\n").filter(Boolean);
  } catch {
    return [];
  }
}

/** `security` whose `find-identity` prints `output`. */
function keychainWith(output: string): void {
  const file = join(dir, "find-identity.txt");
  writeFileSync(file, output);
  recordingTool(
    "security",
    `case "$1" in find-identity) cat "${file}" ;; find-certificate) exit 0 ;; esac`,
  );
}

function runScript(args: readonly string[], env: Record<string, string> = {}) {
  const result = spawnSync(SHELL, [SCRIPT, ...args], {
    encoding: "utf8",
    env: { PATH: `${bin}:/usr/bin:/bin`, HOME: dir, TMPDIR: dir, ...env },
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

/** A program named `name` (the app is `Ghost`) that prints its arguments and exits `code`. */
function program(name: string, code = 0): string {
  const path = join(dir, name);
  writeFileSync(path, `#!/bin/sh\nprintf 'ran:%s\\n' "$@"\nexit ${code}\n`);
  chmodSync(path, 0o755);
  return path;
}

describe("macos-signing.sh's pointers into the README", () => {
  // `pnpm signing create` once sent people to a 'Rotating' section; the heading is "Rotate it".
  it("names only sections that exist", () => {
    const headings = readFileSync(README, "utf8")
      .split("\n")
      .flatMap((line) => /^#+\s+(.+?)\s*$/.exec(line)?.[1] ?? []);
    const named = [
      ...readFileSync(SCRIPT, "utf8").matchAll(/'([^']+)' in apps\/desktop\/README\.md/g),
    ].flatMap((match) => match[1] ?? []);
    expect(named.length).toBeGreaterThan(0);
    for (const section of named) expect(headings).toContain(section);
  });
});

describe.skipIf(process.platform === "win32")("macos-signing.sh", () => {
  beforeEach(() => {
    dir = mkdtempSync(join(tmpdir(), "ghost-signing-test-"));
    bin = join(dir, "bin");
    mkdirSync(bin);
    log = join(dir, "calls.log");
  });

  afterEach(() => {
    rmSync(dir, { recursive: true, force: true });
  });

  describe("identity", () => {
    it("prints the hash of the one local identity", () => {
      keychainWith(listing([UNTRUSTED]));
      const result = runScript(["identity"]);
      expect(result.status).toBe(0);
      expect(result.stdout.trim()).toBe(HASH);
    });

    it("counts an identity once even when it is also listed as valid", () => {
      // A certificate someone marked as trusted appears in both parts of the listing.
      const trusted = `${HASH} "Ghost Local Signing"`;
      keychainWith(listing([trusted], [trusted]));
      const result = runScript(["identity"]);
      expect(result.status).toBe(0);
      expect(result.stdout.trim()).toBe(HASH);
    });

    it("reports that there is none with exit status 1", () => {
      keychainWith(listing([]));
      const result = runScript(["identity"]);
      expect(result.status).toBe(1);
      expect(result.stdout).toBe("");
    });

    it("ignores identities whose names only contain the name", () => {
      keychainWith(
        listing([`${OTHER} "my Ghost Local Signing"`, `${OTHER} "Ghost Local Signing (old)"`]),
      );
      expect(runScript(["identity"]).status).toBe(1);
    });

    it("refuses to choose between two identities with the same name", () => {
      keychainWith(listing([UNTRUSTED, `${OTHER} "Ghost Local Signing"`]));
      const result = runScript(["identity"]);
      expect(result.status).toBe(2);
      expect(result.stdout).toBe("");
      expect(result.stderr).toContain(HASH);
      expect(result.stderr).toContain(OTHER);
      expect(result.stderr).toContain("security delete-identity -Z");
    });

    it("prefers APPLE_SIGNING_IDENTITY without asking the keychain", () => {
      keychainWith(listing([UNTRUSTED]));
      const result = runScript(["identity"], { APPLE_SIGNING_IDENTITY: "-" });
      expect(result.status).toBe(0);
      expect(result.stdout.trim()).toBe("-");
      expect(calls()).toEqual([]);
    });

    it("looks only in GHOST_SIGNING_KEYCHAIN when it is set", () => {
      keychainWith(listing([UNTRUSTED]));
      runScript(["identity"], { GHOST_SIGNING_KEYCHAIN: "/tmp/signing.keychain-db" });
      expect(calls()).toEqual(["security find-identity -p codesigning /tmp/signing.keychain-db"]);
    });
  });

  describe("run (Cargo's runner)", () => {
    it("signs the app with the identity and the bundle identifier, then runs it", () => {
      keychainWith(listing([UNTRUSTED]));
      recordingTool("codesign");
      const app = program("ghost");
      const result = runScript(["run", app, "an arg", "second"]);
      expect(result.status).toBe(0);
      expect(result.stdout).toBe("ran:an arg\nran:second\n");
      expect(calls()).toContain(
        `codesign --force --sign ${HASH} --identifier dev.onghost.desktop --entitlements ${ENTITLEMENTS} ${app}`,
      );
    });

    it("reads the bundle identifier without plutil too", () => {
      keychainWith(listing([UNTRUSTED]));
      recordingTool("codesign");
      tool("plutil", "exit 1");
      const app = program("ghost");
      runScript(["run", app]);
      expect(calls()).toContain(
        `codesign --force --sign ${HASH} --identifier dev.onghost.desktop --entitlements ${ENTITLEMENTS} ${app}`,
      );
    });

    it("passes the keychain to codesign when one is configured", () => {
      keychainWith(listing([UNTRUSTED]));
      recordingTool("codesign");
      const app = program("ghost");
      runScript(["run", app], { GHOST_SIGNING_KEYCHAIN: "/tmp/signing.keychain-db" });
      expect(calls()).toContain(
        `codesign --force --sign ${HASH} --identifier dev.onghost.desktop --keychain /tmp/signing.keychain-db --entitlements ${ENTITLEMENTS} ${app}`,
      );
    });

    it("runs the app unsigned, with a warning, when there is no identity", () => {
      keychainWith(listing([]));
      recordingTool("codesign");
      const app = program("ghost");
      const result = runScript(["run", app]);
      expect(result.status).toBe(0);
      expect(result.stdout).toBe("ran:\n"); // it ran, with no arguments
      expect(result.stderr).toContain("pnpm signing create");
      expect(calls().some((call) => call.startsWith("codesign"))).toBe(false);
    });

    it("runs the app unsigned when the identity is ambiguous", () => {
      keychainWith(listing([UNTRUSTED, `${OTHER} "Ghost Local Signing"`]));
      recordingTool("codesign");
      const app = program("ghost", 3);
      const result = runScript(["run", app]);
      expect(result.status).toBe(3);
      expect(result.stderr).toContain("without the local signature");
      expect(calls().some((call) => call.startsWith("codesign"))).toBe(false);
    });

    it("still runs the app when codesign fails", () => {
      keychainWith(listing([UNTRUSTED]));
      recordingTool("codesign", "echo 'errSecInternalComponent' >&2; exit 1");
      const app = program("ghost");
      const result = runScript(["run", app, "x"]);
      expect(result.status).toBe(0);
      expect(result.stdout).toBe("ran:x\n");
      expect(result.stderr).toContain("could not sign this dev build (errSecInternalComponent)");
    });

    it("leaves test binaries alone", () => {
      keychainWith(listing([UNTRUSTED]));
      recordingTool("codesign");
      const tests = program("ghost_desktop-49ec438ee61aacc9");
      const result = runScript(["run", tests, "--nocapture"]);
      expect(result.stdout).toBe("ran:--nocapture\n");
      expect(calls()).toEqual([]);
    });

    it("passes the program's exit status through", () => {
      keychainWith(listing([UNTRUSTED]));
      recordingTool("codesign");
      expect(runScript(["run", program("ghost", 7)]).status).toBe(7);
    });

    it("needs a program", () => {
      expect(runScript(["run"]).status).toBe(64);
    });
  });

  describe("create", () => {
    it("only runs on macOS", () => {
      tool("uname", "echo Linux");
      const result = runScript(["create"]);
      expect(result.status).toBe(1);
      expect(result.stderr).toContain("only needed on macOS");
    });

    it("never replaces an existing identity", () => {
      tool("uname", "echo Darwin");
      keychainWith(listing([UNTRUSTED]));
      recordingTool("codesign");
      recordingTool("openssl", "exit 0");
      const result = runScript(["create"]);
      expect(result.status).toBe(1);
      expect(result.stderr).toContain("already exists");
      expect(result.stderr).toContain("see 'Rotate it' in apps/desktop/README.md");
      expect(calls().filter((call) => /^(openssl req|security import)/.test(call))).toEqual([]);
    });

    it.runIf(spawnSync("openssl", ["version"]).status === 0)(
      "imports a new code-signing key and certificate, and leaves no key file behind",
      () => {
        tool("uname", "echo Darwin");
        recordingTool("codesign");
        const found = join(dir, "found");
        const imported = join(dir, "imported.pem");
        // No identity until the import succeeds, then the new one.
        tool(
          "security",
          [
            `printf '%s %s\\n' security "$*" >> "${log}"`,
            'case "$1" in',
            `find-identity) if [ -f "${found}" ]; then printf '%s\\n' '${listing([UNTRUSTED])}'; fi ;;`,
            `import) cp "$2" "${imported}"; touch "${found}" ;;`,
            "esac",
          ].join("\n"),
        );
        const result = runScript(["create"]);
        expect(result.stderr).toBe("");
        expect(result.status).toBe(0);
        expect(result.stdout).toContain(HASH);

        const imports = calls().filter((call) => call.startsWith("security import"));
        expect(imports).toHaveLength(1);
        expect(imports[0]).toMatch(/ -f pemseq -t agg -T \/usr\/bin\/codesign$/);
        const pem = readFileSync(imported, "utf8");
        expect(pem).toContain("-----BEGIN PRIVATE KEY-----");
        expect(pem).toContain("-----BEGIN CERTIFICATE-----");

        // `-text` rather than `-ext`: LibreSSL, macOS's own openssl, has only the former.
        const certificate = spawnSync("openssl", ["x509", "-noout", "-text"], {
          input: pem.slice(pem.indexOf("-----BEGIN CERTIFICATE-----")),
          encoding: "utf8",
        });
        expect(certificate.stdout).toContain("Ghost Local Signing");
        expect(certificate.stdout).toContain("Code Signing");
        expect(certificate.stdout).toContain("CA:FALSE");

        // The work directory with the private key is gone.
        const left = spawnSync("sh", ["-c", `ls -d "${dir}"/ghost-signing.* 2>/dev/null`], {
          encoding: "utf8",
        });
        expect(left.stdout).toBe("");
      },
    );
  });
});
