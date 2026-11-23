# Changelog

What changed in the o7 app, newest first, under the day it merged in US Eastern time. Only
changes to the app are listed: a change to the website, the docs, the tests or the project's
tooling alone gets no line (owner, 21 November 2026). Each line is written in simple English
about what changed for the person using o7: short everyday words, short sentences, no internal
names, and the headline first (owner, 16 November 2026). A pull request that changes the app
adds its own line in the same pull request; see "Changelog" in
[docs/developing-in-o7.md](docs/developing-in-o7.md). `node .github/scripts/check-changelog.mjs`
checks the format, and with `--dates` checks each day against the time GitHub says it merged.

## 2026-11-23

- Version numbers are shorter. o7 now calls itself by two numbers, like 0.1, with the build beside it where it matters ("0.1 (build 1787)" in the update panel and in Settings > About) instead of one long number. ([#184](https://github.com/itscool2b/o7/pull/184))
- o7 can now tell your assistant when a chat is done. A linked bot used to keep asking how a task was going, which cost it time and tokens. In Settings > Remote access, each bot link has a new Call back button: give it your assistant's webhook address and key (with steps for Grok Bot, Hermes and OpenClaw), and o7 sends one short message when a chat that bot started finishes, fails or asks for an answer. The key stays in your computer's secure storage and is never shown again. You can send a test from the same place. ([#183](https://github.com/itscool2b/o7/pull/183))
- An assistant linked to o7 now gets results it can trust. When it hands your agents a task, what comes back lists the commands that ran and how each ended, and names the one still failing with the first lines of its error, so it can tell you whether the tests passed instead of guessing. One new tool, inbox, tells it in a single call which chats are asking for an answer, which are still working and what the finished ones came to. Waits are shorter so they no longer run into an assistant's own time limit. ([#182](https://github.com/itscool2b/o7/pull/182))
- Linking an assistant works better. The steps for Grok Bot were for the wrong app and are now right (Connect apps, then Add a custom connector). When you make a link, o7 tells you the moment your assistant first reaches this computer, and each link shows how many calls it has made and how many failed. A bot that started a task could be told the chat was "stopped" before the agent had begun; it now waits for the real result. ([#181](https://github.com/itscool2b/o7/pull/181))
- A chat is ready as soon as you open it. After a restart, a chat's agent used to start only when you sent a message, so you could not put it in Plan first. Now you can pick Plan before the agent runs, and the agent starts by itself once the chat has been open for a second. A long plan is also shown as a plan again, with Accept, Edit, Request changes and Decline: a plan over a few pages used to appear as a cut-off block of code with only Allow once and Deny. ([#180](https://github.com/itscool2b/o7/pull/180))
- When you switch a chat to another agent, the new agent now gets the real state of the work, not just the conversation. The handoff lists the commands that ran and how each ended, the folder they ran in, and the last command that was still failing with its first error, so the new agent does not have to find the broken test again. You can see exactly what it was sent: before you confirm, while it waits above the message box, and later on the message that carried it, where it opens from one line instead of filling your message. ([#177](https://github.com/itscool2b/o7/pull/177))
- o7 now says once a day that a copy is running, so we can count how many people use it. When it looks for updates, it also tells o7.codes its version, its system and whether this is its first time. It sends no name, no ID and nothing about your work, and the site keeps only each day's totals. Turning off the update check in Settings turns this off too. ([#175](https://github.com/itscool2b/o7/pull/175))

## 2026-11-22

- A chat no longer dies while its agent waits on background work. When Claude Code left a command running in the background (a build, tests, a server) and you went to another chat, o7 stopped that agent after ten minutes, so the chat never carried on. Now o7 leaves it running until that work is done. ([#174](https://github.com/itscool2b/o7/pull/174))
- Dictation's better ear now arrives by itself. About a minute after o7 starts, it downloads the larger speech model once (486 MB), so there is nothing to press. You can stop or remove it in Settings > Voice, and o7 will not fetch it again unless you ask. It waits if your disk is nearly full. ([#173](https://github.com/itscool2b/o7/pull/173))
- Dictation can get a much better ear. Settings > Voice now offers a larger speech model as a one-time 486 MB download. With it, dictation makes about 40% fewer mistakes in our tests, gets programming words right ("WebSocket", "TypeScript", "async await", "a 401"), and understands 24 more European languages. It runs on your computer like the built-in one, and you can remove it again. o7 checks the file before using it, and you can cancel the download and continue it later. ([#172](https://github.com/itscool2b/o7/pull/172))
- Dictation keeps up on slower computers. It now times its own work and spaces it out, so the live words arrive in bigger steps instead of the microphone falling behind, and finishing a short dictation no longer reads it twice. ([#170](https://github.com/itscool2b/o7/pull/170))
- o7 can read an answer aloud. Press the speaker under any answer and a natural voice reads it; press it again to stop. The voice runs on your computer: nothing is sent anywhere, it needs no account, and it starts almost at once. Code blocks are skipped, since code is hard to follow by ear. Settings > Voice has 38 English voices to choose from, with a button to hear each. Installers are about 60 MB larger because the voice comes with the app. Dropdowns also take the keyboard after a click on a Mac. ([#168](https://github.com/itscool2b/o7/pull/168))
- A plan now reads like a document, not like a tool asking for permission. When an agent finishes planning, its plan shows with its own title, its sections and its steps, and your answers stay in view under it. Accept it, decline it, or ask for changes with a note. Edit opens the plan's own text: change it, then accept your version (Claude Code is told your version is the approved plan) or send it back for the agent to keep planning. Ctrl+Enter accepts. ([#167](https://github.com/itscool2b/o7/pull/167))
- o7 has a tour. Its wing lifts off from the logo at the foot of the sidebar, flies to each part of the window in turn and points at it, and a small card says what that part is for. You are asked first, it takes about a minute, and Escape ends it. Nothing is dimmed or locked while it runs. When a later update adds something, you are offered a short tour of only the new part. Settings > General > Tour takes it again. Setup is livelier too: the wing flies in on your first launch and marks how far along you are. ([#166](https://github.com/itscool2b/o7/pull/166))
- Project memory is now a short brief plus notes. Every chat in a project still gets the brief, so keep in it only what every chat needs. Everything else goes in notes: each has a name and one line saying when to read it, and an agent reads a note only when it fits the task. Agents add or change one note at a time, so adding a fact no longer pushes another out, and chats stop paying for memory they do not use. Settings > Project memory lists the notes under the brief. Memory you already have is split at its headings when o7 updates, with nothing lost. ([#165](https://github.com/itscool2b/o7/pull/165))
- An agent that updates project memory is no longer sent the whole memory back with your next message. It wrote that text itself a moment before, so sending it again only used up your allowance. Other chats in the project still get the update, marked as an agent's. Agents are also told, as soon as they connect, what project memory is for and what does not belong in it. ([#164](https://github.com/itscool2b/o7/pull/164))
- o7 opens with your projects and chats already there. A long-used o7 used to show an empty sidebar for about ten seconds, with a red "database is busy" line, before your chats appeared. Now the window opens with them in place, and o7 goes straight to your chats instead of showing its logo first. Clearing up after chats that were still working when you quit happens once the window is open, not before it. If your chats really cannot be read, the message has a Retry button. Diagnostics shows how long opening took. ([#163](https://github.com/itscool2b/o7/pull/163))

## 2026-11-21

- A phone beside your chat, for building phone apps. Press the phone button in a chat and o7 starts an Android phone that you and your agent both use: the agent can install your app, tap, type, read the screen and check the logs. Each chat gets its own phone. The first time, o7 shows what it will download from Google and the licence, then sets Android up for you. Your own Android phone on a cable works too, once you allow the agent to use it. Turn it off in Settings > Mobile. ([#162](https://github.com/itscool2b/o7/pull/162))
- Your own assistant can run your coding agents, and setup looks better. Link Grok Bot, Muse, Hermes or OpenClaw in Settings > Remote access or during setup: pick yours and o7 shows what to paste and where. A linked assistant now needs fewer steps, so it uses far less of its own allowance: one request sends your message and brings back the answer. Setup has a picture that fills in as you go. Plan mode no longer asks you to allow every command when the chat is on Full access; it still asks before any file edit. ([#159](https://github.com/itscool2b/o7/pull/159))
- New chat's agent menu shows every agent in full again. It had shrunk to the width of the agent button, cutting off names and the search box and scrolling sideways. ([#158](https://github.com/itscool2b/o7/pull/158))

## 2026-11-20

- The Dashboard shows how many lines your agents changed over the past year. Each day is a square that gets brighter the more lines were changed, with the year's total, your busiest month and day, and your longest and current streaks. It counts every agent, in chats and in terminal chats, but only work done in o7. Pick All or one agent to see only theirs, and point at a square to see that day's count. ([#157](https://github.com/itscool2b/o7/pull/157))
- Plan and build is now part of each chat, and you can watch the agent think. Open the model menu in a chat and choose, under Plan and build, the model it plans with and the agent and model it builds with; Settings no longer has Models by role. While the agent thinks you see its newest thoughts, and afterwards "Thought for 4 s" opens to all of them. Running steps shimmer, each group of steps shows icons for what it did, and plans show their numbered lists again. ([#155](https://github.com/itscool2b/o7/pull/155))

## 2026-11-19

- o7 installs as o7. This update replaces the auracode program with o7: the Start menu, desktop and taskbar open o7, and auracode leaves your list of apps. Your chats, projects, settings and sign-ins stay exactly where they were, and the code lives at github.com/itscool2b/o7. ([#154](https://github.com/itscool2b/o7/pull/154))
- auracode is now called o7. The app's window and everything it says use the new name; your chats, settings and installed app carry on as they are, and the program on disk keeps its old name for now. On your phone, the message box now sits right on the keyboard, with no empty space under it and the header still in view. ([#153](https://github.com/itscool2b/o7/pull/153))
- The Update button looks right. Its word ran to the edge of the button with no space around it; it now sits inside with room on both sides. ([#152](https://github.com/itscool2b/o7/pull/152))
- auracode has a new address, o7.codes, and using your computer from your phone works properly now. The phone page looks like your app, in your theme, light or dark. It comes back by itself after your phone sleeps, a chat opens on the first tap, a message shows the moment you send it, and long answers no longer stutter. It also tells you when your computer is asleep. After this update, pair your phone and link your bot once more: both now use o7.codes, and the ones made at the old address stop working, so nothing is left tied to it. ([#151](https://github.com/itscool2b/o7/pull/151))
- Models by role. Settings > General now lets you name an agent and model for each kind of work: plan with one, build with another. Turn Plan on and the chat moves to your Planning model; when a plan is ready, Accept and build with hands it to your Building model, in the same chat or to another agent with the plan carried over. A coordinated project's coordinator and workers take their own. ([#150](https://github.com/itscool2b/o7/pull/150))
- Coordinated projects, an experiment. Turn it on in Settings > Experiments and each project gets a board button: start a coordinator with a brief, and it plans the work, hands it to worker chats that each start fresh in their own Git worktree, and reports back to you. The project keeps a task board, a decision log every agent is shown, shared Context files that keep each earlier version, and routines that run on a timer. Workers cannot start workers or record decisions, at most 4 work at once, and Pause stops everything. ([#149](https://github.com/itscool2b/o7/pull/149))
- auracode updates itself. When a newer version is out, an Update button shows at the foot of the sidebar: one press downloads it, checks it and restarts. auracode looks for a new version a few times a day, and you can turn that off in Settings. Every update is checked against a signature before it installs. ([#148](https://github.com/itscool2b/o7/pull/148))

## 2026-11-18

- auracode has a new logo: a flying wing with two eyes. It is on the app, its icons and the taskbar. ([#145](https://github.com/itscool2b/o7/pull/145))

## 2026-11-17

- auracode opens on new aircraft pictures. The fifteen text drawings are replaced by ten new pictures, from the Wright Flyer and the P-51 to the 747, the SR-71 and the Space Shuttle. Each one is drawn in your theme's own colours, light or dark, and you still see a different one each start. ([#143](https://github.com/itscool2b/o7/pull/143))
- New chats no longer break your other chats, and a second project works. Slow work in one folder no longer holds up the rest. When agents are in the way, auracode names the chats and offers to stop them, and at the limit of 16 agents it stops the one idle longest instead of refusing. Terminals open in every project, two projects can set up at once, and a folder you pick stays in the project it belongs to. ([#142](https://github.com/itscool2b/o7/pull/142))
- auracode looks flat, soft and clean, after the Grok Bot app, in every theme. Glass, blur and glow are gone, dark mode uses faint edges instead of shadows, and thin text is a little heavier. New chat starts with the message box, so you type first, every dropdown opens auracode's own menu, and Settings and setup show your agents joining auracode. ([#140](https://github.com/itscool2b/o7/pull/140))
- Terminals open your own shell, and the keyboard stays where you type. Terminal tabs start your shell with your settings, full colour and your terminal's font, and shortcuts no longer lose your place. The Dashboard drops chats left from earlier days, and any card can be cleared or stopped. On Linux, the browser no longer stops on pages with sound or video, a page that stops can be reloaded, and menus over a page show its picture. ([#139](https://github.com/itscool2b/o7/pull/139))
- auracode no longer has accounts or sync. Claude Code and Codex now sync plugins on their own, so auracode does not need to. There is no sign-in: your chats, projects and settings stay on your computer. Remote access from your phone still works. ([#137](https://github.com/itscool2b/o7/pull/137))
- Linux is smooth on NVIDIA graphics, and the browser works on Wayland. Scrolling and streaming now hold 60 frames a second where they dropped to about 20. The browser beside the chat opens on Wayland instead of asking for X11, and agents' terminals work when auracode runs as an AppImage. ([#136](https://github.com/itscool2b/o7/pull/136))

## 2026-11-16

- Choose a chat's model and agent from your phone. On auracode.si/remote, a chat's header now has "Model and agent". The Instructions, Project memory and Remote access pages in Settings share one cleaner look. Updating an agent now waits until its chats stop working, then shows the result. ([#135](https://github.com/itscool2b/o7/pull/135))

## 2026-11-11

- Behind the scenes: the Mac app builds again. A missing piece needed by dictation had stopped the Mac installer from being made. ([#134](https://github.com/itscool2b/o7/pull/134))
- Agent updates say what happened, and MCP servers come first. Update now tells you the version it reached, or that nothing changed. Plugins & MCP opens on your MCP servers, with Add server at the top. The browser has its own Settings page, and a bot link can be for chats only or for full control. ([#133](https://github.com/itscool2b/o7/pull/133))

## 2026-11-10

- Cards look flat, and the Grok Build theme has new colours. Cards in Settings, the Dashboard and chats lose their borders and shadows. The dark Grok Build theme is now dark grey with off-white text. ([#132](https://github.com/itscool2b/o7/pull/132))
- Remote access stays inside one workspace. A phone or bot link made in one workspace can no longer reach another workspace on the same computer. Opening another workspace ends the first one's remote sessions, and the page says so instead of asking you to pair again. ([#131](https://github.com/itscool2b/o7/pull/131))
- The desktop app gets a tidier look. Your sent messages stand out more, Search sits at the top of the sidebar, and Attach has its own round button beside Send. A chat that is waiting for you shows a badge in its header. ([#130](https://github.com/itscool2b/o7/pull/130))
- Settings pages are shorter and clearer. Project memory is now just your notes and one switch. Account & sync shows the four steps to set up syncing, and turning sync on keeps your theme and defaults. ([#129](https://github.com/itscool2b/o7/pull/129))
- See and steer your computer's chats from your phone. Scan the code in Settings > Remote access, then open auracode.si/remote to read chats, send messages, stop a turn, answer permission requests and start chats. A bot link lets an assistant such as Grok Bot do the same. Everything sent between your phone and your computer is encrypted, and it all stays off until you turn it on. ([#128](https://github.com/itscool2b/o7/pull/128))
- Project memory sent with a message is now tucked behind one line. The chat used to show the whole memory as part of your message. Now it shows only your words, with "Project memory" underneath to open what the agent received. ([#127](https://github.com/itscool2b/o7/pull/127))

## 2026-11-09

- Several terminal chats can run at once. Before, a second terminal chat was refused while the first was running. Now up to eight can be open across the app. ([#123](https://github.com/itscool2b/o7/pull/123))
- A long turn shows just its answer when you reopen the chat. Before, a very long turn showed every step. Now its steps fold under "Worked for" like any other finished turn. ([#122](https://github.com/itscool2b/o7/pull/122))
- Behind the scenes: release builds work again. A change that came with the built-in browser had stopped installers from being built. ([#121](https://github.com/itscool2b/o7/pull/121))
- A browser now sits beside the chat. Open it with the globe in the chat's header. It has tabs, an address bar, saved passwords and downloads. Agents can open, read and click pages in it but never see your passwords, and Annotate lets you mark part of a page and send a note about it to the agent. ([#120](https://github.com/itscool2b/o7/pull/120))
- The message box is plainer, and Canvas narration no longer overflows. The coloured ring around the message box is gone. In Canvas, what you say scrolls inside four lines under the drawing, and Finish puts the drawing and your words straight into the message. ([#119](https://github.com/itscool2b/o7/pull/119))

## 2026-11-08

- OpenCode and Pi now start on Windows. OpenCode installed with npm and Pi installed with its own installer both showed as broken before. They now work in Chat and in Terminal. ([#118](https://github.com/itscool2b/o7/pull/118))
- Canvas lets you draw a sketch and add it to your message. Open it beside Attach, then draw with pens, shapes, arrows and text. Press Talk to explain the drawing as you make it. Add to message attaches the drawing and what you said. ([#116](https://github.com/itscool2b/o7/pull/116))
- Terminal now works for more agents. Grok Build, OpenCode, Hermes, Pi, oh-my-pi, Cursor and Antigravity can open in their own interface inside auracode, as Claude Code and Codex already could. What happens in the terminal comes back into the chat. ([#115](https://github.com/itscool2b/o7/pull/115))

## 2026-11-07

- auracode opens on a military aircraft drawn in text characters. There are fifteen, and you see a different one each start. Notices now wait behind a bell beside Settings instead of floating over the page. New chat opens only when you ask for it. ([#114](https://github.com/itscool2b/o7/pull/114))
- Notices moved to the top right, under the header. They fold into one until you point at them, and each has a short title and one line. When an agent update fails, Run in terminal runs it again so you can see its output. ([#113](https://github.com/itscool2b/o7/pull/113))
- New chat opens on a fighter jet drawn in text characters. There are sixteen, and you see a different one each time auracode starts. It replaces the logo and title. ([#112](https://github.com/itscool2b/o7/pull/112))

## 2026-11-06

- A chat's access no longer says "Access unavailable" before its first message. A chat opened after a restart shows its access and lets you change it. A new Claude chat that never got a message now starts again after a restart instead of failing. ([#111](https://github.com/itscool2b/o7/pull/111))
- New chat no longer opens on a long list of folder paths. Folders kept from deleted chats now sit behind one "Retained worktrees" line. It opens by itself only when one of them needs recovery. ([#110](https://github.com/itscool2b/o7/pull/110))
- A terminal chat counts as Working only while its agent is busy. On the Dashboard, one that is waiting at its prompt sits under Finished as "Open in the terminal". ([#109](https://github.com/itscool2b/o7/pull/109))
- In a terminal chat, every key now goes to the agent. For example, Shift+Tab changes Claude Code's mode instead of moving around the app. Ctrl+Tab leaves the terminal and Ctrl+Shift+C copies. The Dashboard also fills its space again and keeps its cards in order. ([#108](https://github.com/itscool2b/o7/pull/108))
- Terminal chats arrive for Claude Code and Codex. A chat can open in the agent's own interface inside auracode, from the chat's header, from New chat, or as the default in Settings. It keeps the chat's account, MCP servers and project memory, and the conversation comes back into the chat when it ends. ([#107](https://github.com/itscool2b/o7/pull/107))
- Notices now appear at the bottom right, above the message box. Each shows the agent's logo and an Update button. Grok Build and Cursor can now be updated from the app when they were installed with their own installers. ([#106](https://github.com/itscool2b/o7/pull/106))
- Chats and projects show up again after an update or a restart. Before, the sidebar could stay empty with "database not available". It now fills in as soon as the app's data is ready. ([#105](https://github.com/itscool2b/o7/pull/105))
- Choose which agents can use each shared MCP server. A server can be for every agent, including ones you install later, or only for the agents you pick. Set this when you add or edit it in Plugins & MCP. ([#104](https://github.com/itscool2b/o7/pull/104))
- The sea lion and turtle animation is gone. They no longer swim along the message box while an agent works. ([#103](https://github.com/itscool2b/o7/pull/103))
- A notice tells you when an agent has a newer version. It has an Update button and comes back at each start until you update. Typing while an agent works no longer makes the chat flicker. ([#102](https://github.com/itscool2b/o7/pull/102))
- Processes is now the Dashboard. It shows every chat as a card in three columns, Needs you, Working and Finished. A card opens its chat and shows what a working agent is doing. ([#101](https://github.com/itscool2b/o7/pull/101))
- Load earlier now brings back a bigger piece of the chat at once. It keeps loading until 20 more turns appear, and long chats stay light. ([#100](https://github.com/itscool2b/o7/pull/100))
- The agent list in New chat now opens downward. A long list no longer runs under the title bar and hides its search box. ([#99](https://github.com/itscool2b/o7/pull/99))

## 2026-11-05

- Releases now include Linux. Once a signed release is out, auracode on Linux updates itself as it does on the other systems. ([#96](https://github.com/itscool2b/o7/pull/96))
- auracode now runs on Linux. It comes as deb, rpm and AppImage packages. The tray icon, notifications, Keep awake, sounds, dictation and terminals all work, and files open in VS Code, Cursor or Zed. ([#93](https://github.com/itscool2b/o7/pull/93))
- Share one agent's MCP server with every agent. In Plugins & MCP, choose Preview and copy, then Share with every agent. Its keys stay on this device. Agents that cannot use shared servers now start without them and say so, instead of refusing to start. ([#92](https://github.com/itscool2b/o7/pull/92))
- Terminals come back after a restart. A chat's terminal panel offers the terminals it had open, and starts them again when you choose Resume. Nothing starts by itself, and earlier output is not kept. ([#91](https://github.com/itscool2b/o7/pull/91))
- Worktree chats get a Run button. Save a Run command for the folder, then start it from the chat's header and open its address until you press Stop. Each worktree gets local ports of its own, so several can run side by side. ([#90](https://github.com/itscool2b/o7/pull/90))
- File names in a chat open in your editor at the right line. This works for paths in an agent's answers, in a step's files, in the changed-files card and in Changes. Choose the editor in Settings > Chats. ([#89](https://github.com/itscool2b/o7/pull/89))
- A new setting plays a sound when a chat needs you. You hear a short, quiet tone when a chat finishes, fails or needs you, even while auracode is in front. It is in Settings > General and is off by default. ([#88](https://github.com/itscool2b/o7/pull/88))
- Search inside a chat with Ctrl+F (Cmd+F on Mac). A bar counts and highlights every match in your messages and the agent's answers. Step through them with Enter and Shift+Enter. ([#87](https://github.com/itscool2b/o7/pull/87))
- Commit and push your changes from inside auracode. In the Git workspace dialog, pick the changed files, write a message and review the commit before it is made. Then push the branch. ([#86](https://github.com/itscool2b/o7/pull/86))
- Changes folds long files and can undo a single change. Each change shows with three lines around it and the rest folded. Revert above a change puts back just those lines. ([#85](https://github.com/itscool2b/o7/pull/85))
- Comment on lines in Changes and send the comments to the agent. All your comments go as one message that quotes each line. You can also ask the agent to review the changes and report problems before it edits anything. ([#84](https://github.com/itscool2b/o7/pull/84))
- More agents show a command's output as it runs. Where a chat said "terminal output is not available", you now see the output as it comes and how the command ended. ([#83](https://github.com/itscool2b/o7/pull/83))
- MCP server forms now show in Claude and Codex chats. Forms and browser steps that MCP servers ask for used to be cancelled. The Session tab also lists what the session loaded, such as MCP servers, skills, plugins and subagents. ([#82](https://github.com/itscool2b/o7/pull/82))
- Fork keeps the agent's memory, and Rewind restores your files. Fork into a new chat from the latest answer continues Claude's or Pi's own session, with everything it knew. Rewind on a sent message puts files back to how they were before it, then lets you edit the message. You can also steer Pi while it works. ([#81](https://github.com/itscool2b/o7/pull/81))
- Answers now show coloured code, maths and diagrams. Code uses the theme's colours, maths is set out properly, and Mermaid diagrams are drawn, with their source a click away. ([#80](https://github.com/itscool2b/o7/pull/80))
- Agent steps show more detail. An opened tool call shows what it was given, a web search lists the pages it found as links, and an edit counts its changed lines. Claude's to-do list shows as a checklist, and the chat's menu has Compact conversation. ([#79](https://github.com/itscool2b/o7/pull/79))
- Settings is now one screen with a search box. The search finds any setting. Chats can keep the steps of finished turns open, and Voice can test your microphone. ([#78](https://github.com/itscool2b/o7/pull/78))

## 2026-11-04

- Dictation is built in and has no time limit. It runs on your device. Words appear at the cursor as you speak, Finish adds them to your message and Cancel removes them. ([#77](https://github.com/itscool2b/o7/pull/77))
- A finished turn folds its work under one line. The line reads like "Worked for 1 min 42 s · 12 steps" and sits above the final answer. A card under the answer names the files that changed. ([#76](https://github.com/itscool2b/o7/pull/76))
- More of a chat fits on screen. Text is a little smaller and answers use the full width. Choose the Comfortable text size in Settings to get the earlier size back. ([#75](https://github.com/itscool2b/o7/pull/75))
- Claude Code's theme is now the default look. This applies only if you never chose a theme. A theme you picked, Aura included, is kept. ([#74](https://github.com/itscool2b/o7/pull/74))
- The controls around the message box are quieter. Only Send is filled in. Your account shows as its initial with a ring for its usage, and the picker lists models, thinking levels, then every agent. ([#73](https://github.com/itscool2b/o7/pull/73))

## 2026-11-03

- Settings offers a theme for every supported agent. There is one each for Claude Code, Codex, Grok Build, Pi, Antigravity, Cursor, OpenCode, oh-my-pi and Hermes, beside Aura. Each comes in light and dark and applies at once. ([#72](https://github.com/itscool2b/o7/pull/72))
- Grok Build and Hermes now show their plugins and marketplaces. Claude Code marketplaces show where they come from again. You can add a marketplace from a GitHub repository, a Git address or a local folder. ([#71](https://github.com/itscool2b/o7/pull/71))
- See everything your agents are running. A new Processes page lists each chat's subagents, background commands and programs, with a Stop button. Stop in a chat now says it also ends running tasks. ([#70](https://github.com/itscool2b/o7/pull/70))
- Dictation now records on Windows. Before, small audio glitches ended every recording after a tenth of a second. Turning speech into text is also much faster, under a second for 11 seconds of speech instead of 18. ([#68](https://github.com/itscool2b/o7/pull/68))
- The working line shows the model maker's logo. While an agent works, you see the logo of the company whose model is doing the work. ([#66](https://github.com/itscool2b/o7/pull/66))

## 2026-11-02

- Subagents and background commands appear above the message box. Click one to open a panel that follows its progress. ([#65](https://github.com/itscool2b/o7/pull/65))
- Switch a Claude Code chat's account from the message box. You can also see how much each account has used, and read a plan before the agent runs it. The app's glow effects are gone. ([#63](https://github.com/itscool2b/o7/pull/63))
- Behind the scenes: installing a locally built copy of the app on Windows is safer. If the new copy fails to start, the previous app comes back. More of the app is also tested automatically. ([#62](https://github.com/itscool2b/o7/pull/62))
- Image attachments show small previews. You see them before sending and in the chat's history, and can open one to see it larger. ([#61](https://github.com/itscool2b/o7/pull/61))
- Dictation in the message box works offline. It needs a one-time download first, which is checked before use. ([#60](https://github.com/itscool2b/o7/pull/60))
- Newly installed agents are found without restarting auracode. This also covers Cursor's own install on Windows. ([#59](https://github.com/itscool2b/o7/pull/59))
- Links in Settings open the right Customize page. Plugins & MCP, Instructions and Project memory open in Customize, and your chat and draft stay as they were. ([#58](https://github.com/itscool2b/o7/pull/58))
- Questions and permission requests appear one at a time. They show in the order they arrived, with a count of how many are left. ([#57](https://github.com/itscool2b/o7/pull/57))
- A working turn shows one line with the time so far. Tool details also open more smoothly. ([#56](https://github.com/itscool2b/o7/pull/56))
- The model picker switches models reliably. It also keeps the thinking level in view. ([#55](https://github.com/itscool2b/o7/pull/55))
- Agents can now edit project memory directly. A switch in Customize > Project memory turns this off, so that their edits wait for your review instead. ([#54](https://github.com/itscool2b/o7/pull/54))

## 2026-11-01

- A Claude chat that hits its usage limit can switch accounts. It carries on with another account you added, and keeps its history. ([#53](https://github.com/itscool2b/o7/pull/53))
- Behind the scenes: auracode can be developed safely from inside auracode. Test builds keep their own data and run separately, so they never touch your installed app. ([#48](https://github.com/itscool2b/o7/pull/48))
- New worktree chats get the folder's private instruction files. Instruction files that Git ignores used to be missing there, so agents ran without them. This is on by default and can be turned off. ([#47](https://github.com/itscool2b/o7/pull/47))
- The desktop app gets a new look called Halo. Windows look like glass over your system's own window background. ([#46](https://github.com/itscool2b/o7/pull/46))
- Shared MCP servers and project memory arrive, with new chat controls. Access and Plan controls sit beside a searchable picker for agent, model and effort. Customize holds Plugins & MCP, instructions and project memory. ([#45](https://github.com/itscool2b/o7/pull/45))

## 2026-10-31

- Automatic checkpoints are now off unless you turn them on. Ordinary messages send without the delay of saving a snapshot of your files first. Checkpoints saved earlier stay available. ([#44](https://github.com/itscool2b/o7/pull/44))
- Messages reach the agent faster when checkpoints are on. Saving a checkpoint of your files before each message takes far less time. ([#43](https://github.com/itscool2b/o7/pull/43))

## 2026-10-30

- The desktop app gets a new look, called Aura. It follows your system's light or dark setting, and you can choose Light or Dark yourself. ([#42](https://github.com/itscool2b/o7/pull/42))

## 2026-10-29

- An interrupted clone can be recovered safely. If copying a repository is cut short, auracode remembers it and offers to open the folder or try again in another folder. ([#41](https://github.com/itscool2b/o7/pull/41))
- Interrupted changes to Instructions recover safely. Edits you made to those files outside auracode are not lost. ([#40](https://github.com/itscool2b/o7/pull/40))
- The Plugin library fills in one agent at a time. You see results sooner, and the row menus stay where they are. ([#39](https://github.com/itscool2b/o7/pull/39))
- You can write messages while a new worktree is being set up. They wait in a queue and send when the setup is done. ([#38](https://github.com/itscool2b/o7/pull/38))
- Revert is safer. Before it changes your project, auracode saves checked copies of the original files so they can be recovered. ([#37](https://github.com/itscool2b/o7/pull/37))
- Cleaning up saved sign-ins removes only auracode's own entries. Running two copies of the app at the same time is also safer. ([#36](https://github.com/itscool2b/o7/pull/36))
- Sent messages are tracked more reliably. A message being sent is no longer mixed up with another, and held messages keep their order. ([#35](https://github.com/itscool2b/o7/pull/35))
- Sending, queues and recovery are more reliable. You can write during setup, held messages keep their order, interrupted work recovers safely, and the Plugin library fills in step by step. ([#34](https://github.com/itscool2b/o7/pull/34))

## 2026-10-28

- Fixes for Windows. The terminal panel now works, the title bar matches the app, and choosing a project in New chat no longer changes the agent you picked. Names and setup messages are clearer too. ([#33](https://github.com/itscool2b/o7/pull/33))
- A queued message now says why it is waiting. It also no longer waits forever when an agent fails to report that it has finished. ([#32](https://github.com/itscool2b/o7/pull/32))
- The Claude, Grok, Antigravity and Hermes logos are added. Every supported agent now shows its real logo in the app. ([#31](https://github.com/itscool2b/o7/pull/31))
- Codex, Cursor, OpenCode, Pi and oh-my-pi show their real logos. You see them beside each agent in the app. ([#30](https://github.com/itscool2b/o7/pull/30))
- An agent's opened details on the Agents page line up neatly. This is a small layout fix, with a new automatic check for it. ([#29](https://github.com/itscool2b/o7/pull/29))
- Follow-ups to the water design. The Agents and Plugin library pages sit in the same panel as chat, and the swimming animals use far less of your computer's power. The old dotted background is removed. ([#28](https://github.com/itscool2b/o7/pull/28))
- Each agent now appears once in the list. Before, some had a second entry for another way of connecting. That choice is now a Connection setting on the Agents page, and it is automatic by default. ([#27](https://github.com/itscool2b/o7/pull/27))
- Uninstalling auracode removes all of its own data. Settings can do the same. Your coding agents are left alone. ([#26](https://github.com/itscool2b/o7/pull/26))
- The app gets the water design. The chat sits in a rounded panel, the opening screens show lit water with a seal and a turtle, and small spaces have gentle motion. ([#25](https://github.com/itscool2b/o7/pull/25))
- A new Instructions page manages what your agents are told. Use one shared file for every agent, or a file per agent. You review every change before it is saved. ([#24](https://github.com/itscool2b/o7/pull/24))
- The Plugin library has a section for each agent. It lists what each agent itself reports, shows whether an update is available, and syncs agent by agent. ([#23](https://github.com/itscool2b/o7/pull/23))
- Fixes from testing on Windows. Changes made just before quitting are no longer lost. The foot of the sidebar is now only the logo and a Settings button. ([#21](https://github.com/itscool2b/o7/pull/21))
- Clone a repository from inside auracode. It is offered when you first set up and in New project. Making an account is an optional step, and more agents can be installed and signed in to from the app. ([#20](https://github.com/itscool2b/o7/pull/20))

## 2026-10-27

- Fixes found by testing the app on Windows. Among them, the first message in a worktree chat is no longer held back, and changed lines show in the right order. ([#18](https://github.com/itscool2b/o7/pull/18))
- A terminal panel sits at the bottom of each chat. There is also help with merge conflicts, where you can ask the agent to resolve them or open the folder in your editor. More editors are supported, such as Zed and the JetBrains ones. ([#17](https://github.com/itscool2b/o7/pull/17))
- Small gaps are closed across chats and Settings. You can edit and reorder queued messages, open the first nine chats with Ctrl or Cmd plus a number, and set a default agent and model. Settings adds switches for notifications, Keep awake and running in the tray. ([#16](https://github.com/itscool2b/o7/pull/16))
- New chats get their own worktree automatically. A worktree is a separate copy of the project, so chats do not get in each other's way. You can still switch a chat to Local or change its branch before you create it. ([#15](https://github.com/itscool2b/o7/pull/15))
- Behind the scenes: groundwork for chats in their own worktree. The app can now create a chat in a separate copy of the project and recreate a deleted copy. The buttons for it come in a later change. ([#14](https://github.com/itscool2b/o7/pull/14))
- Every screen of the app is redesigned. The sidebar, header, chat and account row all have a cleaner look. ([#13](https://github.com/itscool2b/o7/pull/13))
- OpenCode can be installed with one click on the Agents page. ([#12](https://github.com/itscool2b/o7/pull/12))
- Behind the scenes: the desktop app's buttons, fields and dialogs now share one style. This prepares for the redesign of each screen. ([#11](https://github.com/itscool2b/o7/pull/11))
- The seal and turtle logo is now used everywhere. It replaces the old pixel letter in the app and in every icon. ([#7](https://github.com/itscool2b/o7/pull/7))
- The app gets a dark look called Graphite. It is black with one lime accent colour. This change also adds the view that shows a chat's conversation. ([#6](https://github.com/itscool2b/o7/pull/6))

## 2026-10-25

- The first real agent chats work. auracode can now run Hermes, Claude Code and Codex in one chat window. ([#2](https://github.com/itscool2b/o7/pull/2))
- Behind the scenes: the project is started. It has the basic app window, safe storage on your device and automatic checks. ([#1](https://github.com/itscool2b/o7/pull/1))
