# Contributing

Thanks for looking. Ghost is maintained by one person, so small, focused pull requests are the
ones that get merged. For anything larger than a fix, open an issue first and say what you want to
change and why.

## Setting up

[docs/development.md](docs/development.md) lists the prerequisites and how to run the app. A development
build keeps its data in a folder of its own (`dev.onghost.desktop.dev`), so it never touches an
installed Ghost.

## Before you open a pull request

Run the checks that cover what you changed, from the repository root:

```sh
pnpm check                      # lint, typecheck and the JavaScript tests
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The hosted CI is started by hand, not by a push, so a pull request shows no test results of its
own. Say in the description what you ran and what you saw.

- **Add a line to `CHANGELOG.md` if you change the app.** The changelog lists changes to the app
  only, not the website, docs, tests or tooling: newest first under the day it merges in US
  Eastern time, written for someone who uses the app: what changed, then what
  they will notice.
  [docs/developing-in-ghost.md](docs/developing-in-ghost.md) has the format, and `pnpm lint` checks
  it.
- **Commit generated files with the change that needs them.** After changing a Rust command or a
  type it uses, run `cargo test -p ghost-desktop export_bindings` and commit
  `apps/desktop/src/bindings.ts`. After changing any `package.json` or `Cargo.toml`, run
  `node .github/scripts/third-party-notices.mjs --write`.
- **No real-looking secrets, anywhere.** A scanner checks every commit. Tests that need a token
  build one at run time instead of writing `ghp_...` or `sk-...` in a file.
- **Tests for the refusals.** A change to anything that takes a key, a link, a token or a
  connection needs tests for what it turns away, not only for what it accepts.

## How the project is built

- **Rust first.** Agent drivers, storage, processes, remote access and release tooling are Rust.
  React and TypeScript are for the interface.
- **The interface follows one visual system.** Read
  [docs/design/visual-language.md](docs/design/visual-language.md) before changing how anything
  looks, and extend the shared components and tokens instead of adding new ones.
- **Agents run as their vendors ship them.** Ghost never reads, stores or proxies an agent's
  credentials and never adds hidden prompts.
- **Plain writing.** Short sentences, no em dashes, no marketing words, in the interface, the
  docs and commit messages alike.

## Security

Do not open a public issue for a vulnerability. [SECURITY.md](SECURITY.md) says how to report one.

## License

Ghost is released under the [MIT License](LICENSE). By contributing, you agree that your
contribution is released under the same license.
