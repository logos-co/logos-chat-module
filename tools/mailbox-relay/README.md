# mailbox-relay

A centralized, reliable, inspectable transport for testing logos-chat across
machines in different locations. It replaces only the delivery layer: clients
POST sealed envelopes to per-topic logs and long-poll for anything past their
cursor. MLS, the key-package registry, and everything above the transport seam
are untouched, so a run over the relay still exercises the real app.

The relay sees **metadata only** — topics (delivery addresses), sizes, and
timing. Payloads are opaque MLS ciphertext.

## Run locally

```sh
RELAY_TOKEN=devtoken cargo run --release
# open relay (no auth), e.g. for a quick local two-instance test:
RELAY_NO_AUTH=1 cargo run --release
```

Config (env):

| Var | Default | Meaning |
|---|---|---|
| `RELAY_BIND` | `0.0.0.0:8080` | listen address |
| `RELAY_TOKEN` | (required) | Basic-auth password; startup fails without it unless `RELAY_NO_AUTH=1` |
| `RELAY_NO_AUTH` | unset | `1` disables auth entirely |
| `RELAY_DB` | `./relay.db` | sqlite file (survives restarts) |
| `RELAY_RETENTION_DAYS` | `7` | messages older than this are swept hourly (`0` disables) |
| `RUST_LOG` | `info` | tracing filter; `debug` logs every publish with topic+seq |

## Wire API

All routes except `/healthz` require `Authorization: Basic base64(user:token)`
(the username is ignored; the token is the password). Bodies are JSON; envelope
bytes are base64url (no padding).

- `POST /v1/messages?topic=<topic>` — body `{"data":"<base64url>"}` →
  `201 {"seq":n,"ts_ms":m}`
- `POST /v1/poll` — body `{"topics":{"<topic>":<after_seq>},"wait_secs":25}` →
  `{"messages":[{"topic","seq","ts_ms","data"}]}`. Returns immediately if
  anything is past a cursor, else long-polls up to `wait_secs` (max 30).
- `GET /v1/topics` → `[{"topic","count","last_seq","last_ts_ms","bytes"}]`
- `GET /v1/messages?topic=T&after=N&limit=100` → same message shape (debug tail)
- `DELETE /v1/messages?topic=T` → `{"deleted":n}` (reset between test runs)
- `GET /healthz` → `{"ok":true,"uptime_s":n}` (no auth)

Ordering is per-topic by `seq`; a poller advancing its cursor never misses or
duplicates a message. Cross-topic order is not guaranteed.

## Debugging cookbook

```sh
TOKEN=devtoken; RELAY=https://relay.example.com
# which topics exist, counts, last activity
curl -s -u x:$TOKEN $RELAY/v1/topics | jq
# tail one inbox topic (metadata only; data is ciphertext)
curl -s -u x:$TOKEN "$RELAY/v1/messages?topic=/logos-chat/1/<addr>/proto&after=0" \
  | jq '.messages[] | {seq, ts_ms, size: (.data | length)}'
# inject a probe onto a topic (the subscriber will fetch and fail to decode it,
# proving the transport path end-to-end without valid ciphertext)
curl -s -u x:$TOKEN -X POST "$RELAY/v1/messages?topic=/logos-chat/1/<addr>/proto" \
  -H 'content-type: application/json' -d '{"data":"aGVsbG8"}'
# reset a topic
curl -s -u x:$TOKEN -X DELETE "$RELAY/v1/messages?topic=/logos-chat/1/<addr>/proto"
```

"A sends, B sees nothing" isolates fast: check `/v1/topics` — did A's publish
land (count grew on B's inbox topic)? If no, it is A-side (sender-thread logs);
if yes, check the server log for B's `/v1/poll` advancing past that seq. If B
polls past it and still shows nothing, the problem is above the transport.

## Deploy

Host it wherever you like: fly.io, DigitalOcean, a VPS, any container platform
(a Dockerfile is included). It's one binary plus a sqlite file, so set
`RELAY_TOKEN`, keep `RELAY_DB` on a persistent volume, and expose `:8080` behind
TLS.

One concrete example is a cloudflared quick tunnel, handy for a throwaway public
URL:

```sh
RELAY_TOKEN=devtoken cargo run --release &      # or the built binary
cloudflared tunnel --url http://localhost:8080  # prints an https URL
```

Free, no account, automatic TLS. The URL is ephemeral; use a named tunnel plus
your own domain for a stable one.
