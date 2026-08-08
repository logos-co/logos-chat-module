# logos-chat-module

A Rust [Logos Module](https://github.com/logos-co/logos-liblogos) that wraps
[libchat](https://github.com/logos-messaging/libchat) and exposes
e2e-encrypted chat over the Logos IPC bus. Loaded as a `cdylib` module by
`liblogos_core`; depends on `delivery_module` at runtime (declared in
`metadata.json`).

A companion QML UI App lives in
[`logos-chat-ui`](https://github.com/logos-co/logos-chat-ui).

## Build

```bash
nix build .#chat_module    # the full Qt plugin
```

`nix build` is the entry point and needs no manual hash bookkeeping:
`logos-module-builder` runs `logos-lidl-gen` to emit the module-impl scaffold,
fetches the Cargo deps recorded in `rust-lib/Cargo.lock`, and compiles the
staticlib. Bumping the `libchat` pin is just `cargo metadata` (or `cargo update
-p`) to refresh `rust-lib/Cargo.lock`; the next `nix build` picks it up.

For a bare `cargo build`, first run `nix run .#generate`. It materialises the two
gitignored inputs `rust-lib/` references into the working tree: the SDK source
tree (`logos-rust-sdk-src/`) and the generated scaffold (`rust-lib/generated/`),
both from the rev the builder pins. Then cargo works in `rust-lib/` directly:

```bash
nix run .#generate                                          # stage SDK source + scaffold
cargo build --release --manifest-path rust-lib/Cargo.toml   # Rust staticlib only
```

`cargo` requires `pkg-config`, `perl`, and a C toolchain — `libchat`'s
storage/crypto stack pulls in `openssl-src`, which compiles OpenSSL from source.

## API

The contract consumers call is [`rust-lib/chat_module.lidl`](rust-lib/chat_module.lidl)
(`interface: cdylib`) — the single source of truth. `metadata.json#codegen`
drives `logos-lidl-gen` to generate the module-impl C ABI scaffold (the
`ChatModule` trait, dispatch, the `emit_*` event emitters, and the
`logos_module_*` exports) into `rust-lib/generated/provider_gen.rs`, which
`src/lib.rs` `include!`s and implements; `logos-module-builder` generates the
matching Qt-plugin glue. There is no `build.rs`.

Status-bearing methods return `result`: `Ok(value)` carries any payload (a
conversation id, an intro bundle, or null), `Err(message)` a human-readable
reason. Collection getters (`list_conversations`, `get_messages`) return JSON
arrays. See the `.lidl` for the full method list and record shapes.

`health()` is the exception: it returns `true` and nothing else, needs no `init`,
and holds no lock, so what a caller learns is whether the call arrived at all. It
exists because a module that dies takes no part in noticing. Nothing is pushed
when the process goes, and a consumer otherwise finds out only when the next
thing it does runs out its own timeout, having looked connected until then.
Polling this turns that into a bounded delay, and a failed call is the answer.

Two conversation shapes are exposed. `create_conversation(peer_address)` opens
a 1:1 DirectV1 conversation. `create_group_conversation(name, desc)` creates a
GroupV2 (de-mls) group with this installation as its only member, grown one
peer at a time with `add_group_member(convo_id, peer_address)`; every member
sees the same conversation id, and adds are committed by the group's steward
asynchronously, so a peer joins some time after the call returns. A group's
`name` and `desc` are shared metadata carried to every joiner, both optional.
`list_group_members(convo_id)` returns a conversation's roster from libchat's
MLS state, including invites this instance has sent that the group has not
committed yet, flagged `pending`; a direct conversation reports both
participants and never a pending one. The `Conversation` record and the
`conversation_created` event carry a `kind` (`"direct"` or `"group"`)
distinguishing the two shapes, plus a group's shared `name` and `description`
(unset for direct conversations and unnamed groups). Received
messages carry a `sender` (on the `Message` record and the `message_received`
event): the sender's directory-verified account address, or its device id
when the sender claims no account.

## Events

The module pushes six events over the lp_* IPC event channel (LIDL `event`
declarations); consumers subscribe via `on_<event>()` — no polling. Each carries
positional arguments in `.lidl` order:

- **`message_received`** — an inbound message was decrypted
  - `convo_id` (`tstr`), `content` (`tstr`), `timestamp_ms` (`int`), `sender` (`tstr`)
- **`message_sent`** — an outbound message was recorded
  - `convo_id` (`tstr`), `content` (`tstr`), `timestamp_ms` (`int`)
- **`conversation_created`** — a conversation was opened
  - `convo_id` (`tstr`), `is_outgoing` (`bool`), `peer_label` (`tstr`), `kind` (`tstr`), `name` (`tstr`), `desc` (`tstr`)
- **`conversation_updated`** — a conversation's metadata changed
  - `convo_id` (`tstr`)
- **`conversation_deleted`** — a conversation was removed
  - `convo_id` (`tstr`)
- **`delivery_state_changed`** — network/transport state changed
  - `delivery_state` (`tstr`), `detail` (`tstr`)

## Runtime

End-to-end chat needs a `delivery_module` available to the host at runtime; the
flake pins [`logos-delivery-module`](https://github.com/logos-co/logos-delivery-module)
at `v0.1.3`. Load `chat_module` via `logoscore` or Basecamp.

Bring-up is `init(config)`, taking a `ChatConfig` record whose every field is
optional: `delivery_preset` (empty or absent → `logos.test`) and `log_level`.
`init` starts delivery asynchronously and returns immediately; readiness arrives
later as a `delivery_state_changed` event reaching `online`. State is written to
the instance directory the host assigns, so running two instances side by side
is a matter of giving each host its own session directory (`--config-dir` under
`logoscore`); `init` fails when the host assigned no such directory. The delivery
node listens on ports it picks itself, so instances need no port coordination.

A generated client passes the record itself. `logoscore call` cannot — it coerces
an argument to a bool, a number or a string, never to an object — so from the CLI
pass the record's JSON text and the module reads it back:

```bash
logoscore call chat_module init '{"delivery_preset":"logos.test","log_level":"debug"}'
```

### Logging

`init` installs a `tracing` subscriber writing to two places: the module's
stderr, which the host forwards into its own log, and a file in the instance
directory, which `get_log_path()` names so a consumer can hand the run over
afterwards. Three targets carry the chat core's account of a run: `libchat` (the
conversation core, MLS groups, inbox), `logos_generic_chat` (the threaded client
and its inbound worker), and `chat_module` itself. The module is one of them
because the other two are nearly silent: between them they raise eleven events,
almost all on paths that are already failing, so a run that merely behaves oddly
would write nothing. This module reports its own lifecycle instead, and logs a
message as a byte count and a conversation id, never as content.

`log_level` sets those three targets and nothing else (`error`, `warn`, `info`,
`debug` or `trace`, defaulting to `info`), leaving everything around them at
`warn`, because the crates underneath the chat core have an order of magnitude
more `info` sites than it does. `RUST_LOG`, read from the environment the module
process inherits from its host, outranks the client's choice and replaces the
composition outright, so a verbose run names every target it wants:
`RUST_LOG=warn,chat_module=debug,libchat=debug,logos_generic_chat=debug`. The
level is read once, at the first `init`, and a later `init` leaves it as it was.

A panic goes into that file too, with a backtrace. It cannot arrive as a
`tracing` event, since `panic = "abort"` means the process is already on its way
down, so the panic hook writes it directly and captures the backtrace whether or
not `RUST_BACKTRACE` was set.

The file is `chat_module_<stamp>.log`, moved aside as
`chat_module_<stamp>.NNN.log` when it fills and reopened under the announced
name, with the ten most recent runs kept. That naming is what lets a consumer
group the directory into runs — and lets a second writer keep its own log
alongside without either knowing about the other, since the grouping reads the
stem off the announced path.

A line on stderr is `<SEVERITY>: <target>: <message>`, with `WARNING` for a
warning because that is the token a host ranks the line by, and with no timestamp
because the host stamps what it re-emits. A reader downstream gets the level from
the host's own column and the domain from the target. The same line in the file
leads with an ISO-8601 time, because nothing else stamps it and reading it
against another writer's log takes one.

Only the stderr half meets that host classifier, which is why `debug` and `trace`
are worth asking for: a host drops what it ranks below its own level, and the
file is written directly and drops nothing.

## Doc-tests

The specs under [`doctests/`](doctests/) are executable usage tutorials: each
loads `chat_module` into headless
[`logoscore`](https://github.com/logos-co/logos-logoscore-cli) daemons and
drives a real, end-to-end-encrypted exchange between them over the live
delivery network, documenting the module's API by example.
[`chat-module-exchange.test.yaml`](doctests/chat-module-exchange.test.yaml) is
the two-instance 1:1 round-trip;
[`chat-module-group.test.yaml`](doctests/chat-module-group.test.yaml) runs a
three-instance GroupV2 conversation (create, grow member by member, fan-out
messages with sender attribution). They run on every PR via
[`.github/workflows/doctests.yml`](.github/workflows/doctests.yml) (the
[shared doctest CLI](https://github.com/logos-co/logos-doctest) builds the
commit under test), which also makes them an integration check. Run one locally
against latest master (add `--release-for logos-chat-module=<branch-or-sha>` to
pin it to a pushed commit instead):

```bash
nix run github:logos-co/logos-doctest -- run doctests/chat-module-exchange.test.yaml
```
