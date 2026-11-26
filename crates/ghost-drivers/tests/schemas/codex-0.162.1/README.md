# Codex 0.162.1 schema baseline

`protocol.json` is the unmodified stable bundle produced on Windows by:

```text
codex --version
codex-cli 0.162.1
codex app-server generate-json-schema --out <ignored task directory>
```

Source file: `codex_app_server_protocol.schemas.json`.
SHA-256: `71a94eab0d0b27f2aab30993097bc13d78314eb386801c105280b53f0963af44`.

Generation does not start a conversation. No prompts, credentials or personal configuration were
captured. Tests validate synthetic protocol fixtures against this bundle with remote resolution
disabled. The historical 0.164.0 installation recorded in the plan is not verified by these tests.

The adjacent `LICENSE` and `NOTICE` are copied unchanged from OpenAI Codex tag
[`rust-v0.162.1`](https://github.com/openai/codex/tree/rust-v0.162.1). They apply to the upstream
material, including this generated protocol baseline, and do not select a license for Ghost.
