# AXe

o7 drives iPhone simulators on a Mac with AXe (<https://github.com/cameroncooke/AXe>, by Cameron
Cooke): it reads the simulator's screen, sends taps, typing, keys and buttons, and streams the
pictures the Device pane shows (`crates/o7-devices/src/ios.rs`, `docs/mobile.md`).

**o7 does not ship AXe.** On an iPhone's first start, o7 downloads one pinned release from AXe's
own GitHub release page into its data folder (`crates/o7-devices/src/axe.rs`):

| | |
| --- | --- |
| Version | 2.0.4 (released 2026-11-12) |
| File | `AXe-macOS-v2.0.4-universal.tar.gz` |
| Address | https://github.com/cameroncooke/AXe/releases/download/v2.0.4/AXe-macOS-v2.0.4-universal.tar.gz |
| Size | 16,526,761 bytes |
| SHA-256 | `3178ec66d9ff46455fec737debad719c41cab16edb1435b2c721e07760a208c3` |
| Licence | MIT (`LICENSE` at the `v2.0.4` tag, "Copyright (c) 2025 Cameron Cooke") |

The archive holds the `axe` program, Meta's IDB frameworks it is built on (`FBControlCore`,
`FBSimulatorControl`, MIT, listed in AXe's own `THIRD_PARTY_LICENSES`) and its resource bundle.

Why this version: it is the last release under the MIT licence. From 2.1.0 (2026-11-19) AXe is
GPL-3.0-only, with a commercial licence for software that is not released under the GPL. 2.0.4
supports Xcode 26 and 27, which is what o7 needs. Moving to a later release means checking its
licence again, then changing `VERSION`, `URL`, `SHA256` and `SIZE` in `axe.rs` and this record
together, and running the **iPhone simulator** workflow.

The download is checked before anything in it is used: HTTPS only, redirects followed only to
GitHub's own hosts, the size and the SHA-256 above compared, and only then unpacked by macOS's own
`tar` into an empty folder.
