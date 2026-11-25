# o7 for iPhone

o7 for iPhone (`apps/ios`) is a native app that pairs with o7 on a computer and follows and steers
its chats: what the browser remote at `o7.codes/remote` does, made for the phone. It speaks the
same sealed protocol through the same relay ([remote-sessions.md](remote-sessions.md)), so the
computer decides everything and the relay reads nothing of a chat.

The owner asked for it on 25 November 2026: native SwiftUI, iOS first, the interface above all,
and every part of o7 reachable from the phone. Android is a later, separate app.

## What a person does

1. **On the computer: Settings > Remote access > Pair a device.** The dialog shows a code.
2. **On the iPhone: Scan pairing code** (or paste the link shown under the code). The app reads only
   `https://o7.codes/remote#...` links, asks "Pair with this computer?" with a name for the phone, and
   pairs only after the tap. It shows each step as it happens: reaching the relay, proving the code,
   receiving the phone's key. Then it opens the computer, in the computer's own theme.
3. **Use it.** The Inbox shows what needs you (with Allow and Deny right there), what works (with the
   step it is on), and what ended today. A chat reads as it does on the computer: messages, steps,
   thoughts, plans, requests, the working line. Send, queue, stop, answer, start a chat, choose the
   model, thinking level or agent.
4. **More computers.** "Pair another computer" from the title's menu. Every paired computer keeps a
   session while the app is in front; the switcher says which need you.
5. **Forget a computer** in Settings: its key is deleted here and the computer stops accepting it.

Pairing a computer the phone already has says so ("Pair this iPhone again?"); the new key replaces
the old one, and the app asks the computer to forget the old device, so its Settings lists the
phone once.

## How it is built

| Part | Where | What it does |
|---|---|---|
| Messages | `O7Kit/Sources/O7Wire` | The protocol's messages as Swift types. Every enum reads values it does not know as its fallback, as the desktop's `lenient_enum!` does, so the app can be older or newer than the computer. |
| Sealed channel | `O7Kit/Sources/O7Crypto` | HKDF-SHA256 and AES-256-GCM as `crypto.rs` and `channel.ts` do them, held to the same vectors (`crates/o7-remote/tests/vectors.json`) and the same refusals. Pairing links are read only in their canonical form and only from o7's site. |
| Session | `O7Kit/Sources/O7Client` | One actor per computer: the relay's frames, the handshake (exactly one welcome per hello), sealing in send order through one writer, the first request held alone until the computer accepts the socket, keep-alive, waking a quiet socket, backing off. |
| Transcript | `O7Kit/Sources/O7Transcript` | The desktop's reducer (`apps/desktop/src/lib/chat/transcript.ts` and its helpers) ported line by line, held to goldens the real TypeScript writes (`apps/ios/scripts/write-goldens.mts`). |
| Keys | `O7Kit/Sources/O7Store` | Device keys in the Keychain, on this device only, readable after the first unlock; the list of computers beside them. A fresh install starts with none. |
| Look | `O7Kit/Sources/O7Theme`, `O7Marks` | The ten themes generated from `packages/ui/src/graphite/themes.ts`, the agent marks, the pixel loader, the logos, Geist and Geist Mono. |
| Words | `O7Markdown`, `O7Highlight` | Markdown from Foundation's parser; code coloured by the desktop's own highlight.js, run in JavaScriptCore. |
| A computer in a box | `O7Kit/Sources/O7MockHost` | The desktop's half of the protocol with a scripted agent, for tests, previews, screenshots and the app's mock mode (`-O7Backend mock`). |
| The app | `apps/ios/O7` | SwiftUI: pairing, the tabs, chats, settings. |

The relay gives the app a path of its own, `/api/remote/v1/app/<computer>`, which refuses any request
with an `Origin`, so no web page can use it, and tells the computer which connections came in on it
(`k<cid>`). The computer offers the app more than a browser (`APP_FEATURES`) only on such a
connection, and records the device as o7 for iPhone with its version.

## Look

Liquid Glass for the navigation layer (the tab bar, toolbars, the composer, the strip above the
tabs) and o7's own solid layers, type and marks for everything inside, in the computer's theme. See
"iPhone app composition" in [design/visual-language.md](design/visual-language.md).

## Checking it

- `swift test --package-path apps/ios/O7Kit`: the channel against the desktop's vectors, the
  session against the computer in a box (pairing, a spent link, a refused key, a computer that goes
  away and comes back, a live turn answered, a quiet socket replaced, an operation not offered),
  the transcript goldens, markdown, the theme's contrast.
- `node apps/ios/scripts/e2e.mjs`: the Swift client against the relay in the Workers runtime and
  the desktop's real connection task (`demo_host`), through the app path.
- The app on a simulator: `apps/ios/scripts/project.sh`, then build and run the `O7` scheme; the UI
  tests run against the computer in a box.
- `.github/workflows/ios-app.yml` (by hand): all of the above on GitHub's Macs.
