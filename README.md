# logos-chat-module

A [Logos Core](https://github.com/logos-co/logos-liblogos) module plugin that exposes the [Logos Chat](https://github.com/logos-messaging/logos-chat) to the Logos platform.

Loaded into Logos Core, it wraps `liblogoschat` and bridges its C callback API to Qt signals and invokable methods. Consumers interact with it entirely through the module methods and signals — no direct dependency on `liblogoschat` is required.

> [`logos-chatsdk-ui`](https://github.com/logos-co/logos-chatsdk-ui) is the reference UI built on top of this module.

## What It Provides

- **Identity** — query client ID and identity info, generate introduction bundles
- **Conversations** — list, retrieve, and open new private (1-to-1) conversations
- **Messaging** — send messages and receive push events for new messages, new conversations, and delivery acknowledgements
- **Lifecycle management** — initialise, start, stop, and destroy the chat client

## API

See [chat_module_plugin.h](chat_module_plugin.h) for the full API — methods, async event names, and per-event `data` layouts are documented there.

## How to Build

### Using Nix (recommended)

```bash
# Build everything (plugin + liblogoschat + generated headers)
nix build

# Build only the plugin library
nix build '.#lib'

# Enter the development shell
nix develop
```

> [!NOTE]
> If flakes aren't enabled globally, add `--extra-experimental-features 'nix-command flakes'`. \
> In zsh, quote the target to prevent glob expansion (e.g., `'.#lib'`).

### Using CMake

```bash
mkdir build && cd build
cmake .. -GNinja \
  -DLOGOS_CPP_SDK_ROOT=/path/to/logos-cpp-sdk \
  -DLOGOS_LIBLOGOS_ROOT=/path/to/logos-liblogos \
  -DLOGOS_CHAT_ROOT=/path/to/logos-chat
ninja
```

`LOGOS_CHAT_ROOT` must point to a `logos-chat` build output containing `include/liblogoschat.h` and `lib/liblogoschat.{dylib,so}`.

If `LOGOS_CPP_SDK_ROOT` and `LOGOS_LIBLOGOS_ROOT` are not set, CMake looks for sibling directories (`../logos-cpp-sdk`, `../logos-liblogos`) and falls back to `vendor/` if those don't exist.

## Output Structure

```
result/
├── lib/
│   ├── chat_module_plugin.dylib   # Module plugin (.so on Linux)
│   └── liblogoschat.dylib            # Chat library dependency (.so on Linux)
└── include/
    ├── chat_module_api.h          # Generated C++ API header
    └── chat_module_api.cpp        # Generated C++ API implementation
```

Both libraries must be in the same directory — the plugin uses `@loader_path` / `$ORIGIN` to locate `liblogoschat` at runtime.

## Requirements

> [!TIP]
> When using Nix, all requirements are acquired automatically.

### Build tools

- CMake ≥ 3.14
- Ninja
- pkg-config

### Dependencies

| Dependency | Purpose |
|---|---|
| Qt6 Core | Qt plugin infrastructure |
| Qt6 RemoteObjects | LogosAPI IPC transport |
| [`logos-cpp-sdk`](https://github.com/logos-co/logos-cpp-sdk) | LogosAPI bindings, C++ generator |
| [`logos-liblogos`](https://github.com/logos-co/logos-liblogos) | Logos Core plugin interface |
| [`logos-chat`](https://github.com/logos-messaging/logos-chat) | Provides `liblogoschat` |

## Related Repositories

| Repository | Role |
|---|---|
| [`logos-chatsdk-ui`](https://github.com/logos-co/logos-chatsdk-ui) | Reference Qt UI built on this module |
| [`logos-chat`](https://github.com/logos-messaging/logos-chat) | Logos Chat application (provides `liblogoschat`) |
| [`logos-liblogos`](https://github.com/logos-co/logos-liblogos) | Logos Core platform |
| [`logos-cpp-sdk`](https://github.com/logos-co/logos-cpp-sdk) | LogosAPI and C++ module bindings generator |
