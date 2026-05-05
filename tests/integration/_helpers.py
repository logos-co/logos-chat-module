"""Helpers for the chat-module two-user integration test.

`chat_module` exposes async methods that return `bool` synchronously and emit
the actual result through `eventResponse(<eventName>, [...])`. The pattern
"open subscription before calling, wait for the result event" repeats so much
that it pays to have it in one place.

Schemas come from `chat_module_plugin.h` and the underlying liblogoschat
library. Specifically:

- Conversation object (returned by `chatGetConversationResult`,
  `chatListConversationsResult`): `{"id": "..."}`. Source:
  `library/api/client_api.nim:138-145` in
  `logos-messaging/logos-chat@53302e4373755b72391727de3d5d2b30e1239dbb`.
- Push event payloads (`chatNewConversation`, `chatNewMessage`,
  `chatDeliveryAck`): `{"eventType": ..., "conversationId": ..., ...}`.
  Source: `library/utils.nim:24-75` (same SHA), types `JsonMessageEvent`,
  `JsonConversationEvent`, `JsonDeliveryAckEvent`.
- Message content is hex-encoded UTF-8 (`JsonMessageEvent.content`).
"""

from __future__ import annotations

import json
import time
from dataclasses import dataclass
from typing import Any

from logoscore import LogoscoreClient
from logos_integration_test_framework import subscribe

MODULE = "chat_module"  # note: underscore (per metadata.json)

# Watch-subprocess startup grace. Upstream `logoscore watch` takes a beat to
# come live; events fired in that window are lost. The framework's smoke uses
# the same idiom.
SUBSCRIBE_GRACE_S = 0.4


def hex_encode(s: str) -> str:
    """`chat_module.sendMessage` and `newPrivateConversation` take hex bytes."""
    return s.encode("utf-8").hex()


def hex_decode(h: str) -> str:
    return bytes.fromhex(h).decode("utf-8")


def call_and_wait(
    client: LogoscoreClient,
    method: str,
    *args: Any,
    event: str,
    timeout: float = 15.0,
) -> dict[str, Any]:
    """Call an async chat method; wait for its result event on the same client.

    Subscribes BEFORE calling so the result event isn't missed if it fires fast.
    Returns the raw event dict ({"event": ..., "data": [...]}).
    Raises if the synchronous return is False (request rejected pre-send).
    """
    with subscribe(client, MODULE, event) as w:
        time.sleep(SUBSCRIBE_GRACE_S)
        ok = client.call(MODULE, method, *args)
        if ok is False:
            raise RuntimeError(
                f"{method}({args!r}) returned False — request rejected "
                f"(client likely not initialised)"
            )
        return w.next(timeout=timeout)


def assert_success(event: dict[str, Any], op: str) -> None:
    """For events whose `data[0]` is a success bool (most lifecycle / op results)."""
    data = event["data"]
    if not data or data[0] is not True:
        raise AssertionError(f"{op} failed: event={event!r}")


def parse_json_field(event: dict[str, Any], index: int) -> dict[str, Any]:
    """Pull a JSON-encoded string out of `event['data'][index]` and parse it."""
    raw = event["data"][index]
    parsed: Any = json.loads(raw)
    if not isinstance(parsed, dict):
        raise AssertionError(
            f"expected dict at data[{index}], got {type(parsed).__name__}: {raw!r}"
        )
    return parsed


def extract_convo_id(payload: dict[str, Any]) -> str:
    """Extract conversation id from either a Conversation object or a push event.

    Two legitimate shapes:
    - Conversation object (`chatGetConversationResult.data[0]`,
      `chatListConversationsResult.data[0]`): ``{"id": "..."}``
    - Push event payload (`chatNewConversation.data[0]`,
      `chatNewMessage.data[0]`): ``{"conversationId": "...", ...}``

    Both keys are accepted by design — they identify the same logical convo
    coming from different code paths.
    """
    if "id" in payload:
        return str(payload["id"])
    if "conversationId" in payload:
        return str(payload["conversationId"])
    raise AssertionError(
        f"could not find conversation id in {payload!r}; "
        "expected 'id' (Conversation object) or 'conversationId' (push event)"
    )


def extract_message_content(payload: dict[str, Any]) -> str:
    """Decode the hex-encoded UTF-8 message content from a chatNewMessage payload.

    The `content` field is always hex (`JsonMessageEvent.content` in
    `library/utils.nim`).
    """
    return hex_decode(payload["content"])


@dataclass
class ChatUser:
    """One initialised, started chat client with its identity exposed.

    `installation_name` is what `chat_get_id` returns — the `name` field from
    configJson (e.g. "Alice", "Bob"), NOT a libp2p peerId. See
    `src/chat/client.nim:99-100` in logos-messaging/logos-chat: ``getId() =
    libchatCtx.getInstallationName()``. Useful for log correlation; do not
    treat as a peerId or routing handle.
    """

    client: LogoscoreClient
    installation_name: str
    intro_bundle: str
    label: str  # "A" / "B" — used in error messages


def setup_chat_user(
    client: LogoscoreClient,
    *,
    config_json: str,
    label: str,
    init_timeout: float = 30.0,
) -> ChatUser:
    """Load chat_module, init, register event callback, start, capture identity.

    Sequence per chat_module_plugin.h docstrings:
      load_module → initChat → setEventCallback → startChat
    Then: getId, createIntroBundle to capture the identity for routing.

    `chat_module` declares no dependencies in metadata.json (`dependencies=[]`)
    and POC confirmed liblogoschat pulls its waku-stack runtime as a C-library,
    so no pre-loading of other Logos modules is needed.
    """
    client.load_module(MODULE)

    init_evt = call_and_wait(
        client, "initChat", config_json,
        event="chatInitResult", timeout=init_timeout,
    )
    assert_success(init_evt, f"initChat for {label}")

    # `setEventCallback` is synchronous and does not emit a result event.
    if client.call(MODULE, "setEventCallback") is False:
        raise RuntimeError(f"setEventCallback failed for {label}")

    start_evt = call_and_wait(
        client, "startChat",
        event="chatStartResult", timeout=init_timeout,
    )
    assert_success(start_evt, f"startChat for {label}")

    id_evt = call_and_wait(
        client, "getId",
        event="chatGetIdResult", timeout=10.0,
    )
    installation_name = str(id_evt["data"][0])

    bundle_evt = call_and_wait(
        client, "createIntroBundle",
        event="chatCreateIntroBundleResult", timeout=10.0,
    )
    assert_success(bundle_evt, f"createIntroBundle for {label}")
    intro_bundle = str(bundle_evt["data"][2])

    return ChatUser(
        client=client,
        installation_name=installation_name,
        intro_bundle=intro_bundle,
        label=label,
    )
