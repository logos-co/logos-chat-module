"""`chatDeliveryAck` push-event e2e check — the third push channel alongside
`chatNewMessage` and `chatNewConversation`."""

from __future__ import annotations

import time

import pytest
from logos_integration_test_framework import subscribe

from libs.helpers import (
    MODULE,
    SUBSCRIBE_GRACE_S,
    ChatUser,
    assert_success,
    call_and_wait,
    hex_encode,
    parse_event,
    parse_push_payload,
    wait_event,
)


@pytest.mark.xfail(
    reason="https://github.com/logos-messaging/libchat/issues/121",
    strict=True,
)
def test_delivery_ack_received(saro: ChatUser, raya: ChatUser) -> None:
    # Bootstrap an X3DH conversation. We need an established conversation so
    # `sendMessage` from Saro to Raya goes over the wire and produces a
    # delivery-ack back to Saro.
    with (
        subscribe(saro.client, MODULE, "chatNewConversation") as nc_s,
        subscribe(raya.client, MODULE, "chatNewMessage") as nm_r,
    ):
        time.sleep(SUBSCRIBE_GRACE_S)
        npc_evt = call_and_wait(
            saro.client,
            "newPrivateConversation",
            raya.intro_bundle,
            hex_encode("ack-test-opener"),
            event="chatNewPrivateConversationResult",
            timeout=20.0,
        )
        assert parse_event(npc_evt)["statusCode"] == 0, (
            f"newPrivateConversation failed: {npc_evt!r}"
        )
        nc_payload = parse_push_payload(
            wait_event(nc_s, "chatNewConversation", timeout=20.0)
        )
        convo_id_saro = str(nc_payload["conversationId"])
        # Drain the opener arrival on Raya so it doesn't get picked up by the
        # next-step subscription on Saro.
        wait_event(nm_r, "chatNewMessage", timeout=20.0)

    # Two separate blocks on purpose: any opener-ack fires before this
    # subscription mounts and is dropped — we only care about the ack for
    # the explicit sendMessage below.
    with subscribe(saro.client, MODULE, "chatDeliveryAck") as ack_s:
        time.sleep(SUBSCRIBE_GRACE_S)
        send_evt = call_and_wait(
            saro.client,
            "sendMessage",
            convo_id_saro,
            hex_encode("ack-test-body"),
            event="chatSendMessageResult",
            timeout=15.0,
        )
        assert_success(send_evt, "Saro.sendMessage (delivery-ack)")
        ack_evt = wait_event(ack_s, "chatDeliveryAck", timeout=20.0)

    ack_payload = parse_push_payload(ack_evt)
    assert ack_payload["eventType"] == "delivery_ack", (
        f"unexpected eventType in delivery-ack payload: {ack_payload!r}"
    )
    assert ack_payload["conversationId"] == convo_id_saro, (
        f"delivery-ack conversationId mismatch: "
        f"expected {convo_id_saro!r}, got {ack_payload['conversationId']!r}"
    )
    # `messageId` correlation is best-effort. `chatSendMessageResult.result`
    # may be opaque (libchat owns the format); just verify `messageId` is
    # present and non-empty in the ack payload, which proves the plugin
    # propagates the field rather than dropping it.
    assert "messageId" in ack_payload, f"delivery-ack missing messageId: {ack_payload!r}"
    assert isinstance(ack_payload["messageId"], str) and ack_payload["messageId"], (
        f"delivery-ack messageId is empty or non-string: {ack_payload!r}"
    )
