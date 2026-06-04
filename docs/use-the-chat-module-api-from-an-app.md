# Use the Logos Chat module API from an app

> Tracks [logos-docs#141](https://github.com/logos-co/logos-docs/issues/141). This doc targets `logos-chat-module` **`v0.1.0`**.

## 1. Outcome and purpose

- **What the user achieves:** A developer builds a Logos module that calls the Logos Chat module API to exchange introduction bundles, open private 1:1 conversations, and send/receive end-to-end encrypted messages on the Logos network.
- **Why it matters:** Proves the Logos Chat module is functioning and gives application developers a working pattern for integrating private 1:1 e2ee messaging into their own modules — without direct dependencies neither on `liblogoschat` nor `logos-delivery`.
- **Key components:**
  - `logos-chat-module` — the Logos module exposing the chat API as invokable methods + async events. This doc targets tag `v0.1.0`.
  - `logos-chat` — underlying `liblogoschat` implementation, a transitive dependency resolved automatically by Nix and bundled alongside the plugin.
  - `logos-chat-ui` — the reference UI module that follows this exact pattern end-to-end 

## 2. Scope

- **Repositories:**
  - https://github.com/logos-co/logos-chat-module @ v0.1.0
- **Runtime target:** Testnet v0.1 (the `logos.dev` network — Waku `clusterId 2` / `shardId 1`)
- **Prerequisites:**
  - MacOS (aarch64 or x86_64) or Linux (aarch64 or x86_64)
  - Network access so the two instances can reach each other
  - Nix with flakes enabled

## 3. Happy path

### Step 1: Create a Logos module

Scaffold a new module using [logos-module-builder](https://github.com/logos-co/logos-module-builder). For a full walkthrough, see the [Logos module developer guide](https://github.com/logos-co/logos-tutorial/blob/master/logos-developer-guide.md). For a complete worked example that follows this pattern, see [`logos-chat-ui`](https://github.com/logos-co/logos-chat-ui).

### Step 2: Declare `chat_module` as a dependency

In `metadata.json`:

```json
{
  "name": "my_app",
  "dependencies": ["chat_module"],
  ...
}
```

In `flake.nix`, add a matching input. **Pin to the released tag** so the doc and your app remain stable as the module's API evolves:

```nix
inputs = {
  logos-module-builder.url = "github:logos-co/logos-module-builder";
  chat_module.url = "github:logos-co/logos-chat-module/v0.1.0";
};
```

> The flake input name (`chat_module`) must match the dependency name in `metadata.json`. `logos-module-builder` automatically generates the typed `chat_module` wrapper at build time.

### Step 3: Call the chat module API

> [!TIP]
> For the full API reference — every method, async event name, and per-event `data` layout — see [`src/chat_module_plugin.h`](https://github.com/logos-co/logos-chat-module/blob/v0.1.0/src/chat_module_plugin.h).

In your module's `initLogos()`, construct `LogosModules` with the provided `LogosAPI*`. `LogosModules` is generated at build time by `logos-module-builder`; pull it in via the umbrella header and keep it on the plugin as a member.

```cpp
#include "logos_sdk.h"   // generated umbrella — exposes LogosModules

// In your plugin class:
//   LogosModules* m_logos = nullptr;

void MyPlugin::initLogos(LogosAPI* api) {
    m_logos = new LogosModules(api);
    // m_logos->chat_module is now the typed wrapper for the Logos Chat module.
}
```

Unlike a synchronous API, **every chat method returns `bool` immediately** — `true` means the request was accepted, `false` means it was rejected before being sent (typically because the client is not initialised yet). The actual result arrives later as a named event. Drive the module through the following sequence.

> [!NOTE]
> The `bool` / `QString` / `.on(name, QVariantList)` surface shown here is the **Qt-typed wrapper** that `logos-module-builder` generates for a *legacy* module like `chat_module` v0.1.0 (its `metadata.json` has no `"interface"` field, so it defaults to `legacy`). This is what you get when your own module is the default (legacy) interface. If you build your module as `"interface": "universal"`, the generator emits std-typed wrappers instead (`std::string` args, `LogosResult` returns) — adjust the call sites accordingly.

#### 1. Register event handlers (before starting)

Wire your handlers **before** `startChat()` so you don't miss early results or the first incoming messages. Handlers receive a `QVariantList`; for result events the entries are the JSON fields in declared order, and for push events `data[0]` is the JSON payload string.

```cpp
// Lifecycle + request results
m_logos->chat_module.on("chatInitResult",  [](const QVariantList& data) {
    // data[0]: bool success, data[1]: int statusCode, data[2]: QString message, data[3]: timestamp
});
m_logos->chat_module.on("chatStartResult", [](const QVariantList& data) { /* success, statusCode, message, ts */ });
m_logos->chat_module.on("chatCreateIntroBundleResult", [](const QVariantList& data) {
    // data[0]: bool success, data[2]: QString introBundle
});
m_logos->chat_module.on("chatNewPrivateConversationResult", [](const QVariantList& data) {
    // data[0]: bool success, data[2]: QString conversation (JSON object with the conversation id)
});
m_logos->chat_module.on("chatSendMessageResult", [](const QVariantList& data) { /* success, statusCode, result, ts */ });

// Push events (delivered after setEventCallback())
m_logos->chat_module.on("chatNewConversation", [](const QVariantList& data) {
    // data[0]: QString — JSON payload describing the new conversation
});
m_logos->chat_module.on("chatNewMessage", [](const QVariantList& data) {
    // data[0]: QString — JSON payload with conversationId, sender, content (hex-encoded)
});
m_logos->chat_module.on("chatDeliveryAck", [](const QVariantList& data) { /* JSON payload */ });
```

See `src/chat_module_plugin.h` for the exact field layout of every event.

#### 2. Initialize the client

`initChat` takes a JSON config string (see §5). Check the return value:

```cpp
const QString cfg = R"({"name":"alice","clusterId":2,"shardId":1})";
if (!m_logos->chat_module.initChat(cfg)) {
    qWarning() << "initChat rejected — config invalid";
    return;
}
// wait for chatInitResult (success == true) before continuing
```

#### 3. Subscribe to push events, then start

Call `setEventCallback()` after `initChat` and before `startChat()` so no incoming messages are missed:

```cpp
m_logos->chat_module.setEventCallback();
m_logos->chat_module.startChat();
// wait for chatStartResult (success == true) — the client is now connected
```

#### 4. Create and share your introduction bundle

```cpp
m_logos->chat_module.createIntroBundle();
// chatCreateIntroBundleResult delivers the bundle string — share it out-of-band.
```

#### 5. Open a private conversation

The initiator calls `newPrivateConversation` with the *other* party's intro bundle and a hex-encoded opening message. The recipient instead receives a `chatNewConversation` push event.

```cpp
const QString contentHex = toHex("Hello!");               // content must be hex-encoded
m_logos->chat_module.newPrivateConversation(peerBundle, contentHex);
// chatNewPrivateConversationResult carries the new conversation id.
```

#### 6. Send and receive messages

```cpp
m_logos->chat_module.sendMessage(conversationId, toHex("How are you?"));
// chatSendMessageResult confirms acceptance; the peer gets a chatNewMessage push event.
```

#### 7. Clean shutdown

```cpp
m_logos->chat_module.stopChat();   // disconnects and tears the client down
```

### Step 4: Build and run

Build your module with Nix:

```sh
nix build              # build the module
nix run                # preview via logos-standalone-app (for ui_qml modules)
nix build .#lgx        # package as .lgx for installation into logos-basecamp
```

## 4. Verification

- `initChat()` returns `true`; `chatInitResult` fires with `success == true`.
- `chatStartResult` fires with `success == true` (the client is connected).
- `createIntroBundle()` → `chatCreateIntroBundleResult` carries a non-empty `introBundle`.
- `newPrivateConversation()` → `chatNewPrivateConversationResult` with `success == true` and a conversation id.
- `sendMessage()` → `chatSendMessageResult` with `success == true`.
- On the recipient side, `chatNewConversation` then `chatNewMessage` fire on the new conversation.

## 5. Configuration

`initChat` takes a flat JSON object (as a string) consumed by `liblogoschat`. Minimal working config for the `logos.dev` network:

```json
{
  "name": "alice",
  "clusterId": 2,
  "shardId": 1
}
```

| Field         | Type     | Notes                                                               |
| ------------- | -------- | ------------------------------------------------------------------- |
| `name`        | string   | Identity name. `getId()` returns this string                        |
| `port`        | int      | Logos Delivery (Waku) TCP port. `0` or omitted picks a random port. |
| `clusterId`   | int      | **Must be `2`** to reach the `logos.dev` network.                   |
| `shardId`     | int      | **Must be `1`** to reach the `logos.dev` network.                   |
| `staticPeers` | string[] | Optional bootstrap peer multiaddrs.                                 |

The pubsub topic is derived from `clusterId`/`shardId`, so they must match across all participants or messages won't propagate. For the config builder used by the reference UI, see [`ChatConfig.h`](https://github.com/logos-co/logos-chat-ui/blob/v0.1.0/src/ChatConfig.h).

## 6. Known issues and troubleshooting

1. **A method returns `false`**
   - Cause: the client is not initialised (or, for `startChat`, `initChat` has not completed).
   - Fix: call `initChat()` first and wait for `chatInitResult` with `success == true` before calling anything else.

2. **Peers don't connect / messages don't propagate**
   - Cause: mismatched `clusterId`/`shardId` (must be `2`/`1` for `logos.dev`).
   - Fix: use `clusterId: 2`, `shardId: 1`, and supply a reachable bootstrap peer if your network needs one.

3. **Message content arrives garbled or empty**
   - Cause: `newPrivateConversation` and `sendMessage` take **hex-encoded** content; passing raw UTF-8 produces wrong bytes on the wire.
   - Fix: hex-encode outgoing content and hex-decode the `content` field of `chatNewMessage`.

**Out of scope for this doc:**

- Message/identity persistence (state is ephemeral — see §7).
- Group conversations (only private 1:1 conversations are supported).
- Running a self-hosted delivery backend or custom bootstrap configuration.

## 7. Additional context

- **Complete example:** [`logos-chat-ui`](https://github.com/logos-co/logos-chat-ui) — the reference Qt/QML module. Its `ChatBackend` constructs `LogosModules`, registers the same `on(...)` handlers, and drives `initChat → setEventCallback → startChat → createIntroBundle → newPrivateConversation → sendMessage` exactly as above.
- **Full API reference:** [`src/chat_module_plugin.h`](https://github.com/logos-co/logos-chat-module/blob/v0.1.0/src/chat_module_plugin.h) contains Doxygen documentation for every method and event contract, including which calls are not guaranteed to emit a result.
- **Module development guide:** [`logos-developer-guide.md`](https://github.com/logos-co/logos-tutorial/blob/master/logos-developer-guide.md) covers scaffolding, inter-module communication, and the generated wrappers.

- **Hardware requirements:** Standard developer machine. No special hardware required. Minimum ~1 GB RAM for the node process.

- **Security notes:** Identity and conversations are **ephemeral** — they exist only for the lifetime of the client and are lost on `stopChat()`/restart.

## References

- `logos-chat-module` (this doc targets [`v0.1.0`](https://github.com/logos-co/logos-chat-module/tree/v0.1.0)): https://github.com/logos-co/logos-chat-module
- `logos-chat-ui` (reference UI / worked example): https://github.com/logos-co/logos-chat-ui
- `logos-module-builder` (build system + scaffolding): https://github.com/logos-co/logos-module-builder
- `logos-tutorial` (module development walkthrough): https://github.com/logos-co/logos-tutorial
- `logos-chat` (underlying `liblogoschat` implementation): https://github.com/logos-messaging/logos-chat
