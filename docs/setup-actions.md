# Explicit setup actions

## Native agents

The Agents and onboarding screens offer reviewed npm installation for missing Claude Code, Codex,
Pi, OpenCode 1 and Grok Build on Windows and macOS. The versions match the catalog's acceptance
pins: Claude 2.1.310, Codex 0.162.1, Pi 1.0.2, OpenCode 1.19.4 and Grok Build 1.0.70. These are
integration pins, not a claim that the full current milestone or every platform has passed
real-agent acceptance.

Reviewing an installation inspects only executable/package metadata. It does not install, sign in,
read npm configuration or read agent state. Rust resolves Node plus npm's actual `bin/npm-cli.js`,
checks the npm package name, and presents the exact program, argument vector and working directory.
The Start action refreshes detection and checks the reviewed paths, arguments and executable bytes.
It refuses an agent that is no longer missing. No shell shim, command interpolation, administrator
elevation or execution-policy override is used.

The explicit action runs npm in the existing owned, unlogged terminal. npm itself uses its ordinary
global prefix and configuration. A user must have a suitable Node/npm installation and permission
to write that prefix. Closing the sheet, closing/reloading its page or reaching ten minutes stops
only the owned installer tree. A failed or canceled installation can leave a partial npm package;
the app preserves npm's files and offers normal refresh/review instead of deleting them. Success
does not imply authentication or availability until detection has checked the installed agent.

Install sources checked 2026-10-26 and 2026-10-27:

- [Claude installation documentation](https://code.claude.com/docs/en/setup#install-with-npm)
  documents `npm install -g @anthropic-ai/claude-code` and its required optional native packages
  and postinstall step. Its install keeps lifecycle scripts enabled.
- [Codex 0.162.1 README](https://github.com/openai/codex/blob/rust-v0.162.1/README.md)
  documents `npm install -g @openai/codex` as a supported package-manager installation.
- [Pi 1.0.2 README](https://github.com/earendil-works/pi/blob/v1.0.2/packages/coding-agent/README.md)
  documents `npm install -g --ignore-scripts @earendil-works/pi-coding-agent`, requiring
  Node 22.19 or newer. Pi keeps that explicit script restriction.
- [OpenCode documentation](https://opencode.ai/docs) documents `npm install -g opencode-ai`,
  the package that carries OpenCode 1.x (1.19.4).
- [Grok Build enterprise setup](https://docs.x.ai/build/enterprise) documents
  `npm install -g @xai-official/grok` as xAI's npm distribution. The package is published and
  signed by xAI's own npm account (`xai-security`); 1.0.70 is its `latest` tag and the catalog's
  minimum. It needs Node 20 or newer.

The other catalog agents keep their official guides, for these reasons:

- **Cursor**: the [CLI installation](https://cursor.com/docs/cli/installation) is a script
  (`curl https://cursor.com/install | bash`, `irm 'https://cursor.com/install?win32=true' | iex`)
  that fetches the latest build with no published checksum; there is no official npm package.
- **Hermes**: the [installation guide](https://hermes-agent.nousresearch.com/docs/getting-started/installation)
  offers a script or app installers with no version choice or checksum. Nous's PyPI package lags
  the GitHub releases and the docs do not mention it.
- **oh-my-pi**: its package declares the Bun runtime and its CLI starts with `#!/usr/bin/env
  bun`, so npm with Node cannot run it; its own installer fetches the script live.
- **OpenCode 2**: it ships as `@opencode/cli`, whose `opencode` command would collide with an
  OpenCode 1 install from `opencode-ai`, and its [documentation](https://opencode.ai/v2/docs)
  says Windows package managers are not supported (Windows uses a download).
- **Antigravity**: Google distributes a desktop app and an `agy` install script; neither offers a
  pinned version or checksum in the vendor's documentation.

### Discovering an installation added while Ghost is open

The installation observer refreshes the resolved environment before its 30-second metadata
check. Newly created known tool directories and changes to the Windows registry PATH or Unix
login-shell PATH can therefore become available without restarting Ghost. Executable probes run
only when installation metadata or saved choices change; running sessions keep their original
environment and executable. The agent list picks up background results on its regular refresh.

Cursor's official Windows installer places `cursor-agent.cmd` beside `versions/<build>/node.exe`
and `index.js` under `%LOCALAPPDATA%/cursor-agent`. That directory is a known tool location. Ghost
qualifies the `@anysphere/agent-cli-runtime` manifest and runs the bundled Node with its entry
directly. It does not execute the shell launcher. Version discovery is bounded, supports dated
and timestamped builds, ignores incomplete distributions, and confines runtime files to their
distribution. The vendor-named `cursor-agent` entry remains required; a generic `agent` executable
is not assumed to be Cursor. The [current installation guide](https://cursor.com/docs/cli/installation)
also documents the shorter `agent` alias. This adds discovery support, not automatic installation.

### Sign in

Sign in runs the agent's own documented login command for the detected executable, in the
owned terminal sheet, after its separate Start. Ghost never opens credential files, and the
vendor's flow (a browser, a device code or a key prompt) happens in that visible terminal:

- Claude Code `claude auth login`, Codex `codex login`;
- Cursor `cursor-agent login` ([authentication](https://cursor.com/docs/cli/reference/authentication),
  which opens the browser);
- Grok Build `grok login` ([CLI reference](https://docs.x.ai/build/cli/reference));
- OpenCode `opencode auth login` (choose a provider, then sign in or paste its key);
- Hermes `hermes auth` (its interactive provider sign-in; `hermes login` was removed).

Pi, oh-my-pi and Antigravity sign in inside their own interface (`/login`, or the first run), so
Ghost offers no Sign in for them and says nothing it cannot run. Status checks (Check sign-in)
stay limited to Claude Code and Codex, whose status commands report a coarse state without
account data.

Add account, for Claude Code's added accounts, runs `claude setup-token` the same way: the same
owned sheet and separate Start. It prints a token the person pastes into the dialog, which goes to
secure storage. [Agent accounts](agent-accounts.md) has the rest.

Fresh Node/npm installation and vendor authentication are separate user actions. Development tests
use synthetic packages, fake browser replies and a harmless local terminal child, never a real
sign-in command.
## Cloning a repository

Setup's first-project step and New project both offer Clone a repository. The person pastes an
address, chooses the folder it goes into and sees the exact command before anything runs:
`git clone --progress -- <address> <folder>`, with the Git executable found on this computer.

**Addresses.** Rust accepts `https://host[:port]/path`, `ssh://[user@]host[:port]/path` and
scp-like `user@host:path` (git@github.com:owner/repository.git); a pasted `github.com/owner/repo`
gets its `https://`. Everything else is refused with its reason in one sentence: `http://` and
`git://` are unencrypted, `file://` and local paths belong to Choose a folder, transports such as
`ext::` and `fd::` run commands, a user name or token in an https address would be stored in the
clone's configuration, `?` and `#` parts and GitHub pages inside a repository (`/tree/main`) are
not repositories, and hosts or paths that start with a hyphen could be read as options. The folder
name defaults to the repository's own (Git's choice) and must be one portable name.

**Checking is inert.** Every change to the form asks Rust again (typing waits 200ms) and Rust
answers with the first problem beside its field (address, folder, name or Git) or a review that
holds the exact program, arguments and destination for ten minutes. Answers for an older form are
dropped and their reviews released; each window keeps at most eight. No folder is created and
Git does not run until Clone.

**The person's own Git.** Git runs with the person's own configuration, credential helpers, SSH
keys and agent, exactly as in their terminal. Ghost never reads, stores or passes credentials.
It removes only the variables that would point Git at another repository (`GIT_DIR`,
`GIT_WORK_TREE` and the like) and sets `GIT_TERMINAL_PROMPT=0`, because no terminal is attached:
a helper with its own window (Git Credential Manager, the macOS keychain) still answers, and a
missing sign-in fails with a reason instead of waiting forever.

**The one folder Ghost owns.** Start creates the destination itself, atomically, so an
existing folder of that name, even an empty one, is refused and never reused. Git clones into
it. Progress streams from Git's `--progress` lines (stage, the whole clone's percentage and what
has arrived) through a long poll that answers at each change. Cancel, closing the form, the page
going away or quitting stops Git's whole process tree; only after the stop is confirmed does Rust
remove the unfinished folder, and only while the parent is still the chosen folder and the name
still holds the folder created at start. A failed clone is removed the same way, and keeps Git's
last eight lines (never logged) under a plain reason: sign-in, not found, untrusted SSH host,
unreachable server, full disk, path too long or unwritable folder. A finished clone is checked
to hold `.git` and becomes the project: at once in setup, through Create project in New project.
A clone still running after two hours is stopped. If Ghost itself ends mid-clone, the
unfinished folder stays. The clone form in onboarding and New project lists previous attempts
after restart, with the destination, Open folder when its identity is known, Retry in another
folder, and Dismiss notice. Dismiss removes only the recovery record. A missing, replaced,
linked or unavailable destination is never removed or reused by recovery. A completed clone
with its recorded directory identity and Git directory can enter normal project addition;
the destination is checked again when that action is selected. The notice stays until dismissed,
so a failed project-add step does not lose the existing clone.

Rust saves a versioned lifecycle record in `<data>/clones` before creating a destination, after
creation, after Git exits successfully, and on settlement. The record contains the validated
address, destination, directory identities and phase. Raw Git output, progress and environment
variables are never persisted there. Files are published atomically with synced contents and
read-back verification; Unix also syncs publication directories. Windows power-loss durability
has not been established. If journal publication fails, Git does not start or the completed
folder is retained with an explicit recovery error. Malformed records and unknown files stay
untouched until an explicit record-only dismissal.

The Git behavior follows the official [clone documentation](https://git-scm.com/docs/git-clone)
and [environment variables](https://git-scm.com/docs/git#_environment_variables). Rust tests
clone a local bare repository through a `file://` URL with an empty Git configuration (the form
itself refuses `file://`), fail on a missing repository, cancel against a server that never
answers, and remove a folder holding read-only pack files while keeping one put in its place.
