# Ghost for iPhone

A native SwiftUI app that pairs with Ghost on a computer and follows and steers its chats, as the
browser remote at `onghost.dev/remote` does. How it works and what it may do is in
[docs/ios-app.md](../../docs/ios-app.md); the protocol it speaks is in
[docs/remote-sessions.md](../../docs/remote-sessions.md).

## Layout

| Folder | What it holds |
|---|---|
| `GhostKit/` | A Swift package with everything that is not a screen, testable on a Mac: `GhostWire` (the messages), `GhostCrypto` (the sealed channel and pairing links), `GhostClient` (one computer's session), `GhostTranscript` (the desktop's transcript reducer, ported), `GhostStore` (paired computers and their keys), `GhostTheme` (the ten themes, generated), `GhostMarks` (agent marks, the pixel loader, logos, fonts), `GhostMarkdown`, `GhostHighlight` (highlight.js in JavaScriptCore), `GhostMockHost` (a computer in a box), `GhostPush` (sealed notifications opened on the phone, their words, the keys shared with the notification extension, the widgets' snapshot, the Live Activity's state). |
| `Ghost/` | The app: `App/` (models, entry, notifications), `Design/` (style and shared parts), `Pairing/`, `Tabs/`, `Chat/`, `Settings/`. |
| `GhostNotificationService/` | The notification extension: opens each sealed notification and writes its words. |
| `GhostWidgets/` | The Needs you widget and the Live Activity. |
| `GhostTests/`, `GhostUITests/` | Tests run on a simulator. |
| `scripts/` | Generators (`write-*.mts`, each with `--check`), the project generator and the end-to-end harness. |
| `Config/` | Build settings. A team for builds on a phone goes in `Signing.local.xcconfig`, which is not committed. |

## Working on it

```sh
# The package's tests, on this Mac (no simulator needed).
swift test --package-path apps/ios/GhostKit

# The Xcode project, from project.yml (XcodeGen 2.45.4; the project is not committed).
apps/ios/scripts/project.sh
open apps/ios/Ghost.xcodeproj

# Build for a simulator from the command line.
xcodebuild -project apps/ios/Ghost.xcodeproj -scheme Ghost \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' build

# The client end to end against the relay and the stand-in desktop.
pnpm --filter @ghost/relay worker:build
cargo build -p ghost-remote --example demo_host
node apps/ios/scripts/e2e.mjs

# Generated sources match where they come from.
pnpm --filter @ghost/ios check

# A sealed notification to the app on a simulator, as if from the demo computer. A simulator
# does not run the notification extension for it, so Apple's fixed words show; --buttons adds
# the buttons the extension would set, to try answering from a notification.
node apps/ios/scripts/push.mjs --udid <simulator> [--kind asking|finished|failed|resolved] [--buttons]

# Screenshots of every theme, light and dark, at two text sizes (minutes; kept in the results).
TEST_RUNNER_GHOST_SCREENSHOTS=1 xcodebuild test -project apps/ios/Ghost.xcodeproj -scheme Ghost \
  -destination 'platform=iOS Simulator,name=iPhone 17 Pro' -only-testing:GhostUITests/ScreenshotTests
```

`-GhostBackend mock` runs the app against a computer in a box (`GhostMockHost`), with
`-GhostMockPaired` already paired: the screens, UI tests and screenshots use it. A development build
also takes a relay of its own in Settings > Developer (for example `ws://127.0.0.1:8799`) and
pairing links from `localhost` and `preview.onghost.dev`; a release build pairs only with
`https://onghost.dev`.

## What needs the owner's Apple account

Everything above runs on a simulator without a team. These need the Apple Developer team, and
only the owner does them:

1. **Team.** Put the Team ID in `Config/Signing.local.xcconfig` (`DEVELOPMENT_TEAM = <id>`) and
   in `ASSOCIATION` in `apps/web-worker/src/lib.rs` (the Universal Links file, `<id>.dev.onghost.mobile`).
   Automatic signing then turns on Push Notifications, App Groups (`group.dev.onghost.mobile`),
   Associated Domains (`applinks:onghost.dev`) and Time Sensitive Notifications for the app and its
   two extensions.
2. **Push key.** Make an APNs key (Certificates, Identifiers & Profiles > Keys), put its id and
   the team's in `APNS_KEY_ID` and `APNS_TEAM_ID` in `apps/relay/wrangler.toml`, and the key
   itself with `wrangler secret put APNS_KEY --env preview` (and `--env production`); see
   "Notifications" in `apps/relay/README.md`.
3. **Deploy the relay** (`wrangler deploy --env preview`, then `--env production`).
4. **Try a real push** on a phone: pair, turn notifications on, and use Send test notification
   in Ghost's Settings on the computer.
5. **TestFlight** (the owner's choice for now, 25 November 2026). Archive the `Ghost` scheme
   (Release) and upload it from Xcode's Organizer. Each upload needs a new build number: let the
   Organizer raise it, or raise `CURRENT_PROJECT_VERSION` in `project.yml`.
   - A TestFlight build is a release build. It pairs only with `https://onghost.dev`, so the relay
     must be deployed to production, and the computers need a desktop release that has these
     changes.
   - Its notifications go through Apple's production service, so the push key must be set with
     `--env production`.
   - App Store Connect asks about encryption for every build until `ITSAppUsesNonExemptEncryption`
     in `Ghost/Info.plist` answers it. That answer is the owner's to give.
   - Internal testers can install a build at once. External testers need Beta App Review, which
     asks for test information such as a privacy policy address.
6. **Later, for the App Store**: the listing, screenshots (the iPhone app workflow's screenshots
   input), the privacy label, then the desktop tour stop and a Smart App Banner on `/remote`, which
   need the App Store link.
