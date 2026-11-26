# Rust website Worker

This Worker serves the existing `apps/web/build/client` prerender output through a normal Workers
code deployment. React rendering and `scripts/headers.mjs` remain build-time tooling; routing,
response policies and body selection run in Rust. It has no Assets binding, object bucket, secret,
outbound subrequest or runtime filesystem access. Its one binding is `COUNTS`, the D1 database of the
site's anonymous download and daily-use counters (`src/count.rs`, `schema.sql`,
[docs/usage-counts.md](../../docs/usage-counts.md)).

The existing assets-only configuration remains an alternative. Do not deploy both configurations
to the same website route. This code path does not call the denied Static Assets upload endpoint,
change connector authorization, or require a user login. No cloud mutation is part of these checks.

## Build and verify

Use the repository's pinned pnpm and Rust toolchain. The Wasm build needs `worker-build` 0.8.7
and the `wasm32-unknown-unknown` target.

```powershell
corepack pnpm install --frozen-lockfile
cargo test -p ghost-web-worker --locked
cargo clippy -p ghost-web-worker --all-targets --locked -- -D warnings
corepack pnpm --filter @ghost/web-worker worker:build
cargo clippy -p ghost-web-worker --target wasm32-unknown-unknown --locked -- -D warnings
corepack pnpm --filter @ghost/web-worker worker:test
```

`worker:build` first runs the unchanged website build, including its generated `_headers`, then
`worker-build --release`. The Rust build script embeds public bytes with `include_bytes!`, MIME,
SHA-256 ETags and the resolved header rules. Cargo tracks the build directory so HTML, asset and
header changes invalidate the manifest. Missing prerender output or unsupported header syntax
fails the Wasm build. Native workspace tests require no frontend output and use synthetic assets.

The only deployment modules are `build/index.js` and `build/index_bg.wasm`. JavaScript is generated
platform glue. A local build from the current website source measured 20,473 bytes and 985,588
bytes respectively, about 1 MB total. Rebuild and measure the final integrated source before upload.
The build rejects more than 1,024 public files or 16 MiB of public content. Cloudflare's current
[Worker size limit](https://developers.cloudflare.com/workers/platform/limits/) is 64 MiB uncompressed.

## Response contract

- Every prerendered page serves its pretty path, trailing slash and explicit `index.html` alias with
  the same page policy. A missing page or asset is a plain 404, never the SPA fallback.
- `_headers`, `__spa-fallback.html`, server output, hidden files and unknown file types are excluded
  or rejected at build time. Symlinks and special files are refused. Only compiled public entries
  can be selected. Traversal, encoded separators and ambiguous paths reaching Rust are refused.
  WHATWG Request may normalize literal dot segments before user code; there is no path-based
  filesystem access to escape even in that case.
- Rust applies the existing global and page-specific `_headers` rules. Multiple CSP policies are
  combined with commas and header removals are preserved. No page lifts a global header today:
  every page refuses framing. `/remote` uses `no-store` on every alias, without ETags.
- Vite's hashed assets use `public, max-age=31536000, immutable`. Other public files revalidate with
  content ETags. `If-None-Match` supports weak comparison and lists. HEAD retains representation
  headers without bytes, and 304 has no response body. Cloudflare may compress transport bytes;
  identity-encoding tests verify exact Content-Length independently of compression.
- Only GET and HEAD serve files. Other website methods return 405. `/api` and `/api/*` always return
  an uncached 404 from this Worker, including POST, so missing route wiring cannot answer a relay
  request with a page. The separate relay Worker (`apps/relay`) owns `/api/remote/*`; no Worker
  owns any other `/api` path.
- This uses ordinary Worker request execution rather than Assets hosting. Browser caching reduces
  repeat asset requests; no extra edge cache or cache rule is configured by this implementation.

Cloudflare does not automatically apply an asset `_headers` file to
[Worker-generated responses](https://developers.cloudflare.com/workers/static-assets/headers/),
which is why these rules are explicitly compiled into the Rust response path.

## Deployment handoff

The checked-in `wrangler.toml` names local, preview and production scripts. `workers_dev` and preview
URLs are disabled. No environment variables or secrets are needed. Use ordinary multipart code
upload with `build/index.js` as the ES module and `build/index_bg.wasm` as the Wasm module. Preserve
the existing more-specific relay route, `preview.onghost.dev/api/remote/*`, before attaching this Worker's
`preview.onghost.dev/*` route. Cloudflare's
[route precedence](https://developers.cloudflare.com/workers/configuration/routing/routes/)
selects the more-specific match. This is a deployment check, not a substitute for testing it.

After root deploys the reviewed build, verify the live TLS preview's page, JS, font, image, missing
asset, HEAD and security-header responses, plus `/api/remote/v1/health` from the relay Worker.
Performance and production publication remain separate acceptance gates. No denied Assets upload
retry is required or performed here.

The five compiled Miniflare cases compare every public file to its source bytes, all seven pages and
their aliases, inline CSP hashes, the remote page's cache policy, framing, MIME, ETag/HEAD/304,
errors and `/api` isolation. They disable outbound requests and dispose their owned runtime.
