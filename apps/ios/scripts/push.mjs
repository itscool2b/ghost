#!/usr/bin/env node
// Sends a sealed Ghost notification to the app on a simulator, as the computer and the relay would:
// the payload sealed with the push key of a device (`push.rs`), inside Apple's fixed outer words.
// The notification extension opens it, or leaves the fixed words when it cannot.
//
// A simulator does not run the notification extension for a notification sent this way, so
// Apple's fixed words show. --buttons adds the category the extension would set once it opened
// the payload, so the buttons and the app's answer can be tried on a simulator; the relay never
// sends one.
//
//   node apps/ios/scripts/push.mjs --udid <simulator> [--kind asking|finished|failed|resolved|test]
//     [--buttons]
//     [--chat c3] [--title "Tidy the changelog"] [--line "Run pnpm lint --fix?"]
//     [--host <host id>] [--device <device id>] [--key <device key, hex>]
//
// Without --host, --device and --key it seals for the app's mock computer (`-GhostBackend mock`),
// whose pairing uses a fixed device id and key in debug builds.

import { execFileSync } from "node:child_process";
import { webcrypto as crypto } from "node:crypto";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { parseArgs } from "node:util";

const MOCK = {
  host: "TW9jayBjb21wdXRlciAxMg",
  device: "TW9ja0RldjE",
  key: Buffer.from(Array.from({ length: 32 }, (_, index) => index)).toString("hex"),
};

const { values } = parseArgs({
  options: {
    udid: { type: "string" },
    kind: { type: "string", default: "asking" },
    chat: { type: "string", default: "c3" },
    title: { type: "string", default: "Tidy the changelog" },
    line: { type: "string", default: "Run `pnpm lint --fix`?" },
    project: { type: "string", default: "Website" },
    agent: { type: "string", default: "Claude Code" },
    host: { type: "string", default: MOCK.host },
    device: { type: "string", default: MOCK.device },
    key: { type: "string", default: MOCK.key },
    request: { type: "string", default: "r3" },
    print: { type: "boolean", default: false },
    buttons: { type: "boolean", default: false },
  },
});

const base64url = (bytes) => Buffer.from(bytes).toString("base64url");

async function seal(payload, deviceKey, host, device) {
  const material = await crypto.subtle.importKey("raw", deviceKey, "HKDF", false, ["deriveKey"]);
  const key = await crypto.subtle.deriveKey(
    {
      name: "HKDF",
      hash: "SHA-256",
      salt: new Uint8Array(0),
      info: new TextEncoder().encode("o7 push v1"),
    },
    material,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt"],
  );
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const sealed = await crypto.subtle.encrypt(
    {
      name: "AES-GCM",
      iv: nonce,
      additionalData: new TextEncoder().encode(`o7 push v1|${host}|${device}`),
    },
    key,
    new TextEncoder().encode(JSON.stringify(payload)),
  );
  return base64url(Buffer.concat([Buffer.from(nonce), Buffer.from(sealed)]));
}

const payload = {
  v: 1,
  id: crypto.randomUUID(),
  at: Date.now(),
  host: "Studio",
  kind: values.kind,
  chat: values.chat,
  title: values.title,
  project: values.project,
  agent: values.agent,
  line: values.line,
  asking_count: values.kind === "asking" ? 1 : 0,
  ...(values.kind === "asking"
    ? { request: { id: values.request, type: "permission", allow: "allow_once", deny: "reject" } }
    : {}),
};

const blob = await seal(payload, Buffer.from(values.key, "hex"), values.host, values.device);
const notification = {
  "Simulator Target Bundle": "dev.onghost.mobile",
  aps: {
    alert: { title: "Ghost", body: "Something changed on your computer." },
    sound: "default",
    "mutable-content": 1,
    ...(values.buttons && values.kind === "asking" ? { category: "ghost.permission" } : {}),
  },
  ghost: { v: 1, d: values.device, b: blob },
};

if (values.print || !values.udid) {
  console.log(JSON.stringify(notification, null, 2));
} else {
  const file = join(mkdtempSync(join(tmpdir(), "ghost-push-")), "push.apns");
  writeFileSync(file, JSON.stringify(notification));
  execFileSync("xcrun", ["simctl", "push", values.udid, "dev.onghost.mobile", file], {
    stdio: "inherit",
  });
}
