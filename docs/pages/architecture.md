# Architecture

`chat_module` is a Logos Core module: a `cdylib` plugin that `liblogos_core`
loads, declared in [`metadata.json`](https://github.com/logos-co/logos-chat-module/blob/master/metadata.json)
as `type: core`, `category: chat`, with `delivery_module` as a runtime
dependency.

## Where it sits

```
logos-chat-ui  (or any other consumer)
      │  generated client, over the Logos IPC bus
      ▼
chat_module          ← this repository
      │  libchat (logos-generic-chat): conversations, MLS groups, inbox
      │  delivery_module: publish/subscribe over the network
      ▼
the delivery network
```

The module owns no cryptography and no transport of its own. It wraps
[libchat](https://github.com/logos-messaging/libchat) for the chat protocol and
calls `delivery_module` for the network, and its job is to expose both to the
rest of the runtime as one contract.

It depends on the transport-generic `logos-generic-chat` crate rather than the
`logos-chat` facade, because that facade bundles an embedded delivery and this
module brings its own — `logoscore`'s `delivery_module`. The flake pins
[`logos-delivery-module`](https://github.com/logos-co/logos-delivery-module) at
`v0.2.0` and re-exports the matching `.lgx`, so the delivery build a
`chat_module` release was tested against is the one shipped with it.

## From contract to plugin

The contract is
[`rust-lib/chat_module.lidl`](https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/chat_module.lidl).
Nothing is hand-written twice from it:

1. `metadata.json#codegen` points `logos-lidl-gen` at the contract. It emits the
   module-impl C ABI scaffold — the `ChatModule` trait, dispatch, the `emit_*`
   event emitters, and the `logos_module_*` exports — into
   `rust-lib/generated/provider_gen.rs`.
2. `rust-lib/src/lib.rs` `include!`s that scaffold and implements the trait. The
   crate builds as a `staticlib`. There is no `build.rs`.
3. `logos-module-builder` generates the matching Qt plugin glue, and
   `CMakeLists.txt` links the Rust archive into it.
4. Consumers run the same generator over the same `.lidl` in the other
   direction, producing a typed client. `logos-chat-ui` is one such consumer.

The generated scaffold and the staged SDK source tree are not committed; `nix
run .#generate` materialises both into the working tree for a bare `cargo
build`.

The module's own dependency on `delivery_module` is generated the same way,
from [`rust-lib/deps/delivery_module.lidl`](https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/deps/delivery_module.lidl)
— the subset of the delivery contract this module uses, supplied as a
`dependency_override`. It is hand-maintained, so it has to be kept in step with
the pinned delivery revision.

## Threading

State is split across two locks. The client lock guards the libchat
`ChatClient` and is held across cryptographic work; the display lock guards the
conversation history, so the read methods never wait on the slow client work.
The API is safe to call from any thread, and `init` imposes no thread affinity:
the SDK makes calls into `delivery_module` — event subscription and publish —
thread-safe regardless of the calling thread.

Events reach consumers over the `lp_*` IPC channel. The module calls the
generated `emit_*` functions, which the host marshals onto the Qt thread and
republishes as IPC events.

The crate builds with `panic = "abort"`: panicking across the FFI boundary is
undefined behaviour, and `safer-ffi` (transitive via libchat) requires abort.
That is also why a panic cannot be reported as a `tracing` event — see
[Logging](logging.md).

## Identity storage

The identity store opened by `init` is keyed from the host-assigned instance
persistence path: obfuscated, not protected. Passphrase UX is planned.
