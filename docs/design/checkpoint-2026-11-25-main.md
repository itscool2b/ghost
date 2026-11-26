# Main: review record

The record of how the Main styling pass was checked, one entry a step. What Main is: [visual-language.md](visual-language.md), "Main". What it was measured against: [reference-audit.md](reference-audit.md). The order of work: [the plan](../superpowers/plans/2026-11-25-main-look.md).

Evidence here is of two kinds, and they are not the same thing:

- **Browser fixtures**: the desktop's `?preview=` states on its Vite server, opened in o7's own browser beside the chat, and the check scripts that drive the same states in Chromium and WebKit. They show layout, colour, contrast and behaviour.
- **Native**: a real window on a real system. The caption, the window material behind the sidebar, native menus and focus are only proven here.

## Step 1: foundation, sidebar and message box (25 November 2026)

What changed: the dark base palette (`#131313` panes, `#1b1b1b` raised, `#909090` tertiary text), headings a size smaller, the icon stroke, three control heights and a row height, a 20px radius; buttons, fields, switches, menus, dialogs and the segmented control; the sidebar's rows with each chat's agent mark and time; the message box's bare controls and 28px Send; the sent message as a soft step of ink in both appearances; Main as the default theme with a one-time move for profiles on the earlier default; the iPhone app's generated palettes.

Seen in browser fixtures, in dark and in light, with the Main palette:

| State | What was looked at |
| --- | --- |
| `settled` | Sidebar rows, header, sent message, answer, code block, action row, message box |
| `settled`, model menu open | Menu radius and rows, group lines, thinking levels, agent grid |
| `settled`, Changes open | Panel tabs without a track, file rows, diff |
| `tool-details` | Work fold, tool group row, checklist, compaction notice |
| `new` | New chat's stage and message box |
| `settings&page=appearance` | Navigation rows, the open page, segmented controls, the theme gallery with Main first |

Checked by tests: the shared UI's token and theme tests (contrast at 4.5:1 on every layer, with the tertiary grey raised from `#8e8e8e` to `#909090` after it measured 4.44 on a control stacked on a card), and the desktop's unit tests for the files touched. The full suites ran on GitHub; their results are in the pull request.

Not seen natively: the window's caption and material. Their colours did not change (`--g-window` and `--g-fg-2` are as they were, and the Rust test that pins them to the caption still holds), so nothing native was expected to move. The next approved build is the first real window with Main.
