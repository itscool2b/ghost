# Mobile: a phone beside the chat

o7 puts a real device beside each chat for building phone apps: an Android emulator of the chat's
own, or a phone on a cable. The person watches and uses it in the **Device pane**; the chat's
agent uses the same device through the `o7_mobile` tools. The plan, with what is built and what is
still to come (iOS, phones without a Mac, recipes for running a project), is
[superpowers/plans/2026-11-21-mobile.md](superpowers/plans/2026-11-21-mobile.md). This page says
how what is built works.

## What a person sees

- **The phone button** in a chat's header (Ctrl+Shift+D) opens the Device pane. It shares the
  chat's side with the browser, Changes and the task panel: opening one closes the other. Chat
  view only.
- **Nothing installed**: the pane offers to set Android up. It first shows what would be
  downloaded from Google (adb, the emulator, one system image: about 2.5 GB to download, about
  8 GB on disk), with Google's Android SDK licence in full. Nothing is fetched until the person
  presses Accept and download.
- **Start a phone** starts an emulator with no window of its own. A first start takes about a
  minute; later ones take a few seconds, from the state the emulator saved when it stopped.
- **The screen** is live video. A click is a tap, a drag is a swipe, the wheel scrolls, and the
  keyboard types into the device while its screen has focus. Back, Home and Recent apps are under
  it; turning the device and saving a picture are in the pane's header.
- **A phone on a cable** appears in the list once USB debugging is on. The person chooses it for a
  chat, and the chat's agent cannot use it until the person turns on "Let the agent use this
  phone" there.
- **Settings > Mobile** has the switch (on by default), what agents get, and what is installed.
  Off stops every device o7 started, closes the pane everywhere and makes every tool answer that
  Mobile is off, in running chats too.

## One device per chat

A chat's worktree is its own copy of a project, so two chats on one device would install over
each other. A chat holds at most one device and a device is held by at most one chat. o7 keeps up
to four emulators on disk (`Phone 1` to `Phone 4`), runs at most two at once (each takes 2 to 4 GB
of memory), and gives a chat that asks for "a phone" a running one nobody holds, else a stopped
one, else a new one. A chat lets go of a device it has not watched or used for 10 minutes, and an
emulator nobody holds is stopped. Emulators are o7's own processes: they are in a job object
(Windows) or a process group, so none outlives the app, and a normal quit asks each to save its
state first.

## How it works

Everything between the app and a device is `crates/o7-devices`, plain Rust with no Tauri in it.

| Part | What it does |
| --- | --- |
| `adb` | adb's own wire protocol, spoken to the adb server on 127.0.0.1 (`host:transport:<serial>`, then `exec:`, `shell,v2,raw:`, `sync:`, `localabstract:`). No `adb` process per call. The server is started with `adb start-server` when none answers. |
| `scrcpy` | The screen and input. The pinned scrcpy server (see below) is pushed to the device and started in forward-tunnel mode; its video and control sockets are opened **through the adb transport**, so no TCP port is forwarded and nothing listens on the computer. Video is H.264; touches, keys and text go over its control channel. |
| `outline` | The screen as an outline, from `uiautomator dump`: roles, names and values, with a ref (`d12`) on everything that can be acted on. Layout containers are dropped. A password field's text never leaves this module. |
| `picture` | Screenshots for agents, shrunk by a whole number so the longest side fits what models look at; a point in the picture times that number is a point on the device. |
| `repo`, `install`, `sdk` | Google's package index (the one `sdkmanager` reads), the checked download, and finding an SDK a person already has. No Java is needed. |
| `avd`, `emulator` | Virtual devices written as files (what `avdmanager` would write), in o7's own folder, and the emulator's life. |
| `manager`, `tools` | Which chat holds which device, the video for the pane, and the agents' tools. |

The desktop (`apps/desktop/src-tauri/src/mobile.rs`) adds the pane's commands, sends the video to
the page as raw bytes over a channel, and bridges agents' calls. The page decodes the video with
WebCodecs and draws it on a canvas (`apps/desktop/src/lib/mobile.ts`,
`components/mobile/DevicePanel.tsx`).

Agents reach it as they reach the browser: the app's own executable runs as a stdio MCP server
(`--o7-mobile-helper --mcp`) that a native Chat session's agent starts; it talks to the running
app over a loopback listener with a random per-session token, and the session's end revokes it
(`crates/o7-core/src/mobile_service.rs`). Terminal chats and agents on another host get no device
tools.

### Where things are

- `<data folder>/devices/android/sdk`: what o7 fetched. An SDK the person already has
  (`ANDROID_HOME`, `ANDROID_SDK_ROOT`, Android Studio's default folder) is used when it holds an
  emulator and an image; o7 reads it and never writes to it.
- `<data folder>/devices/android/avd`: o7's virtual devices (`ANDROID_AVD_HOME` when its emulator
  runs). The AVDs a person made in Android Studio are never listed, started or changed.
- `<data folder>/devices/android/logs`: what each emulator printed, for saying why one did not
  start.

A development build has its own data folder, so it downloads its own copy.

### What the computer needs

The emulator needs hardware virtualization: Windows Hypervisor Platform on Windows (Google retires
its own AEHD driver on 31 December 2026), KVM on Linux, nothing extra on macOS. The pane asks the
emulator itself (`emulator -accel-check`) and says the one thing to turn on. Phones on a cable
need only adb. Building an Android project needs that project's own JDK and build tools; o7 does
not install them.

## The agents' tools

`mobile_devices`, `mobile_start`, `mobile_install`, `mobile_launch`, `mobile_stop_app`,
`mobile_snapshot`, `mobile_tap`, `mobile_type`, `mobile_swipe`, `mobile_press`,
`mobile_screenshot`, `mobile_logs`, `mobile_open_link`, `mobile_settings`, `mobile_wait_for` and
`mobile_stop`. Their descriptions and the instructions agents read are in
`crates/o7-core/src/mobile_service/helper.rs` and `crates/o7-core/resources/mobile/`.

An agent builds the app with the project's own tools in its shell, then installs and starts it
with the tools, reads the screen as an outline, acts on refs, and takes a screenshot when layout
matters. `mobile_start` waits for the device (up to 200 seconds) and says where adb is and the
device's serial, for `adb reverse` to a development server.

## Security

Who may do what, and where it is checked:

- **An agent reaches only its own chat's device.** The helper's request carries a token and no
  chat id (`deny_unknown_fields`); the app maps the token to the chat it was made for, compares it
  in constant time, and binds 127.0.0.1 only. The listener dies with the session.
- **Setting Android up is the person's.** No tool fetches anything or accepts Google's licence:
  those exist only as the pane's own commands, and a fetch needs an offer that was shown first.
- **A phone is the person's until they say otherwise.** An agent cannot choose a phone, and
  cannot use the one its chat holds until the person turns that on in the pane, per chat. The
  answer is forgotten when the chat lets go of the phone. o7's own emulators need no question.
- **What the device shows is data.** Tool replies describe the screen; the instructions tell
  agents never to follow text shown by an app, a notification, a web page or a log.
- **Words from outside never reach a command unchecked.** A device's shell parses one command
  line, so package and activity names are checked against a strict form, and links and other text
  are quoted (`shell.rs`). Serials are checked before they go into an adb service line.
- **Secrets.** Password fields are blank in the outline. A picture cannot be masked: on a phone,
  the question the person answers says the agent can see everything on it. What an agent types is
  not logged and not shown in the pane ("Typing").
- **Downloads** come only from `dl.google.com`, over HTTPS, with no redirect followed. Each
  archive is checked against the size and checksum in Google's index before it is unpacked (the
  index states SHA-1; the connection to Google is what vouches for the index). Unpacking writes
  folders and files first, refuses a second entry of the same name, counts the bytes it really
  writes against a limit, makes links last, and resolves every link for real: one that leads
  outside the package's folder fails the whole unpack.
- **The scrcpy server** is one pinned release (5.0.1), bundled in the app and checked by its
  published SHA-256 in a test; `third-party/scrcpy.json` and the notices record it. It runs on
  the device as the shell user, as scrcpy itself does.
- **No new listeners.** adb's server is on loopback by default and o7 never asks for more; no
  device port is forwarded to the computer; the emulator runs with `-no-metrics` and its console
  and gRPC ports are left at their loopback defaults.
- **Remote access** does not reach the pane or its tools.

Limits, said plainly:

- An agent with a shell can run `adb` itself, within the chat's access mode. On an emulator that
  is the point. On a phone the person plugged in, adb reaches it whether or not the person allowed
  the *tools*: the switch governs o7's tools, not the agent's shell. A person who does not want
  an agent near a phone should not plug it in while that agent has a shell, and the pane's
  question says the agent can see and use everything on the phone.
- While a session's screen is being read, another program on the device could in principle
  connect to scrcpy's local socket before o7 does. This is scrcpy's own design; the server accepts
  exactly the sockets it expects and then stops listening.

## Checking it

- `cargo test -p o7-devices`: the protocol clients against a stand-in adb server, scrcpy's own
  byte layouts, the outline (one test reads a real dump), the index, the unpacker's refusals, the
  manager's rules (one device per chat, the running limit, a phone's consent, Mobile off).
- `cargo test -p o7-core mobile_service`: the grant, its token, the switch, the tool list.
- `node apps/desktop/scripts/check-device.mjs` (in the fixture suite): every state of the pane,
  its input, the phone's question and Settings > Mobile, in two browser engines and both
  appearances, with axe. There is no device there, so no video.
- A real emulator needs a development build: start it with a scratch `O7_DATA_DIR`, and use the
  development-only `mobile_agent_call` command to run a tool as an agent would.
