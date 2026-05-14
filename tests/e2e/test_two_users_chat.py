"""Two-user chat PR-time e2e check.

Flow:
  1. Saro initiates a private conversation with Raya by accepting Raya's intro
     bundle, including an opening message.
  2. Both Saro and Raya observe `chatNewConversation` push events on their
     own clients (X3DH-style libchat protocol: Initiator and Responder get
     different local convo ids; both are valid for `sendMessage` from their
     respective sides). Raya also observes `chatNewMessage` with Saro's content.
  3. Raya replies to Saro. Saro sees `chatNewMessage`.
  4. Repeat one more round each direction (>=2 messages each way).

Subscriptions on the receiving client are opened BEFORE the sending client
triggers anything — `logoscore watch` subprocess needs a beat to come live,
and missing the push event because of that race is the most likely flake mode.
"""

from __future__ import annotations

import time

from logos_integration_test_framework import subscribe

from libs.helpers import (
    MODULE,
    SUBSCRIBE_GRACE_S,
    ChatUser,
    assert_success,
    call_and_wait,
    extract_convo_id,
    extract_message_content,
    hex_encode,
    parse_event,
    parse_push_payload,
    wait_event,
)

MSG_S1 = "hello-from-saro-1"
MSG_R1 = "hello-from-raya-1"
MSG_S2 = "hello-from-saro-2"
MSG_R2 = "hello-from-raya-2"


def _send_and_verify(
    sender: ChatUser,
    sender_convo_id: str,
    receiver: ChatUser,
    content: str,
    op: str,
) -> None:
    """sender → receiver via sendMessage; assert receiver gets the matching content."""
    with subscribe(receiver.client, MODULE, "chatNewMessage") as nm:
        time.sleep(SUBSCRIBE_GRACE_S)
        send_evt = call_and_wait(
            sender.client,
            "sendMessage",
            sender_convo_id,
            hex_encode(content),
            event="chatSendMessageResult",
            timeout=15.0,
        )
        assert_success(send_evt, op)
        received = wait_event(nm, "chatNewMessage", timeout=20.0)
    assert extract_message_content(parse_push_payload(received)) == content


def test_two_users_can_chat(saro: ChatUser, raya: ChatUser) -> None:
    # X3DH-asymmetric: Saro and Raya get DIFFERENT local convo ids for the
    # same logical conversation; each side uses its own when sending.
    with (
        subscribe(saro.client, MODULE, "chatNewConversation") as nc_s,
        subscribe(raya.client, MODULE, "chatNewConversation") as nc_r,
        subscribe(raya.client, MODULE, "chatNewMessage") as nm_r,
    ):
        time.sleep(SUBSCRIBE_GRACE_S)

        npc_evt = call_and_wait(
            saro.client,
            "newPrivateConversation",
            raya.intro_bundle,
            hex_encode(MSG_S1),
            event="chatNewPrivateConversationResult",
            timeout=20.0,
        )
        # `success` may be False on the success path because liblogoschat
        # returns ok("") for newPrivateConversation, leaving conversation=""
        # → plugin's `success = (RET_OK && !empty())` flag flips false. Use
        # statusCode==0 (RET_OK) as the truthful success signal; pull convo ids
        # from the chatNewConversation push events below.
        assert parse_event(npc_evt)["statusCode"] == 0, f"newPrivateConversation failed: {npc_evt!r}"

        convo_id_saro = extract_convo_id(
            parse_push_payload(wait_event(nc_s, "chatNewConversation", timeout=20.0))
        )
        convo_id_raya = extract_convo_id(
            parse_push_payload(wait_event(nc_r, "chatNewConversation", timeout=20.0))
        )

        first_msg = wait_event(nm_r, "chatNewMessage", timeout=20.0)
        # Push events wrap libchat's payload as a *string* (consumers do two
        # json.loads). Pin this — without the assert, a regression surfaces
        # as a cryptic TypeError from parse_push_payload.
        assert isinstance(parse_event(first_msg)["payload"], str), (
            f"chatNewMessage `payload` must be a JSON-encoded string, "
            f"got {type(parse_event(first_msg)['payload']).__name__}"
        )
        assert extract_message_content(parse_push_payload(first_msg)) == MSG_S1

    _send_and_verify(raya, convo_id_raya, saro, MSG_R1, "Raya.sendMessage #1")
    _send_and_verify(saro, convo_id_saro, raya, MSG_S2, "Saro.sendMessage #2")
    _send_and_verify(raya, convo_id_raya, saro, MSG_R2, "Raya.sendMessage #2")
