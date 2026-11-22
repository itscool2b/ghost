# Third-party notices

These files preserve third-party terms. They do not select, grant or change a license for o7,
whose own license is the [MIT License](../LICENSE) at the repository root.

`catalogue.json` contains the locked production dependency closure of the desktop and website,
source archive links and SHA-256 hashes. `texts.json` preserves the original license texts. The desktop set covers Windows
x64/ARM64 and macOS x64/ARM64. The website set contains only its frontend production dependencies.
The three bundled Fontsource packages retain their full OFL text and copyright notices. OGL's full
Unlicense section comes from its installed README. This is a conservative manifest-based inventory;
tree shaking and native linking can remove components from a particular binary.

Rust traversal follows normal dependency edges and excludes build-only dependencies, test
dependencies and procedural macro executables. Playwright, Biome, Sharp, Cargo build helpers and
the test-only Codex schema are not added to runtime notices. The schema instead has its own pinned
upstream `LICENSE` and `NOTICE` beside the fixture.

Dictation's speech engine, transcribe.cpp, vendors ggml, miniz and Silero VAD's segmentation code;
each gets its own record from the text in the locked `transcribe-cpp-sys` archive. The speech models
the desktop bundles as resources are listed in `models.json`, and
`apps/desktop/scripts/dictation-models.mjs` pins them. They are model weights, not code. Each
record keeps the model's license, its converter's NOTICE and an attribution, from texts preserved
in `upstream/`. The generator fails when the pinned models and that list disagree.

The desktop also bundles one prebuilt program that is not built from this repository: the scrcpy
server (Genymobile, Apache-2.0), which o7 puts on a person's Android device to show its screen
and take input in the Device pane. `scrcpy.json` pins its release and the SHA-256 the release
publishes, `crates/o7-devices` refuses any other bytes, and its LICENSE is preserved in
`upstream/`. The generator fails when the bundled file and that pin disagree. Android's own tools
(adb, the emulator, a system image) are not bundled: o7 fetches them from Google on the person's
own computer, after the person has read and accepted Google's SDK licence in the pane.

An npm package whose package.json names no license gets the license its own license file states,
reviewed into `NPM_LICENSES` (khroma, MIT); any other stops the refresh. `NPM_EXCLUDED` lists packages
a dependency names that o7 does not ship, with why and what keeps them out: elkjs (EPL-2.0 or
GPL-3.0), which Mermaid imports only for its optional ELK layout, is replaced by a stub through the
desktop's Vite alias, and the generator fails if that alias goes.

Some published Rust archives omit their license files. `overrides.json` identifies the exact
`.cargo_vcs_info.json` revision and the authoritative upstream source used for each replacement.
`upstream/` preserves those texts with hashes. The standard MPL-2.0 text for selectors is identical
to the text in the locked cssparser archive; selectors' own source header names MPL-2.0. Source
archive links in the notices identify the corresponding unmodified MPL-covered source.

## Reproduce

Install dependencies from the lockfiles first. Refreshing reads installed npm packages and Cargo's
registry sources. Cargo metadata is offline and does not compile or run the application. No vendor
configuration or credentials are read, and the generator performs no network fetches.

```sh
node .github/scripts/third-party-notices.mjs --write
node .github/scripts/third-party-notices.mjs
node --test .github/scripts/third-party-notices.test.mjs
```

Review changed dependencies and original texts before committing a refresh. An absent license or
changed upstream revision fails closed. Both Vite builds verify the recorded lockfile/manifest
hashes and emit `third-party-notices.txt` and a dark, script-free `third-party-notices/index.html`
alongside their frontend assets. No dependency resolution or download runs from the Vite hook.

The website footer links to `/third-party-notices`. The desktop app shows the same text from
Settings, About, Third-party notices. It loads `virtual:third-party-notices` as a lazy script chunk
because the desktop CSP allows no same-origin fetches.

## Remaining release qualification

- The frontend builds emit and link the notices, but the final desktop installers and deployed
  website outputs have not yet been inspected with this checkpoint.
- Verify WebView2Loader's Microsoft SDK redistribution terms and exact bundled loader version.
  The Rust `webview2-com` wrapper's MIT text does not substitute for those terms. The independently
  downloaded WebView2 runtime/bootstrapper also needs its installer review.
- Inspect actual Windows and macOS release artifacts for other bundled native SDK files, linked
  libraries, installer components and system-vs-bundled crypto. This inventory is not a completed
  binary-level compliance certification. In particular, a custom OpenSSL override on any platform
  needs its own matched notice: no OpenSSL is vendored into the product.
- Keep unmodified third-party source archives available at the listed source locations when
  distributing MPL-covered binaries. Modified dependencies would require a new source review.

These gaps do not authorize signing or an unreviewed release.
