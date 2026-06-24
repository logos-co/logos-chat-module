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

## Events

The module pushes six events over the lp_* IPC event channel (LIDL `event`
declarations); consumers subscribe via `on_<event>()` — no polling. Each carries
positional arguments in `.lidl` order:

- **`message_received`** — an inbound message was decrypted
  - `convo_id` (`tstr`), `content` (`tstr`), `timestamp_ms` (`int`)
- **`message_sent`** — an outbound message was recorded
  - `convo_id` (`tstr`), `content` (`tstr`), `timestamp_ms` (`int`)
- **`conversation_created`** — a conversation was opened
  - `convo_id` (`tstr`), `is_outgoing` (`bool`), `peer_label` (`tstr`)
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
