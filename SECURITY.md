# Security

## Reporting a vulnerability

Report it privately: open the repository's **Security** tab and choose **Report a vulnerability**.
That goes to the maintainer alone. Please do not open a public issue or pull request for a
vulnerability, and do not test against other people's computers or the hosted relay beyond what is
needed to show the problem.

A useful report says what you can do that you should not be able to, the version or commit, your
operating system, and the steps. A proof of concept helps. Never include a real key, token or
pairing link: make a throwaway one.

Ghost is maintained by one person. Expect an answer within a week, and a fix or a plan as soon
as the problem is understood. You will be credited in the changelog unless you ask not to be.

## What is in scope

- **The desktop app**: reading or changing files, chats or settings it should not, running
  something the user did not ask for, leaking an agent's credentials, or getting around a
  permission prompt.
- **Remote access**: the pairing flow, device keys, bot links, the relay (`apps/relay`) and the
  phone page. [docs/remote-sessions.md](docs/remote-sessions.md) says who is trusted with what and
  lists the limits that are already known under "Not done"; a report about one of those is still
  welcome when it shows more than the list says.
- **The website** at onghost.dev and its Worker (`apps/web`, `apps/web-worker`).
- **The build and release tooling** (`.github/`, `crates/ghost-release`).

The coding agents Ghost drives (Claude Code, Codex and the others) are their vendors' software:
report a problem in one of them to its vendor.

## What to know about the installers

The installers on the download page are unsigned preview builds. They are not signed by a
publisher or notarized, and they do not update themselves. Each one is listed with its SHA-256
checksum; compare it before installing. A signed release channel exists in the code
([docs/releases.md](docs/releases.md)) and is not in use yet.
