"""Two-user chat PR-time integration check.

Flow:
  1. A initiates a private conversation with B by accepting B's intro bundle,
     including an opening message.
  2. Both A and B observe `chatNewConversation` push events on their own
     clients (X3DH-style libchat protocol: Initiator and Responder get
     different local convo ids; both are valid for `sendMessage` from their
     respective sides). B also observes `chatNewMessage` with A's content.
  3. B replies to A. A sees `chatNewMessage`.
  4. Repeat one more round each direction (>=2 messages each way).

Subscriptions on the receiving client are opened BEFORE the sending client
triggers anything — `logoscore watch` subprocess needs a beat to come live,
and missing the push event because of that race is the most likely flake mode.
"""

from __future__ import annotations

import time

from logos_integration_test_framework import subscribe

from _helpers import (
    MODULE,
    SUBSCRIBE_GRACE_S,
    ChatUser,
    assert_success,
    call_and_wait,
    event_arg,
    extract_convo_id,
    extract_message_content,
    hex_encode,
    parse_json_field,
    wait_event,
)


def test_two_users_can_chat(chat_users: tuple[ChatUser, ChatUser]) -> None:
    a, b = chat_users

    # X3DH-asymmetric: Alice and Bob get DIFFERENT local convo ids for the
    # same logical conversation; each side needs its own.
    with (
        subscribe(a.client, MODULE, "chatNewConversation") as nc_a,
        subscribe(b.client, MODULE, "chatNewConversation") as nc_b,
        subscribe(b.client, MODULE, "chatNewMessage") as nm_b,
    ):
        time.sleep(SUBSCRIBE_GRACE_S)

        npc_evt = call_and_wait(
            a.client,
            "newPrivateConversation",
            b.intro_bundle,
            hex_encode("hello-from-A-1"),
            event="chatNewPrivateConversationResult",
            timeout=20.0,
        )
        # arg0 (success bool) and arg2 (conversation JSON) are both unreliable —
        # liblogoschat's `conversation_api.nim:42` returns `ok("")` on success.
        # Use arg1 == 0 (RET_OK) as the success signal; pull convo ids from the
        # chatNewConversation push events below.
        assert event_arg(npc_evt, 1) == 0, f"newPrivateConversation failed: {npc_evt!r}"

        convo_id_a = extract_convo_id(
            parse_json_field(wait_event(nc_a, "chatNewConversation", timeout=20.0), 0)
        )
        convo_id_b = extract_convo_id(
            parse_json_field(wait_event(nc_b, "chatNewConversation", timeout=20.0), 0)
        )

        first_msg = wait_event(nm_b, "chatNewMessage", timeout=20.0)
        assert extract_message_content(parse_json_field(first_msg, 0)) == "hello-from-A-1"

    # B → A reply
    with subscribe(a.client, MODULE, "chatNewMessage") as nm_a:
        time.sleep(SUBSCRIBE_GRACE_S)
        send_b = call_and_wait(
            b.client,
            "sendMessage",
            convo_id_b,
            hex_encode("hello-from-B-1"),
            event="chatSendMessageResult",
            timeout=15.0,
        )
        assert_success(send_b, "B.sendMessage #1")
        a_received = wait_event(nm_a, "chatNewMessage", timeout=20.0)
    assert extract_message_content(parse_json_field(a_received, 0)) == "hello-from-B-1"

    # A → B (round 2)
    with subscribe(b.client, MODULE, "chatNewMessage") as nm_b2:
        time.sleep(SUBSCRIBE_GRACE_S)
        send_a2 = call_and_wait(
            a.client,
            "sendMessage",
            convo_id_a,
            hex_encode("hello-from-A-2"),
            event="chatSendMessageResult",
            timeout=15.0,
        )
        assert_success(send_a2, "A.sendMessage #2")
        b_received_2 = wait_event(nm_b2, "chatNewMessage", timeout=20.0)
    assert extract_message_content(parse_json_field(b_received_2, 0)) == "hello-from-A-2"

    # B → A (round 2)
    with subscribe(a.client, MODULE, "chatNewMessage") as nm_a2:
        time.sleep(SUBSCRIBE_GRACE_S)
        send_b2 = call_and_wait(
            b.client,
            "sendMessage",
            convo_id_b,
            hex_encode("hello-from-B-2"),
            event="chatSendMessageResult",
            timeout=15.0,
        )
        assert_success(send_b2, "B.sendMessage #2")
        a_received_2 = wait_event(nm_a2, "chatNewMessage", timeout=20.0)
    assert extract_message_content(parse_json_field(a_received_2, 0)) == "hello-from-B-2"
