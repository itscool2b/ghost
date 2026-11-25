# o7 for iPhone

A native SwiftUI app that pairs with o7 on a computer and follows and steers its chats, as the
browser remote at `o7.codes/remote` does. How it works and what it may do is in
[docs/ios-app.md](../../docs/ios-app.md); the protocol it speaks is in
[docs/remote-sessions.md](../../docs/remote-sessions.md).

## Layout

| Folder | What it holds |
|---|---|
| `O7Kit/` | A Swift package with everything that is not a screen, testable on a Mac: `O7Wire` (the messages), `O7Crypto` (the sealed channel and pairing links), `O7Client` (one computer's session), `O7Transcript` (the desktop's transcript reducer, ported), `O7Store` (paired computers and their keys), `O7Theme` (the ten themes, generated), `O7Marks` (agent marks, the pixel loader, logos, fonts), `O7Markdown`, `O7Highlight` (highlight.js in JavaScriptCore), `O7MockHost` (a computer in a box). |
| `O7/` | The app: `App/` (models, entry), `Design/` (style and shared parts), `Pairing/`, `Tabs/`, `Chat/`, `Settings/`. |
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
```

`-O7Backend mock` runs the app against a computer in a box (`O7MockHost`), with
`-O7MockPaired` already paired: the screens, UI tests and screenshots use it. A development build
also takes a relay of its own in Settings > Developer (for example `ws://127.0.0.1:8799`) and
pairing links from `localhost` and `preview.o7.codes`; a release build pairs only with
`https://o7.codes`.
