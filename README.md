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

Two conversation shapes are exposed. `create_conversation(peer_address)` opens
a 1:1 DirectV1 conversation. `create_group_conversation(name, desc)` creates a
GroupV2 (de-mls) group with this installation as its only member, grown one
peer at a time with `add_group_member(convo_id, peer_address)`; every member
sees the same conversation id, and adds are committed by the group's steward
asynchronously, so a peer joins some time after the call returns. A group's
`name` and `desc` are shared metadata carried to every joiner, both optional.
`list_group_members(convo_id)` returns a group's roster from libchat's MLS
state. The `Conversation` record and the `conversation_created` event carry a
`kind` (`"direct"` or `"group"`) distinguishing the two shapes, plus a group's
shared `name` and `description` (unset for direct conversations and unnamed
groups). Received
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
at `v0.1.2`. Load `chat_module` via `logoscore` or Basecamp.

Bring-up is `init(instance_path, delivery_preset, tcp_port)` (empty preset →
`logos.dev`). `init` starts delivery asynchronously and returns immediately;
readiness arrives later as a `delivery_state_changed` event reaching `online`.

`init` also installs a `tracing` subscriber, so libchat's log events go to the
module's stderr and the host forwards them into its own log. The default level
is `warn`; `RUST_LOG`, read from the environment the module process inherits
from its host, selects more. libchat logs under two targets: `libchat` (the
conversation core, MLS groups, inbox) and `logos_generic_chat` (the threaded
client and its inbound worker), so a verbose run is
`RUST_LOG=warn,libchat=debug,logos_generic_chat=debug`.

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
