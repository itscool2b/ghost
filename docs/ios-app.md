# Ghost for iPhone

Ghost for iPhone (`apps/ios`) is a native app that pairs with Ghost on a computer and follows and steers
its chats: what the browser remote at `onghost.dev/remote` does, made for the phone. It speaks the
same sealed protocol through the same relay ([remote-sessions.md](remote-sessions.md)), so the
computer decides everything and the relay reads nothing of a chat.

The owner asked for it on 25 November 2026: native SwiftUI, iOS first, the interface above all,
and every part of Ghost reachable from the phone. Android is a later, separate app.

## What a person does

1. **On the computer: Settings > Remote access > Pair a device.** The dialog shows a code.
2. **On the iPhone: Scan pairing code** (or paste the link shown under the code). The app reads only
   `https://onghost.dev/remote#...` links, asks "Pair with this computer?" with a name for the phone, and
   pairs only after the tap. It shows each step as it happens: reaching the relay, proving the code,
   receiving the phone's key. Then it opens the computer, in the computer's own theme.
3. **Use it.** The Inbox shows what needs you (with Allow and Deny right there), what works (with the
   step it is on), and what ended today. A chat reads as it does on the computer: messages, steps,
   thoughts, plans, requests, the working line. Send, queue, stop, answer, start a chat, choose the
   model, thinking level or agent.
4. **Look at the work.** In a chat, **Changes** lists the files the last turn changed, or for a chat
   on its own branch everything the branch changed; each opens as a diff (hunks with their line
   numbers, unchanged stretches folded, the changed words marked, the code in the theme's colours).
   Above them: the branch, what is not committed, the pull request with its checks and the reviews
   of people the repository trusts, and the agent's usage. **Screen** shows the chat's browser page or
   device, taken again every 3 seconds while it is open.
5. **Steer it.** From a chat's menu: the access level (loosening it asks first, and with "Face ID to
   allow" on, asks for Face ID), rename, archive (archived chats are under the project's
   **Archived**), start a stopped agent. A project's **Brief** edits the part of its memory every chat
   there receives, at most 4 KB, saved as the person's own; if it changed on the computer meanwhile,
   the app says so and lets the person keep theirs or take the computer's.
6. **Hand work over.** **New work** starts a chat, or with **Own branch** a task in a worktree of its
   own, reviewed by a second agent when done, started now or **tonight** (between 1 and 6 in the
   morning on the computer's clock). **Tonight** lists what is queued, what could not start and why,
   and the tasks handed over; a swipe cancels a queued one.
7. **Photos.** The composer's **+** (and New work's) adds photos from the library or the camera, up
   to 8 a message. Each goes as a JPEG at most 1,600 points on its longer side, without its
   location, in chunks the relay's limits allow, and is checked against its SHA-256 on the computer.
   The message shows them at once and keeps them; the history the computer sends carries words only,
   so photos sent from the computer show as "Images".
8. **More computers.** "Pair another computer" from the title's menu. Every paired computer keeps a
   session while the app is in front; the switcher says which need you.
9. **Forget a computer** in Settings: its key is deleted here and the computer stops accepting it.

Each of these needs the computer's Ghost to offer it (`ready.features`: `control`, `review`, `memory`,
`screens`, `tasks`, `attachments`). With an older Ghost the app leaves the control out and never sends
the operation.

## Notifications

The app asks once, right after the first pairing ("Know when an agent needs you"), or from
Settings > Notifications. Each computer then knows where Apple delivers this phone's notifications
(`push_register`) and what the person wants from it: Needs you, Finished, Failed, Tasks, for every
chat or only those started or answered from this phone.

- **Sealed end to end.** The computer seals each notification's words for this phone with a key
  derived from the device key (HKDF-SHA256, `o7 push v1`), bound to the computer and the device.
  The relay and Apple carry a blob they cannot read; the outside of every notification is the same
  fixed "Ghost" and "Something changed on your computer."
- **Opened on the phone.** The notification extension (`GhostNotificationService`) opens it with the
  push key, which the app keeps in the App Group's Keychain group; the device key itself never
  leaves the app's own Keychain. It writes the chat as the title, the project and agent under it,
  and the ask or the result as the body, one thread per chat. A repeat or an older one than the
  newest from that computer comes in quietly. Before the phone's first unlock after it starts, the
  key cannot be read: the fixed words stay, with no buttons.
- **Answering.** A permission gets **Allow once** and **Deny**, a yes-or-no question **Yes** and
  **No**, a question in words a text field; each asks for the phone's own unlock first, and only a
  notification that opened gets buttons, so nothing is allowed unread. The app answers through its
  session, waking it if it slept; when the computer cannot be reached, a notification says so.
- **In the app.** No banner for the chat on screen. A notification's tap opens its chat.
- **Live Activity.** While agents work, the Lock Screen and the Dynamic Island show each computer's
  counts and the leading chat ("Studio: 1 needs you · 1 working"), with a timer. The app keeps it
  current while it runs; the computer then sends its updates through Apple, sealed the same way
  (`activity`). Off in Settings > Notifications.
- **Widgets.** Needs you, small and medium, and on the Lock Screen: counts and up to three chats
  from what the app and the extension last wrote to the App Group. A tap opens the chat
  (`ghost://chat/<computer>/<chat>`) or the Inbox (`ghost://inbox`).

A simulator does not run the notification extension for a simulated push.
`node apps/ios/scripts/push.mjs --udid <simulator> [--buttons]` sends one sealed for the demo
computer (`-GhostBackend mock`), whose ids and key are fixed in debug builds: Apple's fixed words show,
and with `--buttons` the category the extension would set, so the buttons and the app's answer can
be tried. The extension's own opening is covered by `GhostPushTests`, and on a phone by a real push
once the Apple team is set.

Pairing a computer the phone already has says so ("Pair this iPhone again?"); the new key replaces
the old one, and the app asks the computer to forget the old device, so its Settings lists the
phone once.

## How it is built

| Part | Where | What it does |
|---|---|---|
| Messages | `GhostKit/Sources/GhostWire` | The protocol's messages as Swift types. Every enum reads values it does not know as its fallback, as the desktop's `lenient_enum!` does, so the app can be older or newer than the computer. |
| Sealed channel | `GhostKit/Sources/GhostCrypto` | HKDF-SHA256 and AES-256-GCM as `crypto.rs` and `channel.ts` do them, held to the same vectors (`crates/ghost-remote/tests/vectors.json`) and the same refusals. Pairing links are read only in their canonical form and only from Ghost's site. |
| Session | `GhostKit/Sources/GhostClient` | One actor per computer: the relay's frames, the handshake (exactly one welcome per hello), sealing in send order through one writer, the first request held alone until the computer accepts the socket, keep-alive, waking a quiet socket, backing off. |
| Transcript | `GhostKit/Sources/GhostTranscript` | The desktop's reducer (`apps/desktop/src/lib/chat/transcript.ts` and its helpers) ported line by line, held to goldens the real TypeScript writes (`apps/ios/scripts/write-goldens.mts`). |
| Keys | `GhostKit/Sources/GhostStore` | Device keys in the Keychain, on this device only, readable after the first unlock; the list of computers beside them. A fresh install starts with none. |
| Look | `GhostKit/Sources/GhostTheme`, `GhostMarks` | The ten themes generated from `packages/ui/src/graphite/themes.ts`, the agent marks, the pixel loader, the logos, Geist and Geist Mono. |
| Words | `GhostMarkdown`, `GhostHighlight` | Markdown from Foundation's parser; code coloured by the desktop's own highlight.js, run in JavaScriptCore. |
| A computer in a box | `GhostKit/Sources/GhostMockHost` | The desktop's half of the protocol with a scripted agent, for tests, previews, screenshots and the app's mock mode (`-GhostBackend mock`). |
| The app | `apps/ios/Ghost` | SwiftUI: pairing, the tabs, chats, settings. |

The relay gives the app a path of its own, `/api/remote/v1/app/<computer>`, which refuses any request
with an `Origin`, so no web page can use it, and tells the computer which connections came in on it
(`k<cid>`). The computer offers the app more than a browser (`APP_FEATURES`) only on such a
connection, and records the device as Ghost for iPhone with its version.

## Look

Liquid Glass for the navigation layer (the tab bar, toolbars, the composer, the strip above the
tabs) and Ghost's own solid layers, type and marks for everything inside, in the computer's theme. See
"iPhone app composition" in [design/visual-language.md](design/visual-language.md).

## Checking it

- `swift test --package-path apps/ios/GhostKit`: the channel against the desktop's vectors, the
  session against the computer in a box (pairing, a spent link, a refused key, a computer that goes
  away and comes back, a live turn answered, a quiet socket replaced, an operation not offered, a
  photo uploaded in chunks, a brief saved only over the version it was read from, tonight's queue,
  rename and archive, changes and diffs), the transcript goldens, markdown, the theme's contrast.
- `node apps/ios/scripts/e2e.mjs`: the Swift client against the relay in the Workers runtime and
  the desktop's real connection task (`demo_host`), through the app path.
- The app on a simulator: `apps/ios/scripts/project.sh`, then build and run the `Ghost` scheme; the UI
  tests run against the computer in a box (pairing, answering, sending, queueing and cancelling
  tonight's work, changes and a diff, renaming, editing a brief, and the accessibility audit).
- `.github/workflows/ios-app.yml` (by hand): all of the above on GitHub's Macs.

On 26 November 2026 the UI tests do not all pass there. Four fail on the simulator (opening a chat
and sending, changes and a diff, renaming, and the accessibility audit), and a fifth (answering a
permission from the Inbox) fails on some runs. They failed before that day too, unseen: the step
piped `xcodebuild` into `tail` under a shell without `pipefail`, so it passed whatever the tests
did. The step fails now when they do. GhostKit's tests and the app's build pass.
