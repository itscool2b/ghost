# Cloudflare resources

Created and verified on 2026-10-26 in Cloudflare account `5feb14f2fe605875bcf1472f348cb7ed`.
These identifiers are not credentials.

**The site moved to o7.codes on 19 November 2026.** [The last section](#the-site-moves-to-o7codes-2026-11-19)
says what is where now. Everything above it was written while the site was at auracode.si and is
kept as written, so an address in it is the address of its day.

**Accounts were removed from the code on 16 November 2026, at the owner's request, and their
cloud resources were deleted on 17 November 2026.** Claude Code and Codex sync plugins natively, so
the app is local-only: no sign-in, no sync, nothing uploaded by an account. The account API
Workers, the D1 databases, the blob R2 buckets, the Email Sending domain and the Turnstile widgets
no longer exist; [the last section](#accounts-removed-from-the-code-2026-11-16) records what was
deleted and what it held. The record below is kept as it was written, with the deleted parts
marked. The relay Workers, the release bucket and the website Workers are current.

| Resource | Identifier | Purpose | Status |
|---|---|---|---|
| D1 database | `auracode`, `b5b9de26-4714-4475-aacc-9f599a7c2eb2` | Accounts, sessions, device registry, encrypted sync records | Deleted 2026-11-17 |
| R2 bucket | `auracode-blobs` | Private encrypted attachments and config blobs | Deleted 2026-11-17 |
| R2 bucket | `auracode-releases` | Installers, checksums, updater manifests | Current |
| Email Sending domain | `auracode.si`, `84273eb1ef3c4b108b774bc0e0a000ed` | Passwordless sign-in sender | Deleted 2026-11-17 |
| Preview D1 database | `auracode-preview`, `11d6fa29-5c39-49c5-a85b-1ecc7645dbd5` | Isolated acceptance data, US jurisdiction | Deleted 2026-11-17 |
| Preview R2 bucket | `auracode-preview-blobs` | Private acceptance ciphertext | Deleted 2026-11-17 |
| Worker | `auracode-relay`, routes `o7.codes/api/remote/*` and, for copies of the app from before the move, `auracode.si/api/remote/*` | Remote sessions relay: one Durable Object (`Room`, SQLite-backed) per computer, and one more (`Apns`) that signs the iPhone app's notifications for Apple. Stores key and token hashes, and when each computer's recent notifications went out. Its one secret is Apple's push key (`APNS_KEY`), which the owner sets; without it no notification is sent | Current |
| Preview Worker | `auracode-relay-preview`, route `preview.o7.codes/api/remote/*` | The same relay for the preview site | Current |

The zone has one rate limiting rule, added on 2026-11-19 before the repository was opened (ruleset
`e81ea7c28dcc4e47b6d412e383c4a1b4`, phase `http_ratelimit`): a path starting `/api/remote/` gets at
most 60 requests in 10 seconds from one address and Cloudflare location, then a 429 for 10 seconds.
It covers the relay on the site and on the preview. The free plan allows this one rule, with this
period and this timeout. A burst of 120 requests at the preview relay was refused partway while
the website kept answering.

In the paragraphs below, everything about D1, the blob buckets, Email Sending, Turnstile,
`auracode-api`, `auracode-api-preview`, the `/api/*` route, sign-in and invitations describes the
removed account service. The longer runs are marked where they start.

D1 uses US jurisdiction, with read replication disabled. R2 was created with the eastern North
America location hint, which is not a data-residency guarantee. Both buckets start private; release
distribution is enabled only with the release pipeline. The production database remains empty.
Preview deployment status is recorded below.

Application code goes in Rust Workers. Use checked-in Wrangler configuration and SQL migrations,
separate local/preview data from production, and store credentials only in Cloudflare/GitHub secrets.
Cloudflare Email Sending provisioned the return path `cf-bounce.auracode.si`, SPF, DKIM and DMARC
records on 2026-10-26. Its DNS status is `ready`, with no reported errors. Sending is enabled,
reputation is healthy, and account metadata now reports a daily quota of 200 emails with zero sent.
Workers uses the Standard model; subscription billing was not queried. Actual delivery remains
unverified. Production Turnstile and API preparation are recorded below.
No Neon project, Neon Auth service, Hyperdrive connection, or Resend domain is needed.

> **Deleted on 2026-11-17 (accounts removed from the code).**
> The next four paragraphs: the preview bucket and database, their migrations, `auracode-api-preview`
> and its live checks.

The preview bucket's managed public domain is disabled and its custom domain list is empty.
On 2026-10-26, the empty preview database was exported before applying migrations 0001 through 0003.
The downloaded SQL backup is in ignored `target/backups/auracode-preview-before-migrations.sql`,
SHA-256 `309d1516f5d4f4f792b17106f7b761312f848c634e3028d70e6eb8ed39df7398`.
All 29 statements succeeded; migration tracking lists all three files, foreign-key checks return no
violations, and the database contains zero users.

On 2026-10-26, preview migrations 0004 and 0005 were applied through the Cloudflare connector after
downloading a fresh 6525-byte backup to `target/backups/auracode-preview-before-reset-deletion.sql`.
Backup SHA-256: `64f20f22a8d01a804cbf8881760e6f70af4eb770f56cc6f94158acfa0bbd2db3`.
All five migrations are recorded, foreign-key checks are clean and the database still has zero users.

The real compiled Rust Worker is deployed as `auracode-api-preview`, using the preview D1 and R2,
the sender-restricted email binding and disabled observability. Its WASM artifact SHA-256 is
`2ea77875999f5bd52ee6bd1b2ee45293d864bdceb399f64f5663fe5e5996fcbe`.
The connector uploaded verified binary WASM and its generated module, preserving platform secrets;
deployment `cae8aeed9aef4352bb0cb6a9d08e79c6` includes reset cancellation and deletion receipts.
Workers.dev and version preview URLs remain disabled. The route is `preview.auracode.si/api/*`,
with the configured five-minute cleanup trigger and an originless proxied DNS record.
A separate managed Turnstile widget permits only `preview.auracode.si`; its public site key is
`0x4AAAAAAFmu9KbL2voFn5QY`. `TURNSTILE_SECRET` and a cryptographically generated 32-byte
`AUTH_HMAC_KEY` were written directly into Cloudflare's secret store without being returned or logged.

Live preview checks pass: health and public configuration return 200 with `no-store`, an unauthenticated
account request returns 401, and a foreign browser Origin returns 403. Actual email delivery, browser
challenge completion, signed-in device sync and live revocation/cleanup acceptance remain pending.
The compiled Rust API passes fourteen local runtime cases, including failed email binding,
Turnstile rejection/replay, cookies, revocation, reset/cancel arbitration, retained deletion receipts
and scheduled ciphertext cleanup. A real Rust client also completes synthetic two-device key approval,
encrypted record/blob exchange and revocation against the compiled Worker. Live health/configuration and access rejection checks passed again
after the deployment. No email was sent.

The documented Static Assets upload returned Unauthorized, and the connector cannot supply its
separate JWT Authorization header. That upload path remains unused. The website is instead compiled
into a small independent Rust Worker and deployed through the ordinary authorized Worker code API.
The Worker has no Assets, D1, R2, email, secret or outbound-service bindings.

On 2026-10-26, the connector deployed `auracode-web-preview`, deployment
`15cc1403e2e24868b449df0066abd0aa`, with `workers.dev` and version preview URLs disabled. Its new
`preview.auracode.si/*` route (`ea92f2dd41954d8bbccda7968688550d`) retains the more-specific API route.
The 985,588-byte Wasm has SHA-256
`fc9b29d7f557fa3cb1e73b3d4830584b1ea623b4ce25e0e81f7d48557ed2f024`; the 20,473-byte generated JS has
SHA-256 `7fea5f5a1ed14314a3fac9d81f3037e8375a9a4db3b535fe546813fc83a41eeb`.

All 48 public files matched the local tested build byte-for-byte over live HTTPS. Nine page HEAD
requests, missing-asset 404, security headers, auth `no-store` and API route precedence passed.
Evidence is in ignored `target/live-preview-verification.json`. Production website publication is
recorded below; public API activation and actual email/Turnstile/device acceptance remain pending.

Universal SSL is enabled. An active Let's Encrypt certificate covers `auracode.si` and
`*.auracode.si`, expires 2027-01-22 and is renewed by Cloudflare. No registrar certificate is needed.

## Production website

The same tested public modules are deployed as `auracode-web`, deployment
`c200b10b3b3544408ea91257a70c79a0`, with the website hashes recorded above. The Worker has no
application bindings, and development and version preview URLs are disabled. The production route
`auracode.si/*` is `dfbdf5b588f1445c9999b14ef75fd319`. The originless proxied apex A record is
`17d49baeed498e13185721a83493ee8d`, pointing to documentation-only address `192.0.2.1`; the Worker
answers these requests without an origin server.

All 48 public files matched the local tested build over `https://auracode.si`. Nine page HEAD
responses, security headers and missing-asset refusal passed. The closed `/api/v1/config` route
returns plain 404 rather than website HTML. Evidence is in ignored
`target/live-production-verification.json`. The live landing page was visually inspected in the
browser. Installers remain unpublished and the production account API is not publicly routed.

The zone's `always_use_https` setting is enabled and verified. HTTP requests to the apex, a nested
download path with a query string, and the preview host returned 301 to the same HTTPS URL.

## Production preparation, public API route inactive

> **Deleted on 2026-11-17 (accounts removed from the code).**
> This whole section: the production sending domain, database, Turnstile widget and `auracode-api`.

On 2026-10-26, email-content previews were disabled for the existing `auracode.si` sending domain
(`84273eb1ef3c4b108b774bc0e0a000ed`). Readback confirms `preview_enabled:false`, `enabled:true` and
`drop_suppressed_recipients:false`. This avoids retaining sign-in-code message bodies in the Email
Service Activity log. No email was sent and no message content or existing secret was read.

Before any production schema write, inspection found only D1's internal `_cf_KV` table. The complete
SQL export was downloaded to ignored `target/backups/auracode-production-before-migrations.sql` in
the `auracode-web-worker` managed worktree. The backup is 32 bytes, SHA-256
`309d1516f5d4f4f792b17106f7b761312f848c634e3028d70e6eb8ed39df7398`, export bookmark
`00000001-00000000-00005110-0d7425c001b1a6491da77902b2cb9de5`.
Migrations 0001 through 0005 and the same `d1_migrations` tracker used in preview were then applied
to `b5b9de26-4714-4475-aacc-9f599a7c2eb2`. All 35 statements succeeded. All five migration names are
recorded, `PRAGMA foreign_key_check` returns no rows, and users, sessions and devices each count zero.

The separate managed production Turnstile widget is named `auracode production sign-in`, public
site key `0x4AAAAAAFnCekg62MEAwe_j`, with the single allowed-domain entry `auracode.si`. Cloudflare
domain entries also permit their subdomains; the Rust API independently requires the exact
`APP_ORIGIN` hostname and `signin` action. Preview retains its separate widget and secret.
The new widget secret and a freshly generated 32-byte HMAC key were written directly into production
Worker secret bindings. Their values were never returned, logged, copied into a local file or read back.

`auracode-api` contains the root task's already-tested compiled modules: 1,059,109-byte Wasm,
SHA-256 `2ea77875999f5bd52ee6bd1b2ee45293d864bdceb399f64f5663fe5e5996fcbe`, and 29,811-byte generated
JavaScript, SHA-256 `20f208e0d5dfee4413440affbc90e9ae9d9e5990ebf07135d5427223331f8dfb`.
Bytes were verified again inside the connector before ordinary multipart code upload. Code was
initially staged without an authentication key, so it failed closed while public development
endpoints were disabled. Production bindings were added only after that disabled state was verified.
The configured deployment is `35304950-7ef5-4b73-bfc1-011877401f54`, with version
`a6b23422-0e67-44b8-bbec-d6bd17404ea0` at 100 percent.

Readback confirms the production D1 ID, private `auracode-blobs`, `APP_ORIGIN=https://auracode.si`,
`EMAIL_FROM=signin@auracode.si`, the exact sender allowlist and both secret binding names. The bucket's
`r2.dev` access is disabled and its custom-domain list is empty. `workers.dev` and version preview
URLs are disabled; observability was configured disabled, Logpush is false, and the five-minute
cleanup cron is verified. No public API route or apex DNS record was added by this preparation.

Activation remains gated on the root task's source-check confirmation. Root owns the apex DNS and
production website route. Before opening accounts publicly, complete actual email delivery,
Turnstile/hostname/action acceptance, signed-in device approval/sync/revocation/cleanup checks and
the operator identity, legal contact and retention review described in `apps/web/README.md`.
The legal pages remain drafts. Preparation does not qualify those real-user flows or authorize
test bypasses, and no denied Assets upload, auth or billing endpoint was retried.

## Private preview and website analytics

> **Deleted on 2026-11-17 (accounts removed from the code).**
> The first and last paragraphs of this section: the invitation policy and the invitation build of
> both API Workers. The analytics rule between them is current.

The current access policy is an invitation-only cloud preview, with no payment flows. Before
activating the new API build, set `CLOUD_ACCOUNT_ALLOWLIST` independently on production and preview
as a secret JSON array of reviewed addresses. Never put membership in source, logs or browser code.
Missing, empty or malformed configuration denies cloud access. Existing accounts retain their
authenticated export, logout and fresh-verification deletion paths when membership is removed.
This is distinct from public account launch and does not resolve the legal-review requirements.

Cloudflare automatically injected an analytics beacon which the strict website CSP rejected. On
2026-10-26, the zone configuration ruleset `ac102c9e05884870928e61c835bccb08` was created in phase
`http_config_settings` with rule `7cfc6746108a4bbc9879589be10f6262`, reference
`auracode_disable_injected_analytics`. Its `set_config` action sets `disable_rum: true` for exactly
`http.host in {"auracode.si" "preview.auracode.si"}`. Readback verified the rule and a browser-agent
HTML fetch found no injected analytics. Unrelated sites and security controls were not changed.
See Cloudflare's [configuration-rule settings](https://developers.cloudflare.com/rules/configuration-rules/settings/).

The invitation build was subsequently uploaded to both API environments. Preview deployment is
`5e39dc99677c4f7fb58274accecedd1f`; production deployment is
`08607e5f6a334bec9fc4697fcd25b36c`. Their tested Wasm is 1,086,146 bytes, SHA-256
`4918d17be4919c1655b7c3f6261f48446b6681a01dd8f79aa6b8cf73a7760ef7`, and generated JavaScript is
29,939 bytes, SHA-256 `ea31dab3f90ad2c749daf772d97706d3921dce396be22e2c7f8cb615657fcc1e`.
Each environment has its own secret `CLOUD_ACCOUNT_ALLOWLIST` with the owner-approved membership;
existing secrets were preserved without reading their values. All 23 compiled API runtime checks
pass on these root-built modules. Live preview health/config return 200, unauthenticated account
returns 401, and an excluded synthetic email receives 403 `invite_required` before any challenge.
Preview user, session and challenge counts remain zero. Production API routing remains inactive.
No real email was sent, and the websites still need the newest invitation UI/assets deployment.

## Notices and invitation-order update (2026-10-26)

Deployments now use Wrangler with temporary configs under ignored `target/deploy-*` that mirror
the checked-in environment sections, set `no_bundle` and upload the tested modules unchanged.
Wrangler keeps existing secrets; secret names were rechecked after upload and no values were read.

- `auracode-web-preview` version `e31091c4-83c3-448b-aa5d-00f448bdeca4`: JS 20,473 bytes, SHA-256
  `7fea5f5a1ed14314a3fac9d81f3037e8375a9a4db3b535fe546813fc83a41eeb` (unchanged); Wasm 1,107,745
  bytes, SHA-256 `e926c1c676e2aa179e619160ee45457ef95a479378b8e23f1da9ce6cfcee7ff0`. All 53 public
  files match the local build over HTTPS, including `/third-party-notices` and its notice text; ten
  page HEAD checks, the notices frame policy, footer link, API precedence and missing-asset 404 pass.
- **Unused since 2026-11-16, not deleted.** `auracode-api-preview` version `e4550760-f4c9-47fe-b1f9-24b2b09d5a55`: JS unchanged
  (`ea31dab3…`); Wasm 1,087,289 bytes, SHA-256
  `9d545e661c1928e3a8fa355d196bed041364342b58ebf8cea77391e28d800d6a`. Membership is answered only
  after the per-IP limit and Turnstile. Live: config and health 200, unauthenticated account 401,
  uninvited address with no token 400 and with an invalid token 403, never `invite_required`.
  Preview D1 still has zero users, sessions and challenges; one HMAC-keyed IP rate row exists.
  Bindings, cron, disabled `workers.dev`/preview URLs and all routes are unchanged.

The production website and production API staging await explicit owner approval. Production
still serves website deployment `c200b10b…` and staged API `08607e5f…`, with no `/api/*` route.

Production website `auracode-web` version `0d0ca4e4-5884-4151-9ea0-ce639629867f` now serves the same
tested modules as preview (JS `7fea5f5a…`, Wasm `e926c1c6…`). All 53 public files match over
`https://auracode.si`, with ten page HEAD checks, the notices page and policy, footer link and
missing-asset 404. The unrouted `/api/v1/config` still returns a plain non-HTML 404.

> **Deleted on 2026-11-17 (accounts removed from the code).**

Production API `auracode-api` version `a173e36a-cd71-437d-aa9f-b783af83fdd1` is staged with the same
invitation-order build as preview (Wasm `9d545e66…`). All three secrets, D1, R2, email, plain vars,
the five-minute cron and disabled `workers.dev`/preview URLs are unchanged. It still has no route:
the zone routes only `auracode.si/*` to the website and the two preview routes. Production D1 has no
users, sessions or challenges.

## Website deploys on push (2026-10-27)

The website now deploys through Workers Builds, so it uses Cloudflare build minutes rather than
GitHub Actions. The repository connection is `915b851e-4e7f-4c2d-a416-c130a642f59f`, and every
trigger runs from the repository root with the account's existing build token.

| Worker | Branches | Deploy command ends with |
| --- | --- | --- |
| `auracode-web` | `main` | `wrangler deploy --env production` |
| `auracode-web-preview` | `main` | `wrangler deploy --env preview` |
| `auracode-web-preview` | every other branch | `wrangler deploy --env preview` |

The build command installs rustup's stable toolchain with the `wasm32-unknown-unknown` target and
`worker-build` 0.8.7 (`--locked`), then builds `@auracode/web` and runs `worker-build --release` in
`apps/web-worker`. The deploy command puts `$HOME/.cargo/bin` on `PATH` and runs
`pnpm exec wrangler deploy` there; Wrangler runs the checked-in `[build]` command again before
uploading. Workers Builds installs dependencies with the pinned pnpm 12.15.0 on Node.js 24.

The first build (`f4291adc-2cbf-44b5-a7e1-dd78790f3827`, `main` at `64f5529`, preview) passed in
about nine and a half minutes, five and a half of them compiling `worker-build`, which ships no
prebuilt binary. All 55 public files on `preview.auracode.si` matched the local build of the same
tree byte for byte, with the page, notices, API precedence and missing-asset checks passing. The
API Workers still deploy by hand, and production API routing remains inactive.

## Accounts removed from the code (2026-11-16)

The owner decided the app is local-only: Claude Code and Codex sync plugins natively, so auracode
has no account, no sign-in and no sync. The account API (`apps/api`), its D1 migrations
(`infra/migrations`), the website's sign-in, sign-up, account and device verification pages, and
the Turnstile origin in the website's CSP were removed from the repository.

That change itself ran no deploy, D1, R2, email, Turnstile or DNS command. The owner then asked for
the resources to be removed, and they were deleted on 17 November 2026 through the Cloudflare API,
after a read-only look at what they held:

- Workers `auracode-api` and `auracode-api-preview`, with their routes `auracode.si/api/*` and
  `preview.auracode.si/api/*`, their five-minute cleanup cron triggers and their secrets
  (`TURNSTILE_SECRET`, `AUTH_HMAC_KEY`, `CLOUD_ACCOUNT_ALLOWLIST`). The routes went first, each
  checked against its recorded pattern and script before it was deleted.
- D1 databases `auracode` (`b5b9de26-4714-4475-aacc-9f599a7c2eb2`) and `auracode-preview`
  (`11d6fa29-5c39-49c5-a85b-1ecc7645dbd5`). Production held one account with one approved device,
  a Windows PC, six sessions and one key record, and no sync changes or blobs; the owner confirmed
  it was a test. Preview held no rows. No export was kept.
- R2 buckets `auracode-blobs` and `auracode-preview-blobs`, both empty.
- The Email Sending domain `auracode.si` (`84273eb1ef3c4b108b774bc0e0a000ed`). Cloudflare removed
  its `cf-bounce.auracode.si` MX, SPF and DKIM records with it. `_dmarc.auracode.si` stays at
  `v=DMARC1; p=reject;`: the domain sends no mail, and the record keeps others from sending as it.
- The Turnstile widgets `auracode production sign-in` (site key `0x4AAAAAAFnCekg62MEAwe_j`) and
  `auracode preview sign-in` (site key `0x4AAAAAAFmu9KbL2voFn5QY`).

Afterwards `/api/*` outside `/api/remote/*` falls to the website Workers and answers 404, the
relay still answers on `/api/remote/v1/`, and the zone's routes are the two relay routes and the
two website routes. An installed build from before the removal that is signed in can no longer
reach its account; its account profile stays on that computer's disk, untouched.

Still current: the relay Workers `auracode-relay` and `auracode-relay-preview`, the release bucket
`auracode-releases`, the website Workers `auracode-web` and `auracode-web-preview` with their
routes and Workers Builds triggers, the analytics rule, and the zone's TLS and HTTPS settings.

## The site moves to o7.codes (2026-11-19)

The owner bought o7.codes and asked for everything to be wired to it. The zone
(`97dbe68f305979048e80226f5c860992`) is on the same account and nameservers and went active on
2026-11-19 at 19:21 UTC. auracode.si is being abandoned; until it goes it does one job, below.

What o7.codes has, each made to match what auracode.si had:

- **DNS.** Originless proxied A records to the documentation-only `192.0.2.1` for `o7.codes`
  (`12053ef335051c5d230d824800bc5023`) and `preview.o7.codes`
  (`747609af64ae6f4d15fccadaea83d39c`): a Worker route answers, no origin server exists. The
  domain sends no mail and says so, so nobody can send as it: `_dmarc` is `v=DMARC1; p=reject;`
  and the apex SPF is `v=spf1 -all`.
- **TLS.** Universal SSL, a Let's Encrypt certificate for `o7.codes` and `*.o7.codes` that
  Cloudflare renews. Always Use HTTPS is on.
- **The relay's rate limit.** The zone's one rule (ruleset `1a75616aa8c94871b3fddf7512710cad`,
  phase `http_ratelimit`): a path starting `/api/remote/` gets at most 60 requests in 10 seconds
  from one address and Cloudflare location, then is refused for 10 seconds.
- **No injected analytics.** Ruleset `3c21907a548242fd9959cea75eb490f1`, phase
  `http_config_settings`, turns Cloudflare's own page analytics off for `o7.codes` and
  `preview.o7.codes`.
- **Routes**, all written in the checked-in `wrangler.toml` files and applied by a deploy:
  `o7.codes/*` to `auracode-web` and `preview.o7.codes/*` to `auracode-web-preview` (Workers
  Builds, on a push), `o7.codes/api/remote/*` to `auracode-relay` and
  `preview.o7.codes/api/remote/*` to `auracode-relay-preview` (by hand, docs/remote-sessions.md).

What auracode.si still does, and only until copies of the app from before the move have updated:

- `auracode.si/api/remote/*` still reaches `auracode-relay`. A copy from before the move keeps
  its one socket to the relay there, and bot addresses made by it name that address. Both
  addresses reach the same rooms, so a new copy and an old page, or the other way round, meet.
- `auracode.si/*` still reaches `auracode-web`, which serves only `/remote` and the files that
  page loads there, because a phone paired before the move keeps its key under that origin and
  an older copy's pairing links open there. Every other page answers 308 to the same page on
  o7.codes. The relay lets a browser in from that origin as well (`FORMER_ORIGIN`).
- `preview.auracode.si` has no routes any more: the preview moved outright. Its DNS record and
  the old zone's own rate-limit and analytics rules stay while the zone has routes.

To finish the move, once nobody runs a copy from before it: delete the two auracode.si routes
from `apps/relay/wrangler.toml` and `apps/web-worker/wrangler.toml`, `FORMER_ORIGIN` from the
relay, `FORMER_HOSTS` from `apps/web-worker/src/lib.rs` and `wss://auracode.si` from the page's
`connect-src` (`apps/web/scripts/headers.mjs`), then deploy both.

A copy of the app from after the move removes, the first time it starts, every device and bot
link that was issued under auracode.si (docs/remote-sessions.md, "Credentials belong to the site
they were issued under"). So an updated computer has nothing left that the old address could be
used against: its phone pairs once more and its bot is linked once more, at o7.codes.

**Do not let auracode.si lapse while any copy from before the move is still in use.** That copy
has not removed anything. Whoever registers the domain next would answer its relay socket,
receive the tokens of its bot links (they are part of the address a bot posts to), be able to
use the device keys its phones keep under that origin, and serve whatever page they like at
`/remote`, where a pairing link hands its key to the page. Any of those is enough to steer that
computer's chats. Sealing protects a session from the relay; it does not protect a credential
from the address it was issued for. Keep the registration until the old copies are gone.

The app's identifier stays `si.auracode.desktop`. It names the data folder and the installed
app, not a web address; changing it would leave every chat and setting behind.

## Everything is o7 (2026-11-19, evening)

The owner asked for the old name to go everywhere ("change everything"). What changed in
Cloudflare and GitHub, each checked live afterwards:

- **The repository** is `itscool2b/o7` (renamed with `gh repo rename`; GitHub redirects the old
  address, so `releases/latest/download/latest.json`, which copies from before read to update, still
  resolves). Workers Builds' repository connection `915b851e-4e7f-4c2d-a416-c130a642f59f` was
  renamed with it (same repository id, `1431949458`).
- **The Workers** are `o7-web`, `o7-web-preview`, `o7-relay` and `o7-relay-preview`. They were
  moved without a gap: each website Worker was copied module for module from the live one under
  the new name (no workers.dev address, no previews), the relays were deployed from the same code
  without routes, and then every route was pointed at its new Worker in place:
  - `o7.codes/*` and `auracode.si/*` to `o7-web`; `preview.o7.codes/*` to `o7-web-preview`;
  - `o7.codes/api/remote/*` and `auracode.si/api/remote/*` to `o7-relay`;
    `preview.o7.codes/api/remote/*` to `o7-relay-preview`.
  The relays start with no rooms: a computer claims its room again when it connects and sends its
  bot links with it, and paired phones keep their keys, so nothing is lost but the open sockets.
- **Workers Builds** triggers moved with the Workers: `o7-web` has "Deploy main"
  (`8aff9db3-7378-4b24-be0e-f63d1bd4ada9`), `o7-web-preview` has "Deploy non-production branches"
  (`785e5f21-ad90-432d-b756-8e67062aa587`) and "Deploy main" (`c7cd721a-7f0a-4bb0-bb25-54ae13ce3ff9`),
  with the same build token and commands as before, building `@o7/web`. The old Workers' triggers
  were deleted, and the old Workers themselves once the new ones had served a merge.
- **Deleted**: the R2 bucket `auracode-releases`, empty and unused (releases are published to
  GitHub). **Unchanged**: the zones and their DNS, TLS, rate-limit and analytics rules, the
  account and its ids.

## Everything is Ghost (2026-11-26)

The owner bought onghost.dev and asked for the product to become Ghost ADE everywhere, and for
everything to move to that domain. The zone (`99e9603f5a9e6ffd580a383a065b29a3`) is on the same
account and was active the same day. What changed in Cloudflare and GitHub, each checked live
afterwards, and what could not be changed yet.

Done with the access at hand (Wrangler's own sign-in: Workers, routes and custom domains, no DNS,
rules or build settings):

- **The site** answers at `onghost.dev` and the preview at `preview.onghost.dev`, each as a
  custom domain of its Worker (`custom_domain = true` in `apps/web-worker/wrangler.toml`), so
  Cloudflare keeps their DNS records and certificates itself. `.dev` is on the browsers' own list
  of HTTPS-only domains, so no page there loads without TLS.
- **The relay** answers at `onghost.dev/api/remote/*` and `preview.onghost.dev/api/remote/*`
  (routes in `apps/relay/wrangler.toml`). A route on a custom domain's own hostname runs in front
  of it: the relay's answer comes back for those paths and the site's for every other, which was
  checked on the preview before production moved.
- **The addresses before** keep doing the one job auracode.si has done since the last move, and
  o7.codes now does it too: their `/api/remote/*` routes still reach the relay, which lets a
  browser in from either origin (`FORMER_ORIGINS`), and their `/*` routes still reach the site's
  Worker, which serves only `/remote` and the files it loads there and answers every other page
  with a 308 to onghost.dev (`FORMER_HOSTS`). One more thing is answered there: `/api/count`,
  because a copy from before the move counts itself at the address it knew, and would otherwise
  go uncounted until it updated. `preview.o7.codes` has no route any more (its route was deleted
  by hand: a deploy whose config lists only a custom domain leaves old routes in place).
- **The repository** is `itscool2b/ghost` (`gh repo rename`). GitHub redirects both earlier
  addresses, so the update feed that copies from before read still resolves, and the release
  feed keeps naming `itscool2b/auracode`, the one path every copy accepts (docs/name.md).
- **The updater's key** did not change. The repository variable that holds its public half was
  copied to `GHOST_UPDATER_PUBLIC_KEY`, which the workflows now read; the two earlier variables
  stay until no workflow run from before could need them.

Done later the same day, once the owner had signed in to the Cloudflare API (the rename itself
had only Wrangler's sign-in). Each was copied from what o7.codes has and checked live:

- **No injected analytics.** Ruleset `6ab62b9ab69f44759f786d18109c5ad5`, phase
  `http_config_settings`, turns Cloudflare's own page analytics off for `onghost.dev` and
  `preview.onghost.dev`. Before it, Cloudflare added its script to every page and the page's
  security policy refused it. The privacy page names the new address since.
- **The relay's rate limit.** Ruleset `e08127d5573147ad83dfc4d91126ac73`, phase
  `http_ratelimit`: a path starting `/api/remote/` gets at most 60 requests in 10 seconds from
  one address and Cloudflare location, then is refused for 10 seconds.
- **The domain says that it sends no mail.** `_dmarc` is `v=DMARC1; p=reject;` and the apex SPF
  is `v=spf1 -all`.
- **Always Use HTTPS is on.** `http://onghost.dev/` answers 301 to the same page over TLS.
- **The Workers are `ghost-web`, `ghost-web-preview`, `ghost-relay` and
  `ghost-relay-preview`**, renamed in place (`PATCH /accounts/{account}/workers/workers/{id}`
  with a new `name`). A Worker renamed so keeps its id, its routes and custom domains, its
  bindings, its Durable Objects with what they hold, and its build triggers, so nothing had to
  be copied or moved as on 19 November. The site's three build triggers now build `@ghost/web`,
  and `apps/web-builds-alias`, the package that stood in for the old name, is gone. The D1
  databases keep their names (`o7-counts`, `o7-counts-preview`): a database cannot be renamed.

**Do not let o7.codes or auracode.si lapse while any copy from before a move is still in use**,
for the reasons given above for auracode.si: whoever registers the domain next would answer that
copy's relay socket and serve whatever page they like at `/remote`.

The app's data folder is not moved by a rename (docs/name.md): a copy updated from o7 keeps
`codes.o7.desktop`, and one from the auracode days keeps `si.auracode.desktop`.
