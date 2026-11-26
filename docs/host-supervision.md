# Host process supervision

M1e has a Rust Linux endpoint and a fail-closed host boundary. WSL execution is still unavailable:
the Windows bridge, helper deployment, artifact qualification and host preparation are pending.
The presence of `wsl.exe`, a distribution or an installed agent does not enable launch.

`HostSpawner` receives an explicit `HostExecution`. Native execution passes the original spec to
the existing spawner. Other hosts return `SpawnError::HostUnavailable` before a subprocess starts.
The application must inject a qualified deployment after preparing the Linux cwd and identity
realm. A global host enum must not enable execution before those checks.

## Linux endpoint

`ghost-host-supervisor --stdio <absolute-state-directory>` serves one connection. `--version`
reports the package and protocol versions without resolving an environment or starting an agent.
The endpoint uses the existing environment resolver, targeted scrub, redactor, log factory,
process spawner, descendant tracking and leftover registry. The Linux environment remains local.
Provider configuration and credential contents are not inspected. Logs live under the explicit
state directory, with the same redaction rules as native launches.

Protocol version 2 frames consist of a one-byte tag, a four-byte big-endian payload length, then that many bytes.
Control payloads are JSON and capped at 1 MiB. Agent stdin/stdout payloads are separate raw UTF-8
frames, capped at the existing 64 MiB line limit, so JSON escaping cannot multiply their allocation.
The output queue also has count and byte budgets. Malformed headers fail before payload allocation.

| Parent request | Behavior |
| --- | --- |
| Launch | Exactly one explicit Linux program, argv, cwd, reviewed environment overrides, daemon rules and output cap; no shell interpolation or automatic retry. |
| Stdin | One agent protocol line; newline-bearing frames are rejected. |
| CloseStdin | Ends only agent stdin, while ownership and the lease remain active. |
| Heartbeat | Renews the 15-second parent lease. |
| Stop | Stops accepting commands and shuts down the owned tree. Queued prompts are discarded. |
| Inspect | Before launch only: resolves catalog executable names, cwd and known provider configuration roots from metadata, returning a connection-bound preparation token. It exports no generic environment map and executes no discovered program. |

Ready identifies the owned Linux pid. Stdout and oversized-line keys retain their protocol meaning.
Ended contains the direct exit, any unconfirmed leftovers, declared daemons spared, and a redacted
stderr tail. Endpoint signals, parent EOF, lease expiration, malformed control and broken output
all use the existing shutdown chain. A blocked output consumer cannot prevent lease cleanup.
Buffered output drains for the existing bounded EOF grace. A timed-out output does not promise
complete transcript delivery. Unconfirmed process cleanup leaves recovery records for a later
instance rather than claiming success or forgetting the process identities.

Inspection preserves Linux path case and resolves symlink aliases without reading config files.
It uses the same scrub-then-override environment as launch. Only `LaunchSpec.set_env` values may
be sent from the parent; the Linux login environment is never exported. Override validation and
Debug output omit values, and secret overrides join the Linux log redactor before process spawn.
The supervisor binds inspection and launch to one connection and rechecks effective cwd,
configuration roots and executable paths after final driver overrides. A changed realm, changed
symlink target or token from another connection rejects launch. Unreadable roots and dangling
configuration symlinks fail inspection. Heartbeats keep preparation alive until launch; Stop
discards it without starting an agent. The low-level unprepared Launch path is for endpoint tests
and carries no host identity proof; the application bridge must always use preparation.
The launched program must also be one of the inspected executable paths. Adapter preparation
must explicitly include its Node executable; a valid token cannot authorize an unrelated binary.
The bridge must translate Windows drive paths using the actual distribution's `wslpath`, since
custom automount settings can change the mount root. Do not assume `/mnt/c` or normalize Linux
configuration roots with Windows case folding.

## Windows daemon boundary

Windows does not support removing a process from a job after assignment. Explicit breakaway
requires both a compatible enclosing job and a child created with `CREATE_BREAKAWAY_FROM_JOB`.
Silent breakaway releases all children and therefore loses the current kernel guarantee that
ordinary tool descendants die with their owner. See Microsoft's
[Job Objects contract](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects).

This checkpoint does not enable either breakaway mode or change vendor processes. Safe Windows
daemon support still needs qualified vendor behavior, tracked escaped pid/start identities and
crash recovery. An existing daemon outside Ghost's job remains outside its kill authority.

## Verification

| Check | Result |
| --- | --- |
| Host gate fake-spawner regressions | pass, included in the 176-test process unit run |
| Supervisor framing/lifecycle/inspection validation | pass, 11 focused tests on Windows |
| Process all-target Clippy and rustfmt | pass |
| Existing process unit and helper integration checks | pass, 176 unit and 9 helper tests before the final inspection/signal additions |
| Remaining local process integration binaries | pending, Windows Application Control blocked execution |
| Linux real owned-child supervision and recovery tests | pending CI |
| WSL bridge, packaged helper deployment and distribution qualification | pending |
| Windows cooperative daemon breakaway and recovery | pending |

Only Ghost's fake/test children were used. No real vendor agent, paid prompt, credential/config
read, security-policy change or blocked-binary workaround was performed.
