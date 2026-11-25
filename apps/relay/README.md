# o7 relay

The Worker behind remote sessions: one Durable Object per computer holds its outbound host socket
and the sockets of paired browsers and o7 for iPhone (`/app/`, with no Origin), forwards between
them, sends the computer's notifications for o7 for iPhone to Apple, and serves MCP to linked
bots. It stores only the hash of a computer's key, the hashes of its bot tokens and when its recent
notifications went out. How it fits together, who is trusted with what and how to test it are in
[docs/remote-sessions.md](../../docs/remote-sessions.md).

It is TypeScript where the other Workers are Rust: Durable Object WebSocket hibernation is native
there, and this Worker only forwards. Every decision about who may do what is made on the computer.

```powershell
corepack pnpm --filter @o7/relay worker:test    # the relay in the Workers runtime
corepack pnpm --filter @o7/relay worker:e2e     # relay, service, remote page and a bot
corepack pnpm --filter @o7/relay exec wrangler deploy --env production
```

## Notifications

The computer seals what a notification says for the phone, then asks the relay to send it with a
`p` frame on its socket: the phone's address at Apple, the device it sealed for, and the sealed
blob. The relay sends it to Apple inside fixed outer words, "o7" and "Something changed on your
computer.", and never with a category. A computer's key is self-made, so no computer may put
words of its own on a phone; the phone adds its buttons only once it has opened the sealed words.

- Each `p` is answered with `P`: Apple's status and reason, and `drop` when the phone's address is
  no good any more. A frame the relay cannot send is answered `invalid` and never ends the socket.
- Each computer may send 60 notifications a minute and 600 an hour, 30 a minute to one phone, to
  at most 32 phones an hour. Over that the answer is `limited`, and nothing is sent.
- One Durable Object, `Apns`, signs for the whole relay. It makes the ES256 token Apple wants,
  keeps it in its storage and uses it for 50 minutes. It makes one sooner only when Apple refuses
  the token as expired or invalid and the token is at least 20 minutes old: Apple throttles a
  sender that makes them more often.

Apple's push key is the relay's one secret. Until it is set, with its key id and team id, push is
off: no computer is told the relay sends (`f`), and health says `push: false`. For each
environment the owner gives wrangler the key file from Apple, which keeps its line breaks:

```sh
corepack pnpm --filter @o7/relay exec wrangler secret put APNS_KEY --env preview < AuthKey_<key id>.p8
```

then sets `APNS_KEY_ID` and `APNS_TEAM_ID` for that environment in `wrangler.toml` and deploys it.
