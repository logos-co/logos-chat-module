from __future__ import annotations

import json
import time
from dataclasses import dataclass
from typing import Any

from logoscore import LogoscoreClient
from logos_integration_test_framework import subscribe

from libs.constants import CHAT_CLUSTER_ID, CHAT_SHARD_ID

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


def parse_event(event: dict[str, Any]) -> dict[str, Any]:
    """Parse the JSON payload at `arg0` into the named-fields dict the plugin emits.

    Universal codegen wraps the C++ side's single QString-payload signal into a
    one-arg event; CLI's watch puts that string in `data["arg0"]`. The plugin
    serialises a `nlohmann::json` object (named fields like `success`,
    `clientId`, `payload`, `timestamp`, …) and `.dump()`s it — so one
    `json.loads` gets us the structured event body.
    """
    return json.loads(event["data"]["arg0"])


def parse_push_payload(event: dict[str, Any]) -> dict[str, Any]:
    """For chatNew*/chatDeliveryAck push events: parse arg0, then re-parse the
    `payload` field (raw JSON string from liblogoschat's set_event_callback)."""
    return json.loads(parse_event(event)["payload"])


def assert_success(event: dict[str, Any], op: str) -> None:
    """For events that carry a top-level `success` bool.

    Raises if the event payload doesn't have a `success` key — chatDestroyResult
    and similar events without it would otherwise produce a misleading "failed"
    error from `body.get("success") = None`.
    """
    body = parse_event(event)
    if "success" not in body:
        raise AssertionError(f"{op}: event has no `success` field; body={body!r}")
    if not body["success"]:
        raise AssertionError(f"{op} failed: body={body!r}")


def extract_convo_id(payload: dict[str, Any]) -> str:
    """Conversation object (`{"id":...}`) or push event (`{"conversationId":...}`)."""
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
    installation_name = str(parse_event(id_evt)["clientId"])

    bundle_evt = call_and_wait(
        client, "createIntroBundle",
        event="chatCreateIntroBundleResult", timeout=10.0,
    )
    assert_success(bundle_evt, f"createIntroBundle for {label}")
    intro_bundle = str(parse_event(bundle_evt)["introBundle"])

    return ChatUser(
        client=client,
        installation_name=installation_name,
        intro_bundle=intro_bundle,
        label=label,
    )
