# o7 relay

The Worker behind remote sessions: one Durable Object per computer holds its outbound host socket
and the sockets of paired browsers and o7 for iPhone (`/app/`, with no Origin), forwards between
them, and serves MCP to linked bots. It stores
only the hash of a computer's key and the hashes of its bot tokens. How it fits together, who is
trusted with what and how to test it are in [docs/remote-sessions.md](../../docs/remote-sessions.md).

It is TypeScript where the other Workers are Rust: Durable Object WebSocket hibernation is native
there, and this Worker only forwards. Every decision about who may do what is made on the computer.

```powershell
corepack pnpm --filter @o7/relay worker:test    # the relay in the Workers runtime
corepack pnpm --filter @o7/relay worker:e2e     # relay, service, remote page and a bot
corepack pnpm --filter @o7/relay exec wrangler deploy --env production
```
