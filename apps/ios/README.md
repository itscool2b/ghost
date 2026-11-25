# o7 for iPhone

A native SwiftUI app that pairs with o7 on a computer and follows and steers its chats, as the
browser remote at `o7.codes/remote` does. How it works and what it may do is in
[docs/ios-app.md](../../docs/ios-app.md); the protocol it speaks is in
[docs/remote-sessions.md](../../docs/remote-sessions.md).

## Layout

| Folder | What it holds |
|---|---|
| `O7Kit/` | A Swift package with everything that is not a screen, testable on a Mac: `O7Wire` (the messages), `O7Crypto` (the sealed channel and pairing links), `O7Client` (one computer's session), `O7Transcript` (the desktop's transcript reducer, ported), `O7Store` (paired computers and their keys), `O7Theme` (the ten themes, generated), `O7Marks` (agent marks, the pixel loader, logos, fonts), `O7Markdown`, `O7Highlight` (highlight.js in JavaScriptCore), `O7MockHost` (a computer in a box), `O7Push` (sealed notifications opened on the phone, their words, the keys shared with the notification extension, the widgets' snapshot, the Live Activity's state). |
| `O7/` | The app: `App/` (models, entry, notifications), `Design/` (style and shared parts), `Pairing/`, `Tabs/`, `Chat/`, `Settings/`. |
| `O7NotificationService/` | The notification extension: opens each sealed notification and writes its words. |
| `O7Widgets/` | The Needs you widget and the Live Activity. |
| `O7Tests/`, `O7UITests/` | Tests run on a simulator. |
| `scripts/` | Generators (`write-*.mts`, each with `--check`), the project generator and the end-to-end harness. |
| `Config/` | Build settings. A team for builds on a phone goes in `Signing.local.xcconfig`, which is not committed. |

## Working on it

```sh
# The package's tests, on this Mac (no simulator needed).
swift test --package-path apps/ios/O7Kit

# The Xcode project, from project.yml (XcodeGen 2.45.4; the project is not committed).
apps/ios/scripts/project.sh
open apps/ios/O7.xcodeproj

# Build for a simulator from the command line.
xcodebuild -project apps/ios/O7.xcodeproj -scheme O7 \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' build

# The client end to end against the relay and the stand-in desktop.
pnpm --filter @o7/relay worker:build
cargo build -p o7-remote --example demo_host
node apps/ios/scripts/e2e.mjs

# Generated sources match where they come from.
pnpm --filter @o7/ios check

# A sealed notification to the app on a simulator, as if from the demo computer. A simulator
# does not run the notification extension for it, so Apple's fixed words show; --buttons adds
# the buttons the extension would set, to try answering from a notification.
node apps/ios/scripts/push.mjs --udid <simulator> [--kind asking|finished|failed|resolved] [--buttons]

# Screenshots of every theme, light and dark, at two text sizes (minutes; kept in the results).
TEST_RUNNER_O7_SCREENSHOTS=1 xcodebuild test -project apps/ios/O7.xcodeproj -scheme O7 \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' -only-testing:O7UITests/ScreenshotTests
```

`-O7Backend mock` runs the app against a computer in a box (`O7MockHost`), with
`-O7MockPaired` already paired: the screens, UI tests and screenshots use it. A development build
also takes a relay of its own in Settings > Developer (for example `ws://127.0.0.1:8799`) and
pairing links from `localhost` and `preview.o7.codes`; a release build pairs only with
`https://o7.codes`.

## What needs the owner's Apple account

Everything above runs on a simulator without a team. These need the Apple Developer team, and
only the owner does them:

1. **Team.** Put the Team ID in `Config/Signing.local.xcconfig` (`DEVELOPMENT_TEAM = <id>`) and
   in `ASSOCIATION` in `apps/web-worker/src/lib.rs` (the Universal Links file, `<id>.codes.o7.mobile`).
   Automatic signing then turns on Push Notifications, App Groups (`group.codes.o7.mobile`),
   Associated Domains (`applinks:o7.codes`) and Time Sensitive Notifications for the app and its
   two extensions.
2. **Push key.** Make an APNs key (Certificates, Identifiers & Profiles > Keys), put its id and
   the team's in `APNS_KEY_ID` and `APNS_TEAM_ID` in `apps/relay/wrangler.toml`, and the key
   itself with `wrangler secret put APNS_KEY --env preview` (and `--env production`); see
   "Notifications" in `apps/relay/README.md`.
3. **Deploy the relay** (`wrangler deploy --env preview`, then `--env production`).
4. **Try a real push** on a phone: pair, turn notifications on, and use Send test notification
   in o7's Settings on the computer.
5. **Ship**: archive and upload from Xcode; App Store listing, screenshots, privacy labels and
   export compliance (standard encryption only).
