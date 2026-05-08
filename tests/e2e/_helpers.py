from __future__ import annotations

import json
import time
from dataclasses import dataclass
from typing import Any

from logoscore import LogoscoreClient
from logos_integration_test_framework import subscribe

from _constants import CHAT_CLUSTER_ID, CHAT_SHARD_ID

MODULE = "chat_module"

# Grace before triggering, so `logoscore watch` subprocess is live —
# events fired in that window are lost.
SUBSCRIBE_GRACE_S = 0.4


def make_chat_config(name: str, port: int, bootstrap_enr: str) -> str:
    """Build the JSON config for `chat_module.initChat`.

    Cluster + shard must match the bootstrap nwaku's --preset/--shard, otherwise
    the pubsub topic /waku/2/rs/{cluster}/{shard} won't line up across containers.
    """
    return json.dumps({
        "name": name,
        "port": port,
        "clusterId": CHAT_CLUSTER_ID,
        "shardId": CHAT_SHARD_ID,
        "staticPeers": [bootstrap_enr],
    })


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
    Raises if the synchronous return is False (request rejected pre-send).
    """
    # subscribe(...) yields all events for the module, not just `event` —
    # CLI's watchModuleEvents callback only filters by module. Re-filter here.
    with subscribe(client, MODULE, event) as w:
        time.sleep(SUBSCRIBE_GRACE_S)
        ok = client.call(MODULE, method, *args)
        if ok is False:
            raise RuntimeError(
                f"{method}({args!r}) returned False — request rejected "
                f"(client likely not initialised)"
            )
        return w.next(predicate=lambda e: e.get("event") == event, timeout=timeout)


def wait_event(waiter: Any, event_name: str, *, timeout: float) -> dict[str, Any]:
    """Pick the next event whose name matches `event_name` (subscribe() doesn't filter)."""
    return waiter.next(predicate=lambda e: e.get("event") == event_name, timeout=timeout)


def event_arg(event: dict[str, Any], index: int) -> Any:
    """Read positional arg N from a watch-event payload.

    `data` is `{"arg0": ..., "arg1": ...}`, not a list — the CLI wraps the
    QVariantList tail of every event into a JSON object keyed by `argN`.
    """
    return event["data"][f"arg{index}"]


def assert_success(event: dict[str, Any], op: str) -> None:
    """For events whose `arg0` is a success bool."""
    if event_arg(event, 0) is not True:
        raise AssertionError(f"{op} failed: event={event!r}")


def parse_json_field(event: dict[str, Any], index: int) -> dict[str, Any]:
    """Pull a JSON-encoded string out of `arg{index}` and parse it."""
    return json.loads(event_arg(event, index))


def extract_convo_id(payload: dict[str, Any]) -> str:
    """Extract conversation id from either shape: `{"id": ...}` (Conversation
    object) or `{"conversationId": ..., ...}` (push event payload)."""
    return str(payload.get("id") or payload["conversationId"])


def extract_message_content(payload: dict[str, Any]) -> str:
    """Decode hex-encoded UTF-8 from a chatNewMessage `content` field."""
    return hex_decode(payload["content"])


@dataclass
class ChatUser:
    """One initialised, started chat client with its identity exposed.

    `installation_name` is the configJson `name` (e.g. "Saro", "Raya") returned
    by `chat_get_id` — NOT a libp2p peerId. Log correlation only, not routable.
    """

    client: LogoscoreClient
    installation_name: str
    intro_bundle: str
    label: str


def setup_chat_user(
    client: LogoscoreClient,
    *,
    config_json: str,
    label: str,
    init_timeout: float = 30.0,
) -> ChatUser:
    """Load chat_module, init, register event callback, start, capture identity.

    Sequence: load_module → initChat → setEventCallback → startChat,
    then getId + createIntroBundle.
    """
    client.load_module(MODULE)

    init_evt = call_and_wait(
        client, "initChat", config_json,
        event="chatInitResult", timeout=init_timeout,
    )
    assert_success(init_evt, f"initChat for {label}")

    # setEventCallback is synchronous; no result event.
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
    installation_name = str(event_arg(id_evt, 0))

    bundle_evt = call_and_wait(
        client, "createIntroBundle",
        event="chatCreateIntroBundleResult", timeout=10.0,
    )
    assert_success(bundle_evt, f"createIntroBundle for {label}")
    intro_bundle = str(event_arg(bundle_evt, 2))

    return ChatUser(
        client=client,
        installation_name=installation_name,
        intro_bundle=intro_bundle,
        label=label,
    )
