# Native Claude replay evidence

The `recorded-*` fixtures come from completed Claude Code 2.1.310 calls on Windows on
2026-10-26. The six smoke recordings contain seven prompts. `recorded-thinking` contains
one separate Sonnet call at medium effort. Curation and replay made no additional agent calls.

| Fixture | Recorded behavior |
| --- | --- |
| `recorded-basic` | Fresh session, streaming and committed text reconcile to `ok`. |
| `recorded-permission` | Ask mode denies Bash, then allows Write on the next prompt; both tool lifecycles survive. |
| `recorded-interrupt` | Interrupt after partial output `1`, ending as cancelled. |
| `recorded-after-interrupt` | The next resumed turn remembers the last emitted number, `1`. |
| `recorded-full-auto` | Native `bypassPermissions` completes Write without a permission callback. |
| `recorded-resume` | The resumed session recalls both files created during the smoke run. |
| `recorded-thinking` | Empty thinking block and three empty thinking deltas, followed by a completed thought and 302 reported reasoning tokens. No reasoning text is invented. |

These are curated recordings, not complete transcripts of the local environment. The first
note in each fixture documents the reductions. The curation tool replaces home, working
directory, user, host, native session ID and ISO timestamp values with placeholders. Native
session placeholders receive deterministic UUIDs only inside replay. Thinking signatures
and local hook output use explicit omission markers.

The initialize model metadata is retained. Advertised commands are limited to
`help`, `usage`, `compact`, `context`, `cost`, `model`, `permissions` and `clear` when present.
Account data, local integration catalogs and environment inventories are omitted.
Context replies retain their recorded `totalTokens` and `maxTokens`; usage replies retain
session statistics and rate-limit windows. Omitted inventory fields are not fabricated as
empty vendor responses. Tool input and result meaning, control envelopes, turn text,
result accounting and record order remain intact.

Windows paths can cross native `input_json_delta` boundaries. Curation therefore reassembles
each tool input, redacts its values, and distributes changed JSON across the original number
of delta records. Those redacted chunk boundaries are synthetic; unchanged inputs preserve
their original chunks. A regression test covers interleaved main and subagent inputs, UTF-8,
incomplete input and a split unknown home path that must fail curation.

Replay follows record order, not the recorded wall-clock intervals. Golden files normalize
durations. ISO quota reset timestamps become `<TIME>` and cannot test date parsing here;
the synthetic telemetry fixtures verify the exact parsed reset timestamp. The recordings
verify native driver behavior through a fake process, not current account availability,
vendor server behavior or filesystem side effects. The real smoke and acceptance harness
results remain the evidence for those external effects.

`tests/replay.rs` checks these recordings against their goldens and adds semantic assertions
for text deduplication, option validity, permission outcomes, tool completion, cancellation,
resume content and withheld reasoning. It also scans every fixture and golden for personal
data. Existing ACP fixtures are independent of this evidence.
