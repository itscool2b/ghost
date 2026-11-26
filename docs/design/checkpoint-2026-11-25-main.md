# Main: review record

The record of how the Main styling pass was checked, one entry a step. What Main is: [visual-language.md](visual-language.md), "Main". What it was measured against: [reference-audit.md](reference-audit.md). The order of work: [the plan](../superpowers/plans/2026-11-25-main-look.md).

Evidence here is of two kinds, and they are not the same thing:

- **Browser fixtures**: the desktop's `?preview=` states on its Vite server, opened in o7's own browser beside the chat, and the check scripts that drive the same states in Chromium and WebKit. They show layout, colour, contrast and behaviour.
- **Native**: a real window on a real system. The caption, the window material behind the sidebar, native menus and focus are only proven here.

## Step 1: foundation, sidebar and message box (25 November 2026)

What changed: the dark base palette (`#131313` panes, `#1b1b1b` raised; tertiary text stays `#959595`), headings a size smaller, the icon stroke, three control heights and a row height, a 20px radius; buttons, fields, switches, menus, dialogs and the segmented control; the sidebar's rows with each chat's agent mark and time; the message box's bare controls and 28px Send; the sent message as a soft step of ink in both appearances; Main as the default theme with a one-time move for profiles on the earlier default; the iPhone app's generated palettes.

Seen in browser fixtures, in dark and in light, with the Main palette:

| State | What was looked at |
| --- | --- |
| `settled` | Sidebar rows, header, sent message, answer, code block, action row, message box |
| `settled`, model menu open | Menu radius and rows, group lines, thinking levels, agent grid |
| `settled`, Changes open | Panel tabs without a track, file rows, diff |
| `tool-details` | Work fold, tool group row, checklist, compaction notice |
| `new` | New chat's stage and message box |
| `settings&page=appearance` | Navigation rows, the open page, segmented controls, the theme gallery with Main first |

Checked by tests: the shared UI's token and theme tests (contrast at 4.5:1 on every layer), and the desktop's unit tests for the files touched. The full suites ran on GitHub; their results are in the pull request. A darker tertiary grey was tried twice and given up: `#8e8e8e` measured 4.44:1 in the token tests on a control stacked on a card, and `#909090` measured 4.49:1 in the dashboard's accessibility scan, on small text over a lit card. The grey is `#959595`, as it was, and every layer under it is darker than before, so each of those pairs gained contrast.

## Steps 2 to 4: chat, side panes, dialogs and what floats (26 November 2026)

What changed: the message box's menus open above the box on its edge; queued messages and background tasks are sheets of a tray tucked behind the box; New chat's box wears the chat's bare chips and 28px Start; the project dialog was rebuilt (name and colour on one line, folders as a card of rows, one quiet foot) and gained a preview of a saved project's settings (`?preview=project-settings`, in the layout check's list); chat options puts the name and Rename on one line; dialogs open on their first field, a focused field is one line, labels are quiet, and a dialog's other actions are ghosts beside its filled answer; the Git dialog's repository is a flat card, its section actions start on the text's edge and its foot is a hairline; changed-file rows are 30px; the command palette's rows are 10px with a flat highlight; the Dashboard's calendar is ink; the code block's language sits on its line.

Seen in browser fixtures with the Main palette, in dark and in light unless noted:

| State | What was looked at |
| --- | --- |
| `new-project`, with a folder chosen, and its clone dialog | First focus on the name, colour discs, the chosen folder's field, the clone form's fields and its filled answer beside a ghost Cancel |
| `project-settings` | Folder rows with their actions, a folder not found, a kept worktree to review, Remove project, the foot |
| `chat-options` (dark) | The name with Rename beside it, sections |
| `settled` with each of the box's menus open | Add, access and mode at the box's left edge, account and model at its right, quiet headings |
| `steering` with two messages queued, `tasks` | The queue and the task row as sheets behind the box |
| `tool-details`, `rich-answer`, `thinking`, `permission`, `plan`, `questions`, `diff` (dark) | Transcript rows, code and diff wells, the request cards in the box's place, the plan sheet |
| `new`, `new-worktree` and its worktree menu | The box, its chips, the lines under it, the bare worktree chip |
| `settled#changes` and its Session tab, `terminal-panel`, `browser`, `device` (dark) | File rows, the diff, the tabs, the shell tabs, the address bar, the device list |
| `git` and `git&changes` | The repository card, the commit form's spacing, the foot |
| `dashboard`, `coordinator` (dark) | The lanes, the ink calendar, the project board |
| `settings`, `settings&page=remote&remote=on`, `setup`, `launch` (dark) | Navigation, cards of rows, the setup card |
| `settled#palette` (dark) | Scope pills, rows, the highlight |

Not looked at in this pass, and so not claimed: Plugins and MCP, Instructions, Project memory, the Agents page, accounts, the tour, the canvas and the find bar. They take the foundation's primitives and tokens but nobody opened them.

Checked by tests: the desktop's 1,030 unit tests, typecheck and the style lint locally; the browser suites on GitHub, whose results are in the pull request. One measurement shaped a decision: the first foundation run failed its accessibility scan on the Dashboard, where tertiary text at `#909090` measured 4.49:1 on a lit card, so the tertiary grey went back to `#959595`.

## Steps 5 and 6: the website and the remote page (26 November 2026)

What changed: the public site was rebuilt for Main (always dark, one ink and one grey, two-tone headings at the left edge, chapters on hairlines, the app on monochrome stages); the coloured wings and the screenshot of the app are gone; the notices page is dark too. The remote page takes Main's sent message (a soft step of ink in both appearances) and radii, and gained a development-only fixture (`/remote?fixture`) so it can be looked at without a paired computer.

Seen on the development server in o7's own browser and in headless captures: the home page top to bottom at 1132 and 1440 wide, at 390 wide in slices, the download page whole, and the remote page's chat list at 390 wide in dark and in light. o7's own browser asks for reduced motion, so there the field was its still drawing; the moving field, the live window and the list that moves on by itself were seen in the tests' captures with motion on.

Checked by tests, in Chromium and in WebKit: the site's static tests and its browser tests, rewritten around the new page. They hold the page to the same near-black under a dark system, a light system and no scripts; to no sideways scroll at 2560, 1440, 1024, 768, 390 and 320; to accessibility scans of the home page (at 1440 and 320, still and moving), each of the explorer's views, the phone menu, and the changelog, download, privacy and terms pages; and to the field going live with motion and staying a still drawing without it.

Not seen: the remote page's chat screen, sheets and requests (the fixture stands in for the list only), the site on a real phone, and the deployed preview. The preview is walked before the merge; what that walk finds is added here.

Not seen natively: the window's caption and material. Their colours did not change (`--g-window` and `--g-fg-2` are as they were, and the Rust test that pins them to the caption still holds), so nothing native was expected to move. The next approved build is the first real window with Main.
