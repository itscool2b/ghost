# Remote sessions

A person sees and steers the chats on their computer from a phone's browser or from Ghost for
iPhone, and a bot (Grok Bot, or any assistant that connects to MCP servers) can do the same
through tools. The computer opens no port: Ghost keeps one outbound WebSocket to a relay, and
everything else reaches it through that.

It is off until turned on in Settings > Remote access, and it works while Ghost is open,
including in the tray.

## What a person does

1. **Settings > Remote access > Pair a device.** A dialog shows a code to scan and the same link as text.
   The link opens once, within ten minutes. Asking for it turns remote access on. Once a device
   has paired, **Pair another device** in the same dialog makes the next link at once.
2. **Open the link on the other device and tap Pair this browser.** `onghost.dev/remote` takes
   the key out of the address bar and asks first: opening the link pairs nothing by itself, so a
   program that only opens links (a mail or chat scanner) cannot spend it. After the tap it pairs
   and lists the computer's chats by project. From then on that browser opens the
   page without a link.
   - A link opened while the page is already open (a code scanned with the page in front, a link
     pasted into its tab) asks the same question there, over whatever the page shows. "Not now"
     leaves the page as it was.
   - A browser keeps every computer it paired. Pairing another one switches to it; the others
     stay in the computer menu.
   - Pairing a computer the browser already has says so ("Pair this browser again?"). The new
     key replaces the old one, and the browser asks the computer to forget the old device, so
     Settings lists it once. Other tabs of the browser move to the new key.
   - A link that does not work says why, in the computer's words when it refused (a full list of
     devices, a closed link), and offers the way back to the computer the page was with.
   - Ghost for iPhone pairs from the same code or link, scanned or pasted in the app. It asks
     first, as the page does, and nothing is sent before the tap.
3. **Use it.** Open a chat to follow it live, send a message (it queues behind a running turn, as
   in the app), stop a turn, answer a permission request or a question, start a chat in a
   project's folder. "Model and agent" in a chat's header chooses its model and thinking level,
   which apply as they are chosen, and hands the chat to another agent, which opens the chat it
   continues in.
4. **Remove a device** in Settings, or "Forget this computer" on the page. Either ends every
   session of that device at once.

For a bot: **Settings > Remote access** names four assistants (Grok Bot, Muse, Hermes and
OpenClaw) where its list of bots is empty, and **Link a bot** asks which one, what it can do and
its name, then shows one MCP address, once, the way that assistant takes it:

| Assistant | What is copied | Where it goes |
|---|---|---|
| Grok Bot | the address | in the Grok Bot app: Connect apps > Add a custom connector > Remote HTTPS, as the Server URL, then Add plugin |
| Muse | the address | Muse is asked to add a custom connector, and takes it as the server's URL |
| Hermes | four lines of YAML (`mcp_servers: Ghost: url, timeout: 300`) | `~/.hermes/config.yaml`, then `/reload-mcp` |
| OpenClaw | `openclaw mcp add ghost --url ... --transport streamable-http --timeout 300` | a terminal on the computer OpenClaw runs on |
| Another | the address | wherever it takes a remote MCP server (streamable HTTP) |

Setup offers the same four on a page of its own (docs/onboarding.md). Hermes' and OpenClaw's
entries set a five minute tool timeout, so a call may wait for an agent's turn (below) whatever
their defaults are (Hermes documents 300 seconds; OpenClaw's default is not published). The Hermes and OpenClaw steps come from their
published documentation and Grok Bot's from xAI's (read again on 23 November 2026); Muse's come from secondary sources, and none of
the four has been tried with the real assistant.

Once the link is made, its dialog says "Waiting for <name>'s first call" and changes to
"Connected" when that call arrives, so the person knows the assistant reached this computer
before closing it. Each link's row then shows when it was last used and how many calls it has
made and how many of them failed. Those counts stay on this computer (`remote.json`); nothing
about a link's use is sent anywhere.

What a link may do is chosen when it is made and never changed after (a wider link is a new
link). The computer checks it on every call, whatever the relay listed (`bot::Rights`):

- **Read only** (five tools): `inbox`, `chats`, `projects`, `read`, `wait`. It sends,
  answers, stops and changes nothing.
- **Chats** (nine tools): those, and `task`, `send`, `answer`, `stop`.
- **Full control** (those, and five more): `set`, `changes`, `look`, `memory`,
  `new_project`, and in `send` the model, thinking level and access a chat runs with. This is
  what a person does with chats and projects in the app's window.

Two more choices are off unless the person turns them on for the link (23 November 2026):

- **Answer what agents ask.** A permission request (run this command, edit that file) or a
  question. Without it a bot may only decline what an agent asks, and loosen no chat's access
  past `ask`: its person answers, in Ghost or on their phone, and the call-back says so. Any answer
  may approve something (a yes, an "Allow" picked from a list, words that say go ahead), and an
  automated security review of #187 and #188 found every list of such words could be dodged,
  so there is no word list: no answer at all. The bot reads what agents write, and what it reads
  must not be able to approve what an agent does unless its person chose that.
- **Full access in a chat** (full control only): switching a chat to `full_auto`.

A chat a link starts (`send` to a folder, `task`, work queued for tonight, the first set's
`start_chat`) may approve no more than the link may: a new chat takes its project's default
access, which its person may have set to edit without asking or to full access, so before
anything is sent to it the chat is held to `ask` for a link without "Answer what agents ask",
to `auto_edit` for one without "Full access in a chat", and left at the default only for a link
with both (`bot::Most`, `hold_access`). When its access cannot be held, nothing is sent to it.
An automated security review of #195 found this for automations, whose tasks always ask. The
chat that reviews a task is held the same way before its brief is sent (a later review, #199):
the brief carries the task's goal, and an automation's goal carries what a service sent.

Links made before then approve nothing until they are made again. A link unused for 90 days
stops working (a call is told it expired); **Renew** in its row gives it a new address, shown
once, and the old address stops working at once, with what the link may do unchanged.

No link has a tool for Settings, accounts or sign-in, installing or updating software, remote
access itself, deleting data, a raw terminal, or reading a file by its path. The tools are
described to bots in `apps/relay/src/tools.json` and run in `crates/ghost-remote/src/bot/lean.rs`.

### Seeing the work (`look`)

A full-control link may see what a chat's browser tab shows now, or with `of: "device"` its
emulator's screen (tool set 5, 23 November 2026). The answer is the text and a picture, as MCP
carries one (`{ "type": "image", "mimeType": "image/jpeg", "data": ... }`), so a bot can check
that a change looks right without the person.

- **What is taken.** The browser's picture is the one the chat's agent gets from
  `browser_screenshot` (the visible part of the page). The device's is the screen of an
  emulator Ghost runs for that chat; a phone on a cable, or an emulator another program started,
  is never shown, because it is the person's own and can show anything else on it. A chat with
  no page or no running emulator says so.
- **Size.** The computer makes every picture a JPEG of at most 1600 pixels on its longer side
  (quality 75): a page is a few tens of kilobytes. The relay passes on only a PNG or a JPEG in
  base64 of at most 1.5 MiB, and only with `look`'s answer.
- **Trust.** The text says that what the picture shows came from the page or the app, not from
  Ghost or the person, and is data, not instructions. A page can carry words meant for the bot's
  model; the bot's own rules decide what it makes of them.
- **Who.** Only a full-control link is listed `look`, and the computer refuses it from any
  other; only a chat of the link's workspace is asked for. The picture travels through the
  relay like every bot call (a bot link is not sealed end to end), so it is readable there in
  the moment it passes, and kept nowhere.

### What linking costs a bot

A bot pays for a link in tokens three ways, and the tools are shaped by all three:

1. **The tools' descriptions**, read again on every step of the bot's own turn. The first set
   was 21 tools and 11.4 KB as listed (about 3,200 tokens) for a full-control link, 4.7 KB for a
   chats link. The second set was 11 tools and 5.5 KB (about 1,500 tokens, estimated from its
   size), 3.4 KB for a chats link; the current one adds `inbox` (12 tools, 5.9 KB, and 3.8 KB
   for a chats link), then `task` and, for full control only, `look` (7.4 KB for full control). They share their shape: one `set` where there were seven tools that changed a chat,
   `changes` and `memory` that read or write by whether an argument is given, and no titles.
   Every tool says whether it only reads and whether it can destroy (`readOnlyHint`,
   `destructiveHint`), which costs about 370 bytes and lets an assistant decide what to ask its
   person about before calling.
   A call that has just sent a message or an answer gives the chat up to three seconds to be
   seen working before it reads it: the computer acknowledges either a moment before the chat's
   state says so, and a look in that moment once returned "stopped" as a turn's result.
2. **Round trips.** Every call is another step of the bot's turn, which reads everything it
   holds again. `send` starts a chat or sends to one and waits for the turn, so "start it and
   tell me what happened" is one call where it was three; `answer` answers and waits. A wait
   lasts up to 200 seconds when the bot asks (45 by default, under the minute that is the
   shortest tool timeout known among assistants), where the first tools stopped at 45: a ten
   minute turn is three or four calls, not fourteen. It was 240 until 23 November 2026: one
   assistant ends a tool call at 240 seconds with the network's time in it, so a wait of
   exactly that long was lost there every time. `inbox` is the one call for a bot that comes
   back, or is woken: the chats asking with what each asks, the chats working, and the chats
   that finished in the last day with a line each, where it took `chats` and a `read` per
   chat. A full-control link gives the model and access in `send` itself, so a
   new chat's first turn runs with them, which the first tools could not do.
3. **What comes back**, which stays in the bot's context for the rest of its turn. A wait that
   runs out answers in one line (the chat's status, how many steps so far and the last three),
   where the first tools sent the whole turn again each time. A reading is the last 3,000
   characters of the answer and the last five steps unless `full` is asked for (12,000 and 20).
   It carries its proof: the turn's commands with how each ended (the last six, or twelve),
   and the last one still failing with the first lines of its error, read from the tool calls
   by the code a handoff between agents uses (`ghost_protocol::commands`). A command that failed
   and later passed is not "still failing". Values of secret-looking flags and variables are
   hidden. So "did the tests pass" is read, not inferred from the agent's words.
   A changed file is its differing lines with two around them, not the whole file twice. A
   chat's options are a line each.

What was missing is there too: `projects` prints the project ids `memory` takes (the first
`list_projects` did not, so project memory could not be reached), `chats` filters and pages,
and `answer` answers an agent's question in words, a choice word for word or free text where
the question takes it. A form or a sign-in an MCP server asks for is left for the owner.

`memory` reads and writes a project's brief: the short part of its memory that every chat
receives, at most 4 KiB (see "Project memory" in [desktop-customization.md](desktop-customization.md)).
A bot does not see or change the project's notes. Its save is recorded as a bot's, not as the
owner's, so the computer shows a notice of it with Undo, as it does for an agent's.

The relay serves both sets, because the app and the relay are updated apart: a copy of the app
says which set it answers in its bot links (`v`), a relay lists that one, and an older copy,
which says nothing, is listed the first (`apps/relay/src/tools-first.json`, `bot.rs`). A copy
of the app answers both, so it also works with a relay from before.

The level limits the bot's own tools. It is not a wall around the computer: a link that sends
messages directs agents, and agents run commands within the chat's access level.

### A whole task in one call

`task` (set 4, 23 November 2026) is how a bot hands Ghost new work, so Ghost does the managing and
the bot only decides. One call with a goal (and optionally a project, an agent, `review` and
`wait`):

1. **The project** is the one named, else the only one, else the one worked in most recently.
2. **A chat of its own on a new branch**: a Git worktree of the project's first folder, named
   from the goal, made as the window's "new chat in a worktree" makes it but without the saved
   setup (that waits on a terminal nobody watches), so the agent is told the checkout is fresh
   and installs what it needs. A folder that is not a repository gets an ordinary chat.
3. **A brief**: the goal, then how to go about it: plan briefly, make the change, run the
   project's own checks and fix what fails, commit on the branch, push or open a pull request
   only when asked, and finish with a short summary.
4. **A review**, on by default: when the work is done, a second ready agent (another vendor's
   where one is installed) gets a chat in the project folder and is told to read the branch with
   Git (`git -C <worktree> log/diff <base>`), change nothing, and report what is wrong, most
   serious first. No review where there is no branch, or no second agent is ready.
5. **One message**: with a call-back, the bot is told once, when the review is done, with both
   agents' last words (quoted and named as theirs). Without a review it is told when the work is
   done. By default `task` returns at once (`wait` 0), so the bot ends its turn.

The service keeps each task (its chats, branch, worktree, base and reviewer) in `remote.json`,
the last forty, so a review still starts after a restart. A reading from the current tools now
ends with what the chat stands on: its branch (and whether it is a worktree of its own), how
many files are changed and not committed, and its agent's usage as last reported.

### Work for tonight

`task` with `when: "tonight"` (tool set 6, 23 November 2026) queues the task instead of starting
it. Its project and agent are checked when it is queued, so a wrong name is said while the bot
is listening; the answer names the queued task (`q` and a few letters). Between 1 and 6 in the
morning on the computer's own clock, while remote access is on, Ghost starts the oldest queued task
of the open workspace exactly as `task` starts one now (its own branch, the brief, a review),
one at a time (the next waits while the last one or its review works) and at most three a night.
The point is the person's subscriptions: work that can wait runs on capacity nobody is using.

- `inbox` lists the link's queue, and what could not start in the last day with why (an agent
  no longer ready, a project gone); a link with a call-back is also told so at once. A queued
  task keeps its whole goal (at most 64 KiB), which its brief starts with.
- `task` with `cancel` and the id takes a queued task off. A link may leave ten at a time, and
  its queue goes with it when it is removed.
- The queue and the night's starts are kept in `remote.json` (`queued`, `night`), so a restart
  loses nothing. The computer must be awake: "Keep this computer awake" in Settings helps.

### Tasks from the phone

Ghost for iPhone hands over a task as a bot does (`task`, feature `tasks`, 25 November 2026): now
or for tonight, on a branch of its own, with a second agent's review. What differs:

- **Owner.** The task belongs to the device (`by: "device"` in `remote.json`; a bot's records
  have no `by`, so they read as before). It is kept, found and reviewed while that device is
  paired in the open workspace, and goes with it. An owner a newer build wrote reads as a bot's,
  whose id names no link, so it is left alone.
- **Brief.** It says the person handed the task over from Ghost on their phone, where a bot's says
  the person's assistant did.
- **Access.** A bot's task may approve no more than its link may. The person's may do anything
  the project allows, and the access they choose (`ask`, `auto_edit` or `full_auto`) is set on
  the chat before the brief is sent; when it cannot be set, nothing is sent. No choice leaves the
  project's own. The review's chat is held to the same access.
- **Photos.** A task that starts now may carry the person's staged photos with its brief,
  measured before its chat, worktree and branch are made.
- **Queue.** A device may leave ten tasks for tonight, as a link may. They start at night as a
  bot's do, set to the access chosen when they were queued. The person may take any queued task
  off, a bot's too (`task_cancel`); a link takes off only its own.
- **Telling.** No bot follows the person's task: its phone is told instead, if it chose to hear
  of its tasks ("Notifications on the phone"). It hears `reviewed` when the review is done, about
  the task's own chat with the reviewer's beside it, `queued_failed` when a task for tonight
  could not start, with why, and `pull_request` when its pull request has news (it is watched for
  a week while the phone hears of its tasks). `tasks` lists what happened too.

### A task's pull request

When a task's agent opens a pull request (because the bot asked it to), Ghost watches it for the
link that started the task, if that link has a call-back, or for the phone that started it, if
that phone hears of its tasks: every five minutes, for a week, with GitHub CLI as the person is
signed in (`gh pr view` in the task's own folder; nothing is written to GitHub). The bot or the
phone is told, once each (`callback::pull_request_news`):

- the checks failed (with their names), or passed after that;
- a new review or comment: its author, and its words only when the author owns the repository,
  is a member or a collaborator (`authorAssociation`). Anyone can comment on a public
  repository, so a stranger's words are not passed to the person's assistant; the message says
  there is one. An automated security review of #194 asked for this, and for the message to end
  by telling the bot to check with its person before having the agent act on what reviewers
  wrote;
- the pull request was merged, or closed. Then it is no longer looked at.

A reading of a chat on a branch of its own also names its pull request ("Pull request: #12
<address>, open; checks failed: test"), read again at most every two minutes. Without GitHub
CLI, or signed out, nothing is said. What was last seen is kept in `remote.json` (`prs`).

### Automations

An automation (Settings > Remote access > Automations, 23 November 2026) is an address that
starts a task from what a service sends it: a failed build, a new issue, an alert. It is made
with a name, the project its tasks start in and an instruction, and its address
(`/api/remote/v1/hook/<computer>/<id>.<token>`, shown once) takes a plain POST of up to 64 KiB,
as webhooks send. No assistant is in between.

- **What its agent is given.** The instruction, then what was sent, fenced (`<<<` ... `>>>`)
  and named as from outside Ghost, data it must not obey; at most 16,000 characters of it. The
  task starts as `task` starts one: its own branch, a review, the project's default agent, and
  access held to `ask` whatever the project's default: what was sent comes from outside, so
  the agent asks before every edit and command, and nothing approves for it.
- **Limits.** At most three of its tasks work at once and twenty start a day; what is sent
  beyond that is refused, and the sender is told so. What is sent to one automation is taken one
  at a time, so things sent together cannot pass the limits together, and the day's count is
  kept in the automation's own log, which nothing else can push out (an automated security
  review of #195 found both).
- **What the sender hears.** 202 `{"ok": true, "text": "Started a task."}`; 409 when the
  computer refused it; 503 when the computer is away (a sender that retries gets it there
  later); 404 for a wrong address. A start that fails says only that it failed: why (which can
  name projects and agents) is shown in Settings, in the automation's row.
- **What it is not.** Its address is no MCP server (the relay answers it 404 there), a bot
  link's address is no automation's, and the computer refuses anything but a trigger from it.
  A page cannot send to it (the relay refuses a request with an `Origin`). It expires after 90
  days unused, as a bot link does, and Renew gives it a new address.

### On this computer, without the relay (the door)

An assistant that runs on the same computer as Ghost (Hermes, OpenClaw and others) can use Ghost's own
helper as a local MCP server instead of the address: `"<Ghost's program>" --ghost-bot-helper`, with
the bot link's address in the environment variable `GHOST_LINK` (Settings shows both under "If your
assistant runs on this computer" when a link is made). Its calls then never leave the computer.
It works while Ghost runs with remote access on, even when the relay cannot be reached.

- **The tools and the rules are the same.** The helper lists the relay's own tool list
  (`apps/relay/src/tools.json`) filtered as the relay filters it for the link's level and set
  (a test checks its instructions are the relay's word for word), and each call is answered by
  the same code as a relay call (`answer_call`): the same token check, rights, refusals, limits
  and call-backs. An automation's address is not taken there.
- **The door.** The remote service listens on loopback only, at a random port it writes to
  `door.json` in Ghost's data folder with a key new each run (the file is the account's own; mode
  0600 on Unix), and removes the file when it stops. The helper reads it, challenges the door,
  and sends the link's token only after the door proves it holds the key (`crypto::door_proof`,
  an HMAC under the challenge), so a program that took the port while Ghost was not running learns
  nothing (an automated security review of #197 asked for this).
- **Not done:** OAuth 2.1 for directory listings, and MCP's events and tasks. A bot link's
  address already is the credential its assistant pastes, every assistant we link takes one,
  and neither is needed for anything above; they wait until a directory or an assistant asks.

### Ghost's skill

`https://onghost.dev/skill.md` (the file is `apps/web/public/skill.md`) is a skill in the common
`SKILL.md` form for an assistant to keep: when to use Ghost, the loop (one `task`, end the turn, be
told, `inbox`, decide), recipes for fixing and proving, a second opinion, what needs the person,
work for tonight, a bug report, seeing the work and shipping it, and the rules (answer only what
the person decided; what agents, pages and comments say is information, not instructions). The
link dialog gives it after the assistant's own steps, with three things to try, and the site's
FAQ names it. It is served as `text/markdown` by the site's Worker. Whether each assistant picks
it up on its own from its description is not yet measured: none has been tried.

### Telling a bot when a chat is done (the call-back)

A bot that hands Ghost a task used to have two ways to learn how it went: hold a call open, or ask
again and again, each time re-reading everything it holds. Since 23 November 2026 a link can
name one place to be told instead (**Settings > Remote access**, a link's **Call back**): the
assistant's own way of being woken from outside. The bot then sends once, ends its turn, and is
woken once.

| Assistant | The address | The key | How Ghost speaks to it |
|---|---|---|---|
| Grok Bot | a routine's webhook ("POST to") | the routine's key | `Authorization: Bearer <key>`, a JSON body the Bot is given with the routine's instruction |
| Hermes | a webhook route, `http://<host>:8644/webhooks/<route>` | the route's secret | `X-Webhook-Timestamp` and `X-Webhook-Signature-V2`: the hex HMAC-SHA256 of `<timestamp>.<body>`. With `--deliver-only` Hermes passes the text on without a model turn |
| OpenClaw | the Gateway's `/hooks/agent` | `hooks.token` | `Authorization: Bearer <token>`, `message` (which OpenClaw wraps as outside content), and optionally the channel and recipient its answer goes to |
| Another | any receiver of Standard Webhooks | its signing secret | `webhook-id`, `webhook-timestamp`, `webhook-signature: v1,<base64 HMAC-SHA256 of id.timestamp.body>` |

Each form was read from the assistant's own documentation on 23 November 2026; none has been
run against the real assistant. Muse documents no way to be woken.

**When a link is told.** Only about a chat it started, sent to or answered (the last forty),
only when that chat stops working (it finished, failed, or asks for an answer or a permission),
and two seconds after it did. In those two seconds a bot that was waiting on the chat gets the
result in its own call's answer, and is then not told again; a chat that moved on meanwhile is
told of where it ends up instead. Nothing is sent for how chats stood when Ghost started. A
failed message is tried once more after three seconds (never after a refusal), and Settings
shows how the last one went. If a bot's own call returned the result but the bot had already
given up on that call, it is not told: it asks again, as before.

**What it is told.** One short paragraph Ghost writes: which chat, what happened, and the one call
that reads the rest. Everything in it that an agent or a person wrote (the chat's title, what a
request asks, a failing command, the agent's last words) is on one line, cut short, inside a
quotation and named as theirs. A request's options and the commands that ran are not in it;
the bot reads them with `read`. The assistant that receives it may hold its person's mail and
messages, so a chat's output must never be able to speak to it as Ghost.

**Who may do what.** Only the window sets, tests or clears a link's address: a bot has no tool
that reads or changes its own link. The address must be `https`, except on this computer or a
private network, where a Hermes or OpenClaw gateway speaks `http`; it may not carry a sign-in
or a fragment, and Ghost follows no redirect from it, so the key goes where its owner pointed and
nowhere else. The key lives in the computer's secure storage (one entry per link, named by a
digest), is read only at the moment of sending, and is never shown again, written to
`remote.json`, sent to the relay or logged; nor is the address, whose path is itself a secret
for some assistants. Settings shows an address's scheme and host only. Removing the link, or
its call-back, removes the key; so does uninstalling. The relay has no part in any of this:
the computer sends the message itself.

**What the address learns.** That a chat exists, its title, its project and agent by name,
and a line of the agent's last words. That is the tradeoff of being told: it is the same
service the bot's calls already pass through, but it is one more place the words go.

### Notifications on the phone

Ghost for iPhone is told what needs the person while the app is closed (25 November 2026), through
the relay and Apple's push service. What a notification says is sealed on this computer for one
phone: neither the relay nor Apple can read it. The parts are `crates/ghost-remote/src/push.rs`
(sealing, words, limits) and `host/notify.rs` (who hears what, and the frames).

- **Turning them on.** The app asks Apple for its address (a token of 64 to 200 hex characters)
  and gives it with `push_register`, with which of Apple's services it belongs to (`sandbox` for a
  development build, `production` for the App Store's) and what the person wants told (`prefs`).
  The computer keeps them with the device in `remote.json` (`push`, in lower case, with when),
  and the same address leaves any other device record (an earlier pairing of the same phone).
  `push_unregister` forgets it, and removing the device forgets everything kept with it. A
  device without notifications reads in `remote.json` as it did before them.
- **Sealed for one phone.** The push key is HKDF-SHA256 of the device key, with no salt and the
  label `o7 push v1`; the phone's notification extension holds only that key, never the device
  key. Each notification is sealed with AES-256-GCM under a fresh 12-byte nonce, with
  `o7 push v1|<computer id>|<device id>` as associated data, and travels as the unpadded base64url
  of the nonce, the ciphertext and the tag. `crates/ghost-remote/tests/push-vectors.json` holds
  vectors the phone's tests read too; `GHOST_WRITE_VECTORS=1 cargo test -p ghost-remote push::vectors`
  writes them anew.
- **What it says** (`push::Payload`, at most 2,400 bytes once encoded): `{ v: 1, id, at, host,
  kind, chat, title, project, agent, line, asking_count, request: { id, type, allow, deny },
  review, queued }`, each part left out when it is not known. `kind` is `asking`, `finished`,
  `failed`, `reviewed`, `pull_request`, `queued_failed`, `resolved` or `test`. `id` is new each
  time and `at` is when it went (Unix milliseconds), so the phone drops a repeat or one older than
  the last it showed. `asking_count` is how many chats on the computer wait on the person now, for
  the badge. `request.type` is `permission`, or the question's kind (`confirm`, `select`, `input`,
  `form` or `questions`); `allow` is the option of a permission that allows once, never one that
  allows always, and `deny` the one that rejects once. A longer payload loses the end of its line
  (all of it, if need be), then of its title, then its other parts one at a time; never its
  version, id, time, computer, kind or chat.
- **One per chat.** A chat's notifications on a phone share a collapse id: the first 12 bytes of
  HMAC-SHA256, under the device key, of `o7 push collapse v1|<chat id>`, in unpadded base64url. A
  newer one replaces the older, and Apple never learns which chat it is. A notification about no
  chat (a test, work queued for tonight) has none.
- **The frames.** A relay that sends notifications says `f{"push":1}` as soon as it accepts the
  computer's socket. Only on such a socket does the computer send `p{ id, to, env, type: "alert",
  collapse, d, b, ttl, sound }` or `p{ id, to, env, type: "liveactivity", d, b, ttl, priority,
  event }`: `id` its own for the send, `to` the address, `d` the device the blob is sealed for,
  `b` the blob, `ttl` how long Apple keeps trying (an hour, 15 minutes for one that asks). An older
  relay ends the computer's socket on a frame it does not know, every browser with it, so frames
  wait, at most 32 and at most two minutes, for a socket whose relay said `f`, and are then
  dropped. The relay answers each with `P{ id, ok, status, reason, drop }`, on whichever socket is
  open by then, and the computer matches it by `id` (an id it did not send is ignored). `ok` sets
  the device's last notification, which Settings shows. `drop` (Apple answered 410,
  `BadDeviceToken` or `DeviceTokenNotForTopic`) forgets the address, while the device still has
  that one. A refusal (the relay's `invalid`, `off`, `limited` or `unreachable`, or Apple's reason)
  is not tried again.
- **When.** From the chats the call-back's watch already follows (`host.rs`, `tell`):
  - `asking`: a chat starts waiting on the person, two seconds after, if it still does, with its
    request when the phone can answer it and what it asks as the line.
  - `resolved`: it works again and that phone did not answer it. It replaces the notification that
    asked, quietly and under the same collapse id, with the line "Answered on the computer."
    ("Answered on another device." when another phone answered). The phone that answered takes
    its own away. A phone told that a chat asks, which will not hear how the turn ended (it chose
    not to), is told "No longer waiting on you." when the turn ends.
  - `finished` and `failed`: a turn ended, with what it came to in a line (how it ended when it
    did not simply finish, the agent's last words, a command still failing), or why its agent
    stopped.
  - `reviewed`: the review of a task from the phone is done. Its phone hears of it about the
    task's own chat (`chat`), with the reviewer's chat beside it (`review`) and the reviewer's
    last words; any other phone hears that the review's chat finished.
  - `queued_failed`: a task the phone queued for tonight could not start. Its phone hears why,
    with the queued id.
  - `pull_request`: news on a phone's task's pull request, as a bot is told it, to its phone. The
    words of reviewers and commenters go only when they are the repository's own people.
  - `test`: `push_test`, or Settings' **Send test notification**: "Notifications from <computer>
    arrive here."
- **What each phone hears.** `prefs.asking` (asking and resolved), `finished`, `failed` and
  `tasks` (reviewed, queued_failed and pull_request), each on unless the phone says otherwise, and
  `scope`: `all` chats, or `mine`, those the app started, sent to or answered (`following` in
  `remote.json`, the last 40). The computer always sends; the phone hides what is about the chat
  on its screen.
- **Limits.** Per device: one per chat and kind every 15 seconds (a resolved one and a test
  aside), 30 every ten minutes and 300 a day, and a test at most every ten seconds (`busy`). The
  relay has its own: 60 a minute and 600 an hour per computer, 30 a minute per phone, 32 phones
  an hour.
- **The Live Activity.** The app gives the token of the computer's Live Activity with `activity`.
  While something works or waits, the computer sends its state, sealed the same way, as the
  content-state `{ "d": <device id>, "b": <blob> }`; sealed in it is `{ asking, working, lead,
  since }`: how many chats ask and work, the one that leads in plain words (the first that asks,
  by its title and what it asks; else the first that works, by its agent and what it does), and
  when that work began. At most one update every ten seconds per device, at once and at Apple's
  high priority (10) when more chats ask than it shows, else priority 5, and `end` once nothing
  worked or waited for five minutes.
- **What the relay and Apple learn.** The phone's address at Apple, the device id a notification
  is sealed for, its collapse id, its size and when it went; never what it says. The relay wraps
  every alert in the same fixed words, "Ghost" and "Something changed on your computer.", and the
  phone writes the sealed words over them once it opens them. The relay keeps, for its limits,
  when the computer's notifications went out in the last hour, by a hash of each phone's address.
- **Settings.** A phone's row says "Ghost for iPhone 1.0", whether its notifications are on and when
  the last went, with **Send test notification**. Connected to a relay that does not send them,
  the page says "The relay does not send notifications yet." `RemoteStatus.notifications` says
  whether the relay does, and each device's `notifications` and `last_push` how it stands.

## The parts

| Part | Where | What it does |
|---|---|---|
| Relay | `apps/relay` (Worker `ghost-relay`, route `onghost.dev/api/remote/*`) | One Durable Object per computer holds its host socket and the sockets of browsers and Ghost for iPhone, and forwards between them. Serves MCP to bots. |
| Service | `crates/ghost-remote` | Pairing links, devices and bot links (`registry.rs`), the sealed channel (`crypto.rs`), one session per browser or app (`host.rs`, `host/session.rs`, what only the app asks for in `host/app.rs`, and its staged photos in `host/uploads.rs`), notifications for the app (sealed in `push.rs`, sent from `host/notify.rs`), bot tools (`bot.rs`, `render.rs`), the socket (`relay.rs`). |
| App | `apps/desktop/src-tauri/src/remote.rs` | Starts the service, gives it the open profile's chats through the same lease a window's commands take, and exposes the commands Settings uses. |
| Settings page | `apps/desktop/src/components/settings/RemotePanel.tsx` | The switch, devices, bots, and the two dialogs that show a secret once. |
| Remote page | `apps/web/app/pages/remote.tsx`, `apps/web/app/remote/` | The browser's session (`client.ts`), its half of the channel (`channel.ts`), where device keys are kept (`storage.ts`). |
| Ghost for iPhone | `apps/ios` | The native app: the same sealed channel and requests, on a path of its own at the relay (below). |

The remote page folds a chat with the desktop app's own reducer
(`apps/desktop/src/lib/chat/transcript.ts`), from the same events the app's window gets, so a turn
reads the same in both.

The relay is TypeScript, where the rest of the Workers are Rust: Durable Object WebSocket
hibernation is native there, and the relay holds no logic worth sharing, only forwarding. Every
decision about who may do what is made in Rust on the computer.

## Who is trusted with what

- **The computer decides everything.** It checks each browser's key and each bot's token itself.
  Nothing the relay says is believed without that.
- **A paired browser's session is sealed end to end.** The relay carries messages it cannot read,
  change, reorder or replay. A relay that turned hostile could drop messages or refuse service,
  and nothing more.
- **A bot's calls are not sealed.** A bot speaks MCP over HTTPS to the relay, so the relay reads
  each call and answer in passing, as does the bot's own service, and each call carries the
  link's token. So a relay that turned hostile, or the bot's service, could act as any bot link
  it has seen used, at that link's level. The Settings page says so. The computer checks the
  token, the link's workspace and its level on every call, so nobody can invent a link, widen
  one, or use one in another workspace.
- **A bot link's level is the computer's to enforce.** The relay lists the full-control tools
  only to a link made with full control and treats a wider tool as unknown for the others, but
  the computer checks the level itself on every call, whatever the relay listed or let through
  (`host.rs`, and again in `bot::call`).
- **A device or a bot link reaches one workspace**: the profile that was open when it was made.
  `remote.json` is the installation's, but each record names its
  profile, and the computer accepts a key or a token only while that profile is the open one.
  When another profile opens, browsers are told to start over, the other profile's sessions end,
  an open pairing link closes, and the relay is given only the open profile's bot links, so the
  others' addresses answer 404. A refused browser is told the computer has another workspace
  open (`elsewhere`) and keeps its key for when its own is open again. Settings lists the open
  workspace's devices and bots only. With no profile open, nothing is accepted.
  The same binding holds at the point of use: a session keeps the profile its key was accepted
  under, a bot call the profile its token was, and every call into the app's chats is made
  through `Backend::bound(profile)`. The app hands out a profile's chats only while that profile
  is the open one, reading both under the lock a switch takes (`AppState::sessions_of`). So in
  the moment between the app opening another profile and the service hearing of it, a session of
  the old profile is refused rather than served the new one's chats.
- **Anyone holding a device key or a bot address controls the chats**, and agents can edit files
  and run commands. There are no narrower permissions yet: pair your own devices, and link only
  a bot you would trust at the keyboard.

### Pairing and keys

A pairing link is `https://onghost.dev/remote#<computer id>.<pairing id>.<key>`: 16, 8 and 32
random bytes, base64url. The key is in the fragment, which browsers never send to a server; the
page reads it once and removes it from the address bar and history before connecting. The pairing
lives in the computer's memory only, opens once, expires after ten minutes, is replaced by the
next link, and closes after eight sessions that fail to prove its key.

Pairing creates a device: an id and a fresh 32-byte key, sent to the browser inside the pairing
session. The browser imports it as a WebCrypto key that cannot be exported and stores that key
object in IndexedDB, so a script on the origin can use it and none can read it out. The computer
keeps device keys, and only the SHA-256 of each bot token, in `remote.json` in the data folder,
beside the chats themselves and readable by the same user.

Ghost for iPhone keeps its device key in the iPhone's Keychain. Its `pair` names it
(`client: { kind: "ios", version, model }`), and the computer records the device as `ios` with
that version (at most 32 characters) only when the pairing came in on the app's path, which the
relay marks (below). Anything can say it is the app; only the path proves no page sent it. A
browser that says so is recorded as a browser. An `ios` device with no name is called "iPhone".
Settings shows "Ghost for iPhone 1.0" on its row.

### The sealed channel

Both sides hold one 32-byte secret: the pairing key, or the device key.

1. The browser sends `H{ v, kind, id, nonce }` with a fresh 16-byte nonce.
2. The computer finds the key for that id, answers `W{ v, nonce }` with its own nonce, and derives
   two AES-256-GCM keys with HKDF-SHA256 (salt: both nonces; info: one label per direction).
3. Each side numbers its messages from zero. The number is the nonce, and the associated data
   names the credential (`pair|<id>` or `device|<id>`), so a message opens only as the next one,
   from the other side, under that credential.
4. The computer's first sealed message is `ready`. Opening it proves the computer holds the key.
   The browser's first sealed message proves the same the other way, and only then does the
   computer tell the relay the browser is accepted.

The browser accepts exactly one welcome per hello and one `ready` per session, so a replayed
welcome can never restart a key's numbering. Anything that fails to open, arrives out of order,
or is not one of the operations ends the session. An operation the computer knows but did not
offer this session is answered `not_offered`, and the session goes on (see "Operations a device
can ask for"). `crates/ghost-remote/tests/vectors.json` holds vectors both implementations are
tested against.

A refusal (`R`) is not sealed, since there may be no key to seal it with. So it deletes nothing:
the page says the computer no longer accepts this browser and lets the person try again or forget
it.

### What the relay stores and limits

Per computer: a mark that it has an owner, when it was last connected, the hashes of its bot
tokens, what each link may do (read only, full control) and which tool set it was made for, and,
for its limits on notifications, when its notifications went out in the last hour, by a hash of
each phone's address at Apple. No D1, R2 or KV. A room no computer has connected to for thirty
days forgets all of it; the computer sends its bot links again whenever it connects. The relay
holds one secret, Apple's push key (`APNS_KEY`, set by the owner), and one more Durable Object
(`Apns`) keeps the token it signs for Apple with. Without the key it sends no notifications and
never says it does.

- A computer's id is the first sixteen bytes of a SHA-256 over a fixed label and its key
  (`crypto::host_id`), so only the key can claim the id. The Worker checks that before a request
  reaches a room: nobody can take an id first, or take one over, and a wrong claim stores
  nothing. A host socket may not come from a browser.
- A browser socket must come from the site's origin. Until the computer accepts it, it is one of
  at most eight unproven sockets (three per address, where an IPv6 address counts as its /64),
  a newcomer replaces the oldest, and it may send four messages of at most 2 KiB: the handshake
  and nothing more. Accepted browsers are never replaced; after the computer reconnects they
  have thirty seconds to be accepted again.
- Ghost for iPhone connects at `/api/remote/v1/app/<computer id>` and sends no `Origin`. A page
  always sends one (its origin, or `null`), so an upgrade there with any `Origin` is refused
  (403), and a request that is not an upgrade gets 400. The Workers runtime drops a header with
  no value, so an empty `Origin` arrives as none; no page can send one. After it tells the
  computer `o<cid>` for such a socket, the relay sends `k<cid>`, and again after each `o` it
  replays when the computer reconnects. That mark is how the computer knows the socket is the
  app's and not a page's. A Ghost on the computer from before the mark ignores `k`, so the iPhone
  app is offered there what a browser is. Every limit here (the
  unproven places, the four small messages, 96 KiB, 240 in ten seconds, keep-alives, the thirty
  seconds to be accepted again) applies to the app's socket as to a browser's, and both share
  the same places.
- That does not stop someone who knows a computer's id from opening sockets fast enough to keep
  a new browser from finishing its handshake. It costs the computer almost nothing and reaches
  no chat, but it can delay pairing or a reconnect. The id is in every pairing link and bot
  address, so treat those as private. See "Not done".
- A browser sends at most 96 KiB per message and 240 messages per ten seconds.
- A wrong or removed bot address is a plain 404. At most eight bot calls wait at once, each for
  at most 250 seconds: a call may wait up to 200 for an agent's turn, so that a long turn costs
  a bot few calls. Only a link's own bot can hold those eight places, and only on its computer.
  A bot that gives up on a waiting call and says so (`notifications/cancelled` with the call's
  own id) frees its place at once; the chat carries on, and nothing else can be cancelled that
  way.

The computer also ends any connection that has not proved a key within twenty seconds, holds at
most 32 connections, 16 running calls per browser and 8 running bot calls, and stops sending live
events to a browser that falls 4 MiB behind (the page then opens the chat again). A connection
that arrives when all 32 places are taken is not refused: it takes the place of one that never
proved a key, or, when every one did, of the one that has said nothing for longest, whose page
connects again if it is still there. Anything can open a connection, so one that proved nothing
never puts a paired browser out while another unproven one is there to go. For Ghost for
iPhone's photos it holds, in memory only: at most 8 MiB a photo, 4 arriving at once and 16
staged per session, 24 MiB per session and 64 MiB across every session on the computer. Each
photo's whole size counts from its first request until nothing holds its bytes, a message
still being sent with it included, so the 64 MiB bound what photos take in memory. A photo
nothing touched for ten minutes is dropped (the five-second sweep), and a session's photos go
when it ends. Its pieces are taken at once, in order, and hold none of the 16 places for
calls.

## Staying connected and keeping up

What made the page feel broken before 19 November 2026, and what each part does about it:

- **A socket that died without closing.** A phone that slept, or changed networks, keeps a socket
  that says it is open while nothing crosses it; taps then did nothing until a reload. When the
  page comes back to the front, regains focus or the network returns, it sends the relay `ping`
  and starts a new session at once if no `pong` comes within three seconds
  (`RemoteClient.wake`). A request that gets no reply in fifteen seconds asks the same question
  instead of only failing. The first reconnect is immediate; later ones back off.
- **A computer that is gone but looks connected.** The desktop says `ping` every 25 seconds. A
  host socket silent for 75 seconds belongs to a computer that slept or lost its network: the
  relay ends it, tells the browsers `h0` ("Computer is away") and tells a browser that opens
  then the same, instead of forwarding into a socket nobody reads. Three missed pings, so one
  late ping never drops a working computer.
- **Opening a chat while the link is still coming up.** The page keeps the chat that was asked
  for and opens it when the session is ready, instead of refusing the tap.
- **The wrong chat streaming.** Opens and closes used to run side by side on the computer, so a
  slow open could finish after the close that followed it and leave a chat followed that the
  page no longer showed. Each open or close now takes a number; they run in the order asked,
  and one that is no longer the newest when its turn comes answers `superseded` and changes
  nothing (`host/session.rs`).
- **Lag while an agent writes.** The computer merges live frames that follow each other for the
  same browser and chat into one sealed message (up to 256 KiB, never across anything else in
  the queue, so order holds) and writes a burst to the socket with one flush. The page folds
  frames as they arrive but draws at most once per animation frame, keeps finished turns from
  drawing again, and folds repeated tool updates in history as the desktop does.
- **A send that seemed to do nothing.** The message shows at once as "Sending" (or "Queued until
  this turn ends"), the box empties, and a refused send puts the text back with the reason.

### A chat that would not open

The owner's report of 27 November 2026: a chat that is working sometimes cannot be opened from
the page, however often the page is loaded again. Three causes, found by reading the code and
then played against the real app (`apps/relay/tests/native.mjs`):

- **A chat full of sessions that are gone.** A chat takes four followers, the window's own view
  among them. A phone that goes to sleep with a chat open keeps following it: its socket stays
  up at the relay, and nothing tells the computer the page is gone. A few of those, which is
  what watching a long turn from a phone leaves behind, and every new open was answered `busy`.
  The computer now makes room: it lets go of the remote follower of longest standing, tells it
  `lost`, and lets the new one in (`Backend::followers`, `host/session.rs`). It never closes the
  window's view and never ends a bot's wait; when only those fill a chat, the answer is still
  `busy`. A page that is in fact still there asks for the chat again the moment it hears
  `lost`, and for five seconds it is not let in at another's cost, so one page more than a chat
  takes cannot have them trade places for ever. It shows its list, with the reason.
- **An answer a little too large.** The answer to `open` carries the chat as it stands and its
  newest history. Only the history was cut to fit one sealed message, so a page of history a
  little under the limit, with anything of size beside it (a long queued message, a long error,
  a task's output), made an answer a little over it: `too_large`, every time, until the chat
  changed. A long turn's newest page is about that size. The whole answer is fitted now: what
  stands beside the history first, then the history in the room that is left.
- **One step larger than a message.** A step of more than 600 KiB (a very long message, a very
  large tool result) could not be sent in any page, so its chat could not be opened, and
  "Earlier messages" failed at it. Such a step is sent with its long texts cut and marked
  ("cut: too long to show on this device"); the computer keeps it whole.
- **A tap that did nothing.** When an open was refused, the page went back to its list without a
  word: the list had held the reason, and the list is off screen while a chat opens. The
  reason is kept above the list now, and a chat that closes by itself says why.

The page wears the computer's theme: `ready` carries `look` (the theme's id and Light, Dark or
System from Settings > Appearance), the page checks the id against the themes it ships and keeps
it with the paired device, so the next visit is themed before the computer answers. A copy of
the app that sends no `look` gets the default theme. The theme's name is all it says about the
computer, and it reaches only a browser that already proved a key.

### Staying reachable while nobody is at the computer

A phone or a bot reaches this computer only while Ghost runs there with remote access on and the
computer is awake. Two switches in Settings > Remote access help, both off until the person turns
them on (23 November 2026):

- **Open at sign-in** starts Ghost when the person signs in, in the background: the window stays
  hidden, and no chat's agent starts, until the tray icon, the Dock or another launch opens it.
  The system's own record is the only one, so the switch shows what is there: the value under
  Windows' `Run` key (and its switch in Startup apps), a launch agent in
  `~/Library/LaunchAgents` on macOS, an autostart entry in `~/.config/autostart` on Linux (the
  AppImage's own path for an AppImage). An entry that opens another copy of Ghost is not this
  one's. A development build keeps an entry of its own, and a copy started with `GHOST_DATA_DIR`
  cannot have one, because the start would open the usual data folder (`login_start.rs`).
- **Keep this computer awake** holds off idle sleep while remote access is on and a paired
  device, or a bot link that has not expired, can reach the open workspace. The screen still
  turns off, and closing a laptop or choosing Sleep still sleeps it. It is the hold Ghost already
  takes while an agent works (`native_shell/power.rs`); the choice is kept in `remote.json`
  (`keep_awake`).

When the computer is away anyway, a bot's call says so plainly and that nothing was done, with
when the computer was last connected ("It was last connected 3 hours ago"); the relay keeps that
time (`seen`) and forgets it with the rest of the room. A call that was with the computer when it
went says instead that it is not known whether it was done, and to read the chat once the
computer is back.

## Operations a device can ask for

A device is a paired browser or Ghost for iPhone. A pairing link's session asks for `pair` alone;
every device may ask for `overview`, `open`, `close`, `history`, `send`, `interrupt`,
`permission`, `answer`, `create` and `forget` (`crates/ghost-remote/src/wire.rs`). Each maps to the session manager call the app's own window
makes; there is no file, terminal, settings or account access. A new chat starts in the project
folder itself with the project's usual agent.

The rest belong to a feature, and a session may ask only for what it was offered:

| Feature | Requests | Offered to |
|---|---|---|
| `pickers` | `options`, `set_model`, `set_thinking`, `switch_agent` | a browser and the app |
| `inbox` | `inbox` | the app |
| `search` | `search` | the app |
| `control` | `archived`, `set_access`, `rename`, `archive`, `start_agent`, and the access levels in `options` | the app |
| `review` | `changes`, `diff`, `facts` | the app |
| `memory` | `memory`, `set_memory` | the app |
| `screens` | `picture` | the app |
| `tasks` | `task`, `tasks`, `task_cancel` | the app |
| `attachments` | `upload_begin`, `upload_chunk`, `upload_abort`, `image`, and photos in `send`, `create` and `task` | the app |
| `push` | `push_register`, `push_unregister`, `push_test` | the app |
| `activity` | `activity` | the app |

A browser reaches four of a full-control link's actions, the pickers. What they name (a model,
a level, an agent) is a short value from a list the computer sent, checked for length and
control characters before the app sees it, and the app refuses one its chat does not offer. The
iPhone app reaches a few more, each checked the same way first: a chat's access level, title and
archiving, starting its agent, its changes, a picture of its browser or device, and the
project's brief as the person's own edit. No device has a request for a project from an
arbitrary folder, for a bot's memory edit, or for a file by its path.

The computer names what it offers when a session opens (`features` in its `ready` message), and
a device shows a control only to a computer that names it: the page is always the newest, and an
installed Ghost can be older. A browser is offered `pickers`, as before the app, so what a browser
is sent has not changed. The app is offered every feature above, but only on a socket the relay
marked as the app's (`k`), and only with a pairing link or a device recorded as `ios`: a
browser's word that it is the app is not enough, and neither is the app's path with a browser's
key. On the app's path `ready` also carries `site` (`https://onghost.dev`), so the app follows the
site if it moves, and `notifications`, whether the relay in front of the computer sends them now;
the answer to an `ios` pairing carries `features`, so the app knows what it has before its next
`ready`.

- A request the computer does not know ends the session, as anything outside the protocol does.
- A request it knows but did not offer this session is answered `not_offered` ("This computer
  does not offer that to this device."), and the session goes on.
- An answer larger than one sealed message carries (600 KiB) is answered `too_large` instead:
  sealed, it could pass the relay's limit for one frame (2 MiB), and the relay would end the
  computer's whole connection, every device with it. An `open` answered so follows nothing.
  `open` and `history` are made to fit before that: fewer steps, the rest being the next page,
  and a single step too large by itself with its long texts cut ("A chat that would not open",
  above). `too_large` is left for what cannot be cut, a picture for one.
- `open` is answered `busy` only when the chat's followers are all the window's or bots': among
  remote browsers the one of longest standing is let go (`lost`) for the new one.

What only the app asks for runs in `crates/ghost-remote/src/host/app.rs`:

- **`inbox`** is the app's Inbox in one call, read from the same chats as a bot's `inbox`
  (`render::sorted`): `asking`, one entry per request a chat waits on, `{ chat_id, request }`
  with `request` as `{ id, kind: "permission", title, options }` or
  `{ id, kind: "question", title, prompt }`, and `null` when the chat waits on something its
  newest history does not show yet; `working`, `{ chat_id, line, since }`, its latest step and
  when its turn began; and `recent`, `{ chat_id, ended_at, duration_ms, line, files, added,
  removed, pull_request }`, each turn that ended in the last day, newest first, with the bot's
  one-line brief. `files` is how many files the turn's edits changed, as the window's card under
  the answer counts them (each edit, deletion or move that did not fail, by its paths); `added`
  and `removed` are the lines those edits added and removed, as each edit's own +/− counts them,
  left out when an edit was too large to count; `pull_request` is `{ number, state, checks }` as
  the computer last heard of it in the last day (what GitHub CLI said to a reading of the chat,
  or what a task's watcher saw), never a new look. Each is left out when unknown. At most 40 of
  each part. It reads the overview and the newest page of history (up to 256 KiB) of each chat
  it lists, at most 120 pages and in practice a few, so the app asks for it again when a `chat`
  event says something changed, a moment after the last such event, and never on a timer.
- **`search`** takes `query` (1 to 200 characters, one line) and `limit` (at most 50, 20 when
  unsaid) and answers `{ hits: [{ chat_id, title, snippet, at }] }`, best first: the window's
  own search of the open chats' titles, projects, agents and finished messages
  (`workspace_search`), each hit dated by its chat in the overview. `snippet` is `null` when only
  the title, project or agent matched. It costs one search and one overview.
- **`archived`** takes `before` (Unix milliseconds) and `limit` (1 to 100, 50 when unsaid) and
  answers `{ chats: [WorkspaceChat], more }`: the archived chats that last changed before
  `before`, newest first. A page never ends between chats that changed in the same millisecond,
  so asking again with the last one's `updated_at` misses none. The overview leaves archived
  chats out; this reads the workspace with them (`include_archived`).
- **`options`**, for the app, adds `access` (`[{ value, name }]`, by the value `set_access`
  takes) and `access_mode`. A browser's answer has neither.
- **`set_access`** takes `chat_id` and `access`, exactly `ask`, `auto_edit` or `full_auto`
  (anything else is `invalid`), and sets it as the window's picker does. **`rename`** takes a
  `title`, trimmed, of 1 to 200 characters on one line. **`archive`** takes `archived` (`true`
  archives, `false` brings the chat back). **`start_agent`** starts a stopped chat's agent
  without sending anything. Each answers `{}`.
- **`changes`** takes `chat_id` and `scope`: `turn` (the default) for what the chat's last turn
  changed, as the window's Changes shows it, or `branch` for everything its own branch changed
  since that branch began, committed or not, untracked files included. A branch is counted from
  its task's recorded base where the chat does a task's work, else its worktree's base, else
  where it left its folder's default branch; it is read with the app's own bounded Git (no
  shell). It answers `{ files: [{ path, kind }], unavailable, scope, base }`, `kind` being
  `added`, `modified` or `deleted`, at most 2,000 files, and fewer when their paths would not
  fit one answer. `unavailable` says why there is no list
  (a chat with no branch of its own works in the project folder); `base` is the commit a
  branch's changes are counted from.
- **`diff`** takes `chat_id`, `path` and `scope`, and answers
  `{ before, after, binary, truncated }`: the file before and after (`null` where it did not
  exist), each side at most 160 KiB, and less when its text would not fit one answer once
  encoded (a file of control characters takes six bytes for each); `truncated` says a side was
  cut. Only a file the list of changes names is read; any other path is `not_found`. A branch's
  credential and private-key files are not shown.
- **`facts`** answers `{ branch, worktree, uncommitted, usage: [{ window, used }],
  pull_request }`: what a bot's reading ends with, and the chat's pull request in parts,
  `{ number, url, state, checks, failed: [name], notes: [{ author, kind, body, trusted }] }`,
  where `checks` is `none`, `pending`, `passed` or `failed`. At most the newest 20 notes are
  sent, each cut to 2,000 characters, and a note by someone who is not one of the repository's
  own people has an empty `body`. GitHub CLI is asked at most every two minutes per chat; a bot's
  reading and the app share what it said.
- **`memory`** takes `project_id` and answers `{ markdown, revision }`: the project's brief.
  **`set_memory`** takes `markdown` (at most 4 KiB, `too_large` above) and `base_revision`, and
  answers `{ revision }`. It is saved as the person's own edit, "Edited from" the device's name,
  as the window's editor saves: not as a bot's, and whatever the switch for agents' memory edits
  says. With `base_revision` it is saved only while the brief is still at that revision, and is
  refused `conflict` otherwise, also when another edit lands in the moment between. An empty
  `base_revision` says the device read no brief (`memory` answered `revision: null`): it is
  saved while there is still none, and refused `conflict` once there is one. Without
  `base_revision` it saves over whatever is there.
- **`picture`** takes `chat_id` and `of` (`browser` or `device`) and answers `{ mime, data }`
  (standard base64): what the chat's browser tab or emulator shows, as a JPEG made smaller step
  by step (1600, 1280, 1024, 800 and 640 pixels) until the answer stays under 420 KiB; one that
  does not fit even then is `too_large`, and the session goes on. Only a chat of the workspace's
  overview is pictured, and one picture at a time per session (`busy`).
- **`task`**, **`tasks`** and **`task_cancel`** are the phone's tasks ("Tasks from the phone").
  `task` takes `goal` (at most 64 KiB), `project_id`, `agent_id`, `review` (`true` when
  unsaid), `when` (`now`, the default, or `tonight`), `access` and `attachments` (now only), and
  answers `{ chat_id, branch, review_agent }`, or `{ queued }` for tonight. The whole goal goes
  into the task's brief, now or when it starts at night (a queued task keeps all of it); a
  started task keeps its first 2,000 characters for its review. `tasks` answers `{ tasks: [{
  chat_id, goal, branch, review_agent, reviewer_chat, created_at, by }], queued: [{ id, goal,
  project_id, agent_id, review, created_at, failed, failed_at, by }], night: { from, to, budget,
  started } }`: the last 40 tasks of the workspace, newest first, by its bot links (`bot`) and
  its devices (`device`), and everything queued for tonight, oldest first, each goal shown by its
  first 2,000 characters (200 when the whole list would not fit one answer otherwise).
  `task_cancel` takes `queued_id`, any queued task of the workspace, and answers `{}`, or
  `not_found`. A request that ran on past a switch of workspace reads and changes nothing of
  the next one's tasks or queue (`unavailable`).
- **Photos.** `upload_begin` takes `name`, `mime` (`image/png`, `image/jpeg`, `image/webp` or
  `image/gif`), `size` (at most 8 MiB) and `sha256` (hex), and answers `{ upload_id, chunk }`,
  `chunk` being 49,152 bytes. `upload_chunk` takes `upload_id`, `index` (exactly the next, from 0)
  and `data` (standard base64 of at most `chunk` bytes), and answers `{ received, complete }`;
  the last piece is checked against the SHA-256, and a photo that does not match is dropped
  (`corrupt`). `upload_abort` drops one and answers `{}`. A staged photo lives in the session's
  memory only (limits above). `send` and `create` then take `attachments`, the ids of up to 8
  whole photos, and the text may be empty with them. Each photo is kept as the chat's message
  box keeps one: a JPEG stays a JPEG and anything else becomes a PNG, decoded and made again so
  nothing but its pixels is kept (no location, no camera data), turned upright as its camera
  recorded, at most 2048, 1600, 1280 or 1024 pixels, the first under 4 MiB. A message's photos
  are measured that way before anything is made with them: together they take at most 8 MiB,
  what an agent's message may carry, or the request is refused `too_large` ("These photos are
  more than 8 MB together. Send fewer at a time.") and no attachment, chat, worktree or task is
  made. Then each is added to the chat's message box and the message is sent with them, as the
  window's message box sends one; the agent receives a JPEG as `image/jpeg`. A photo leaves the
  stage only once its message went. A browser's `attachments` are not taken.
- **`create`**, for the app, answers `{ chat, unsent }` when the chat was made and its first
  message could not be sent, `unsent` saying why; a browser is answered with the error, as
  before.
- **`image`** takes `chat_id`, `source` and `full`, and answers `{ mime, data, width, height }`:
  an image the chat holds, read as the window's preview reads it. `source` is
  `{ type: "stored", sha256 }`, `{ type: "attachment", id }` or
  `{ type: "message", context, index }`; a project file by its path is never read (`invalid`).
  A PNG that would not fit 420 KiB comes as a smaller JPEG.
- **`push_register`** takes `token` (64 to 200 hex characters, kept in lower case), `env`
  (`sandbox` or `production`) and `prefs` (`{ asking, finished, failed, tasks, scope }`, each
  `true` and `scope` `all` when unsaid; `scope` is `all` or `mine`), and answers `{ relay }`:
  whether the relay in front of the computer sends notifications now. The app may send it on
  every connection; nothing is written when nothing changed. **`push_unregister`** answers `{}`
  and the device is told nothing more. **`push_test`** answers `{}` and sends a test now; at
  most one every ten seconds per device (`busy`), only to a device that registered (`not_found`)
  and through a relay that sends notifications (`unavailable`). **`activity`** takes the
  `token` and `env` of the computer's Live Activity on the phone, and answers `{}`. See
  "Notifications on the phone".

Besides `not_offered` and `too_large`, the app's requests are answered with these codes:
`invalid` (a value outside what the request takes), `empty`, `not_found` (no such chat, file,
photo or queued task, or no notifications to test), `busy` (a picture already being taken, too
many photos staged or arriving, a test too soon), `unavailable` (no relay that sends
notifications), `corrupt`, `conflict`, and `failed` with the computer's words (a task that could
not start, for one).

The computer pushes `chat` (a status changed), `frame` (live events of the followed chat), `lost`
(open the chat again) and `reset` (another profile was opened: start over).

## Running and testing it

- **Deploy:** `corepack pnpm --filter @ghost/relay exec wrangler deploy --env production`
  (and `--env preview`). The route `onghost.dev/api/remote/*` is more specific than the website's
  `onghost.dev/*`, so those requests reach the relay.
- **Development:** a debug build reads `GHOST_REMOTE_RELAY` (for example
  `ws://127.0.0.1:8799`) and `GHOST_REMOTE_SITE` together; release builds reach only
  Ghost's own relay. A page served from `localhost` accepts `?relay=ws://127.0.0.1:<port>`.
- **Tests:**
  - `corepack pnpm --filter @ghost/relay worker:test`: the relay in the Workers runtime, the app's
    path and its mark included. CI runs it.
  - `cargo test -p ghost-remote`: the service against a relay on this machine, with a simulated
    browser, iPhone app and bot: every request of the app reaching the action it should, a
    browser refused each of them, photos, pictures, the phone's tasks now and tonight, and its
    notifications (nothing before `f`, waiting and dropped after two minutes, `drop`, what each
    phone chose, `resolved`, reviews and queued work, the test's rate, the Live Activity), with
    the sealed vectors (`tests/push-vectors.json`).
  - `corepack pnpm --filter @ghost/web test`: the browser's channel against the shared
    vectors, and the page's headers.
  - `apps/desktop/scripts/check-remote.mjs` (part of `check:settings`): the Settings page.
  - `corepack pnpm --filter @ghost/relay worker:e2e`: everything together on this machine.
    It needs the website built and `cargo build -p ghost-remote --example demo_host`, a stand-in
    computer with a scripted agent. The demo answers every request Ghost for iPhone makes: a
    turn's and a branch's changes with one file and its diff, a branch whose pull request passed
    its checks, a brief with revisions, a picture, tasks now and for tonight, and photos. `--engines chromium,webkit` runs it in both, `--scheme dark`
    reports a dark system, and `--shots <folder>` keeps screenshots. Midway it silences the
    phone's socket without closing it and expects the page back on a new one.
  - `node apps/relay/tests/native.mjs <ghost.exe> [--prompt]`: the real debug app in a data
    folder of its own, paired from a separate browser and called by an MCP client. It opens one
    chat from four sessions and freezes their pages, as a sleeping phone's are, then opens it
    from a fifth, loads that page again, and wakes the first. `--prompt` also runs a turn
    through the development build's scripted agent, sends a 700 KiB message in the window and
    reads the chat back from a new session.

## Not done

What comes next for bots (Ghost telling a bot when a chat finishes or needs it, results that carry
proof, a whole task in one call, approval rules per link) is planned in
[superpowers/plans/2026-11-23-bots-manage-tasks.md](superpowers/plans/2026-11-23-bots-manage-tasks.md).

- **A relay that notices a browser is gone.** The relay ends a computer that is silent for 75
  seconds, but not a browser: a phone that slept keeps its place at the relay, and at the
  computer, until its socket is closed from one end. The computer works around that since 28
  November 2026 (it makes room in a full chat and in a full list of connections). The cure is
  the relay asking its browsers, which is a change to the relay and to what it costs to keep a
  socket asleep.

Security, from the review of 10 November 2026 (each is a known limit, not a surprise):

- **A knock the relay can check.** The computer would register a one-way value made from each
  device key and from the open pairing key, and the relay would admit only sockets that present
  one. That closes the handshake flood above. Today the relay cannot tell a paired browser from
  a stranger until the computer accepts it.
- **Forward secrecy.** Session keys come from the pairing or device key and two nonces sent in
  the clear. Someone who recorded a pairing session at the relay and later got the spent link
  could work out the device key. An ephemeral key exchange in the hello and welcome would end
  that; better still, a key pair per device, with only its public half on the computer.
- **Device keys at rest.** They are in `remote.json` as they are, readable by the same user.
- **A code both sides confirm.** Pairing needs a tap on the phone now, but the computer does not
  confirm which browser it paired. Whoever opens the link first within its ten minutes is the
  device; Settings shows the new device at once and Remove ends it.
- **A bot link that is sealed end to end**: it would need the bot to run a program of ours.
- **The relay on an origin of its own.** It answers under `onghost.dev`, the page's origin.
- **The addresses before a move.** The site was auracode.si until 19 November 2026 and o7.codes
  until 26 November. A copy of the app from before a move connects to the relay at the address
  it knew and makes pairing links to its `/remote`, and a phone paired there keeps its key under
  that origin (a key cannot be moved between origins: it is stored so that no script can read
  it). So the relay answers on all three addresses, into the same rooms, and lets in a browser
  from any of their origins (`FORMER_ORIGINS`); the website Worker serves only `/remote` and its
  files at the old addresses and sends every other page to onghost.dev. Remove an old address's
  routes, its entry in `FORMER_ORIGINS` and `FORMER_HOSTS` and its entry in the page's
  `connect-src` once the copies from its day have updated.
- **Credentials belong to the site they were issued under.** A browser keeps a device's key
  under the site's origin, and a bot's token is part of an address at the site, so whoever holds
  that address later holds a page that can use the key and receives the token. Each device and
  bot link therefore records the site it was issued under (`site` in `remote.json`), and a copy
  of the app removes, when it starts, every one issued under a site other than its own, in every
  workspace (`Registry::set_site`). A record from before sites were recorded counts as the old
  address. So the first start after a move ends every pairing and bot link made at the address
  before it (auracode.si, then o7.codes): the phone pairs once more and the bot is linked once
  more, at onghost.dev, and nothing that still works is tied to an address being given up. An
  `ios` device stays: Ghost for
  iPhone keeps its key in the iPhone's Keychain, under no web origin, so a page at the old
  address cannot use it, and its `ready` names the new site.
- **Per-room rate limits** at Cloudflare's edge for new sockets and host claims. Since 19 November
  2026 the zone has one rule per address instead: more than 60 requests to `/api/remote/*` in ten
  seconds from one address are refused for ten seconds (infra/README.md). It blunts a flood from
  one place; it does not tell rooms apart, and a flood from many addresses passes it.

- **A bot's address in a config file or a shell's history.** Hermes takes the address in
  `~/.hermes/config.yaml` and OpenClaw in a command, so the token is written where those keep
  things, in the clear. It is the same secret a Grok or Muse connector stores on its service;
  removing the link in Settings ends it wherever it is written.

Features:

- **Telling a bot through MCP itself.** A link is told through its assistant's own webhook
  ("Telling a bot when a chat is done"). The MCP Events extension would let a bot subscribe
  without its person copying an address and a key; too few assistants take it yet.
- A read-only level for a paired device (bot links have one).
- New chats in a worktree, attachments, images, the queue's controls and the access level from
  the page. Ghost for iPhone has them (features `control`, `review`, `memory`, `screens`, `tasks`
  and `attachments`); the page keeps the pickers it had.
