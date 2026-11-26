// Build-time platform glue: hash the framework's prerendered hydration scripts for a static CSP.
import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = new URL("../build/client/", import.meta.url);
const hashes = new Set();
async function visit(path) {
  for (const item of await readdir(path, { withFileTypes: true })) {
    const name = join(path, item.name);
    if (item.isDirectory()) await visit(name);
    else if (item.name.endsWith(".html")) {
      const text = await readFile(name, "utf8");
      for (const match of text.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)) {
        if (!/\bsrc\s*=/.test(match[1]) && match[2])
          hashes.add(`'sha256-${createHash("sha256").update(match[2]).digest("base64")}'`);
      }
    }
  }
}
await visit(fileURLToPath(root));
// connect-src names the relay, which answers under the site's own address. wss://o7.codes and
// wss://auracode.si are the addresses the site had before (until 26 and 19 November 2026): the
// remote page is still served there for phones paired before a move, and each goes when that
// page does (apps/web-worker/src/lib.rs).
const content =
  `/*\n  X-Content-Type-Options: nosniff\n  Referrer-Policy: no-referrer\n  X-Frame-Options: DENY\n  Permissions-Policy: camera=(), microphone=(), geolocation=()\n  Content-Security-Policy: default-src 'self'; script-src 'self' ${[...hashes].join(" ")}; connect-src 'self' wss://onghost.dev wss://preview.onghost.dev wss://o7.codes wss://auracode.si; frame-src 'none'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; font-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'\n\n/remote*\n  Cache-Control: no-store\n  X-Robots-Tag: noindex, nofollow\n` +
  ["/", "/download*", "/privacy*", "/terms*", "/changelog*", "/remote*", "/third-party-notices*"]
    .map((path) => `\n${path}\n  Content-Security-Policy: frame-ancestors 'none'\n`)
    .join("");
if (content.split("\n").some((line) => line.length > 2000))
  throw new Error("Static security header exceeds Cloudflare's line limit");
await writeFile(new URL("_headers", root), content);
