# Mobile: a phone beside the chat

o7 puts a real device beside each chat for building phone apps: an Android emulator of the chat's
own, an iPhone simulator of its own on a Mac, or a phone on a cable. The person watches and uses it
in the **Device pane**; the chat's agent uses the same device through the `o7_mobile` tools. The
plan, with what is built and what is still to come (iPhones without a Mac, recipes for running a
project), is
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
- **On a Mac** the pane offers an Android phone and an iPhone, one row each. The iPhone row says
  which Xcode, iPhone and iOS it would run, or what is missing (Xcode, its licence, an iOS
  simulator runtime). See "iPhone on a Mac" below.
- **A phone on a cable** appears in the list once USB debugging is on. The person chooses it for a
  chat, and the chat's agent cannot use it until the person turns on "Let the agent use this
  phone" there.
- **Settings > Mobile** has the switch (on by default), what agents get, and what is installed.
  Off stops every device o7 started, closes the pane everywhere and makes every tool answer that
  Mobile is off, in running chats too.
- **Setup** has a page for it, Your phone, after Your assistant: what the phone is for, Android's
  setup with Google's offer and licence exactly as the pane shows them, and on a Mac the iPhone's
  state. It is a page of the agents step, like Your assistant, and Continue never needs it.

## The agent opens it

The person never has to open the pane first (owner, 24 November 2026: "the agent should be able
to open the phone and start working"). Every tool call but `mobile_devices` and `mobile_stop` is
a summon: the manager gives the chat a new number (`MobileStatus.summons`), and a number the
window has not seen opens that chat's pane, sliding in from its side. If Android is not set up,
the agent's `mobile_start` is told the pane opened with the setup, and the pane shows Google's
offer and licence at once; accepting stays the person's.

- The pane takes the side from the browser, never from Changes or a task the person opened: then
  the header's phone button carries a dot and says the agent is using the device, and the pane
  opens when they close what they were reading.
- A pane the person closes while the agent works stays closed for the rest of that stretch of
  work (calls less than a minute apart), and an unanswered summon expires with it
  (`MobileStore`, `apps/desktop/src/lib/mobile.ts`).
- The first call of an app run looks at what is installed (`Manager::look_once`). Before, nothing
  was looked at until the pane was opened, so an agent was told Android was not set up when it
  was.
- What the agent is doing takes the device's name's place in the pane's header, and stays a
  moment (1.2 s) after each call so a run of calls reads as one line. Notices (a problem, a saved
  picture) float over the foot of the stage. Nothing that comes and goes moves the screen (owner,
  24 November 2026: it "jitters up and down"); `check-device.mjs` measures it.
- The instructions, the skill and `mobile_start`'s own description tell agents to start the
  device themselves whenever they build, run, fix or test a phone app, and to check their work
  on it before they report.

## One device per chat

A chat's worktree is its own copy of a project, so two chats on one device would install over
each other. A chat holds at most one device and a device is held by at most one chat. o7 keeps up
to four emulators on disk (`Phone 1` to `Phone 4`), runs at most two at once (each takes 2 to 4 GB
of memory), and gives a chat that asks for "a phone" a running one nobody holds, else a stopped
one, else a new one. A chat lets go of a device it has not watched or used for 10 minutes, and an
emulator nobody holds is stopped. iPhone simulators follow the same rules, counted apart from
emulators (`iPhone 1` to `iPhone 4`, at most two running). Emulators are o7's own processes: they are in a job object
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
| `ios`, `axe` | iPhone simulators on a Mac: Xcode's `simctl` for their lives, apps, links and settings, and AXe for the outline, input and the pane's pictures. See "iPhone on a Mac". |
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
- `<data folder>/devices/ios/axe/2.0.4`: AXe, fetched on an iPhone's first start.
- `<data folder>/devices/ios/simulators.json`: the simulators o7 made, by number. The simulators
  themselves are in Xcode's own folder (`~/Library/Developer/CoreSimulator`), where AXe finds them.

A development build has its own data folder, so it downloads its own copy.

### What the computer needs

The emulator needs hardware virtualization: Windows Hypervisor Platform on Windows (Google retires
its own AEHD driver on 31 December 2026), KVM on Linux, nothing extra on macOS. The pane asks the
emulator itself (`emulator -accel-check`) and says the one thing to turn on. Phones on a cable
need only adb. An iPhone simulator needs a Mac with Xcode 26 or later, its licence accepted, and an
iOS simulator runtime (Xcode's Settings > Components). Building an Android project needs that project's own JDK and build tools; o7 does
not install them.

## iPhone on a Mac

On a Mac with Xcode, a chat can have an iPhone simulator of its own instead of an emulator. The
pane shows its screen as it shows the emulator's. With Xcode 26, AXe can read a simulator's screen
and touch it only while Simulator.app shows it (on GitHub's Macs every read failed with "No
translation object" until it did), so o7 opens Simulator.app hidden and in the background
(`open -g -j`, aimed at the simulator) as the simulator boots. It takes no focus and shows no
window unless the person clicks it in the Dock. Xcode 27 does without: on GitHub's Xcode 27 Macs
AXe read and touched the simulator with nothing open, and o7 opens nothing there.

- **Xcode** is the person's, installed from the App Store. o7 finds it (`DEVELOPER_DIR`, then
  `xcode-select -p`, then Xcode in Applications when only the command line tools are chosen),
  needs Xcode 26 or later, and gives every tool it starts that Xcode as `DEVELOPER_DIR`. It never
  asks macOS to install anything and never accepts Xcode's licence: the pane says to open Xcode
  once, or to get the iOS simulator runtime in Xcode's Settings > Components.
- **Simulators** are o7's own: `o7 iPhone 1` to `o7 iPhone 4`, made with `simctl create` for the
  newest iOS runtime and the newest plain "Pro" iPhone it supports (a runtime Xcode lists for one
  new device only, such as Xcode 27's for a foldable, comes after the general ones), in Xcode's
  usual device set
  (the set AXe reads). Their identities are kept in `<data folder>/devices/ios/simulators.json`, and
  o7 boots, shuts down or deletes only those. A person's own simulators are never listed or
  touched. One made for a runtime that is gone is deleted and made again. A simulator is not o7's
  process, so o7 shuts its own down when nobody holds them, when Mobile is turned off and when o7
  quits; one still booted after o7 crashed is o7's to shut down again at the next start.
- **`simctl`** runs the rest: booting (`boot`, then `bootstatus -b`), installing a `.app`,
  launching and ending an app by its bundle id, opening links, light and dark (`ui appearance`),
  text size (`ui content_size`, Apple's steps nearest the scale asked for), permissions
  (`privacy`), screenshots (`io screenshot`), the app's log (`log show` inside the simulator,
  filtered to its program, a run of the same line said once with its count) and, for a crash, the newest report macOS wrote for it in
  `~/Library/Logs/DiagnosticReports`. A link that opens an app makes iOS ask first ("Open in
  “Hello o7”?"), as it asks a person: `mobile_open_link` says so, and the agent taps Open.
- **AXe** ([cameroncooke/AXe](https://github.com/cameroncooke/AXe)) reads the screen
  (`describe-ui`, turned into the same outline with refs; frames in points become pixels; the
  scroll bars iOS gives every scroll view are left out, since the view itself scrolls), sends
  taps (as real touches: AXe's default tap goes through the accessibility tree at the point,
  which a simulator that has just started may not have ready), swipes, typing, keys and the Home
  and Lock buttons, and streams the pane's pictures (`stream-video --format raw`: one JPEG after
  another, at most 12 a second, at about 1,400 pixels along the longest side). Each picture is a
  screenshot, so they run only while the pane watches: with nobody watching, the stream stops.
  GitHub's virtual Macs, which draw without a GPU, managed 0.1 to 3.1 a second over eleven runs,
  usually 1 to 2; a Mac's own display should do better, and that is not measured yet. Where AXe cannot read Xcode's display catalogue (`displays` failed on
  Xcode 26.6), the screen is measured from the outline's own frame and a screenshot's size. It
  is fetched on an iPhone's first start, version 2.0.4 pinned by size
  and SHA-256 (`third-party/axe.md`), into `<data folder>/devices/ios/axe`. Text an agent types
  goes to AXe on its standard input, never in a command line; text the simulator's keyboard has no
  key for is pasted through the simulator's clipboard, and o7 waits until the screen shows it (a
  few seconds at most) before an Enter follows.
- **The pane** draws each picture as it comes (no video decoder needed, so every web view shows
  it). AXe takes whole gestures, so the person's press becomes a tap, a long press (held 450 ms) or
  a swipe as long as the drag when the finger lifts; the wheel becomes short swipes. Keys and typed
  text are sent in order, a burst of keystrokes as one piece of text. An iPhone has no Back button
  and cannot be turned from o7 yet: the pane shows Home and the app switcher (Home twice), and
  `mobile_press back` swipes in from the left edge, which is Back in most apps.
- **Building** stays the agent's, in its shell: `mobile_start` says the simulator's UDID for
  `xcodebuild ... -destination 'platform=iOS Simulator,id=<UDID>'`, and `mobile_install` takes the
  `.app` the build made. An `.ipa` is refused: it is built for a real iPhone. The simulator shares
  the Mac's network, so a development server is at `localhost`.

## The agents' tools

`mobile_devices`, `mobile_start`, `mobile_install`, `mobile_launch`, `mobile_stop_app`,
`mobile_snapshot`, `mobile_tap`, `mobile_type`, `mobile_swipe`, `mobile_press`,
`mobile_screenshot`, `mobile_logs`, `mobile_open_link`, `mobile_settings`, `mobile_wait_for` and
`mobile_stop`. Their descriptions and the instructions agents read are in
`crates/o7-core/src/mobile_service/helper.rs` and `crates/o7-core/resources/mobile/`.

An agent builds the app with the project's own tools in its shell, then installs and starts it
with the tools, reads the screen as an outline, acts on refs, and takes a screenshot when layout
matters. `mobile_start` waits for the device (up to 200 seconds); `platform: "ios"` asks for an
iPhone simulator, and without it a chat gets an emulator when Android is set up, else an iPhone
when the Mac has one. For an iPhone it says the simulator's UDID to build for.

`mobile_settings` answers once the change is drawn (1.5 s for appearance, text size and turning).
Turning reads the main display's size back from the window manager (`dumpsys window displays`);
a system that did not follow Android's setting is asked again with `wm user-rotation lock`, and
the reply says plainly when the device did not turn.

## Security

Who may do what, and where it is checked:

- **An agent reaches only its own chat's device.** The helper's request carries a token and no
  chat id (`deny_unknown_fields`); the app maps the token to the chat it was made for, compares it
  in constant time, and binds 127.0.0.1 only. The listener dies with the session.
- **Setting Android up is the person's.** No tool fetches anything or accepts Google's licence:
  those exist only as the pane's own commands, and a fetch needs an offer that was shown first.
- **A phone is the person's until they say otherwise.** An agent cannot choose a phone, and
  cannot use the one its chat holds until the person turns that on in the pane, per chat. The
  answer is forgotten when the chat lets go of the phone. o7's own emulators and simulators need
  no question.
- **Only o7's own simulators.** o7 acts only on the simulators whose identities it wrote down when
  it made them; every identity is checked to be a UUID before it goes into a command, and bundle
  ids and links are checked against strict forms. `simctl` and AXe are run as programs with their
  arguments apart, never through a shell; the one predicate o7 writes (`log show`) takes only a
  program name of plain characters.
- **AXe** comes only from its pinned GitHub release, over HTTPS, following redirects only to
  GitHub's own hosts. The archive is checked against its pinned size and SHA-256 before macOS's
  own `tar` unpacks it into an empty folder, so what is unpacked is exactly what was published.
  It is not shipped with o7.
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
  manager's rules (one device per chat, the running limit, a phone's consent, Mobile off), and for
  iPhones: `simctl`'s and AXe's answers, the outline from AXe's tree, the picture stream, the
  person's gestures and typing, and a Mac being needed.
- `cargo test -p o7-core mobile_service`: the grant, its token, the switch, the tool list.
- `node apps/desktop/scripts/check-device.mjs` (in the fixture suite): every state of the pane,
  its input, the phone's question and Settings > Mobile, in two browser engines and both
  appearances, with axe. There is no device there, so no video.
- A real emulator needs a development build: start it with a scratch `O7_DATA_DIR`, and use the
  development-only `mobile_agent_call` command to run a tool as an agent would.
- A real iPhone simulator needs a Mac. The manual workflow **iPhone simulator**
  (`.github/workflows/ios.yml`) runs `cargo test -p o7-devices` on GitHub's Macs and then
  `crates/o7-devices/tests/ios.rs` with `O7_IOS_E2E=1`, once with Xcode 26 (the `macos-26` image)
  and once with Xcode 27 (the `xcode-27` image); its `xcode` input picks one or both. The test
  fetches AXe, makes and boots a simulator through the manager, drives every tool against
  Settings, Safari and a small app it builds (`crates/o7-devices/tests/ios-app`), reads the
  pane's pictures and the person's input through the same calls the pane makes, and keeps a
  transcript, screenshots and pictures as the run's artifact. GitHub's Macs are slow and uneven
  (a first boot took 80 to 200 seconds, a screenshot up to a minute), so the test calls again
  where an agent would: a start that is still going, a wait that ran out. Until a Mac is at hand
  those runners are the only native evidence, and the plan's record says so.
