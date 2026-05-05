"""Two-user chat PR-time integration check.

Flow:
  1. A initiates a private conversation with B by accepting B's intro bundle,
     including an opening message.
  2. Both A and B observe `chatNewConversation` push events on their own
     clients (X3DH-style libchat protocol: Initiator and Responder get
     DIFFERENT local convo ids; both are valid for `sendMessage` from their
     respective sides). B also observes `chatNewMessage` with A's content.
  3. B replies to A. A sees `chatNewMessage`.
  4. Repeat one more round each direction (>=2 messages each way).

Goal: red CI on this PR if `chat_module` / `liblogoschat` / waku-stack
break the basic happy path. Library-level coverage is elsewhere.

Subscriptions on the receiving client are opened BEFORE the sending client
triggers anything — the framework's `logoscore watch` subprocess needs a beat
to come live, and missing the push event because of that race is the most
likely flake mode for cross-process tests like this.

Notes on bugs in the underlying library that this test works around:

- `chatNewPrivateConversationResult.data[0]` (success bool) is unreliable and
  `data[2]` (conversation JSON) is empty — root cause is
  `library/api/conversation_api.nim:42` returning `ok("")` instead of
  `ok($conversationId)`. We use `data[1] == 0` (RET_OK) as the success
  signal and pull convo_id_a from A's own `chatNewConversation` push event.
  Once the library is fixed, the workaround can revert; the push-event
  approach stays correct regardless.
"""

from __future__ import annotations

import time

from logos_integration_test_framework import EventTimeout, subscribe

from _helpers import (
    MODULE,
    SUBSCRIBE_GRACE_S,
    ChatUser,
    assert_success,
    call_and_wait,
    extract_convo_id,
    extract_message_content,
    hex_encode,
    parse_json_field,
)


def test_two_users_can_chat(chat_users: tuple[ChatUser, ChatUser]) -> None:
    a, b = chat_users

    # ── 1+2. Pre-open subscriptions on BOTH clients before A triggers anything.
    # Each side gets its own `chatNewConversation` event with its own local
    # convo id (X3DH-asymmetric: Initiator's id != Responder's id by design;
    # see core/conversations/src/conversation/privatev1.rs `id_for_participant`).
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
        # data[0] (success bool) ненадёжен из-за бага в conversation_api.nim:42
        # (returns ok("") on success). Use data[1] == 0 (RET_OK) as the success
        # signal; data[2] is empty for the same reason — convo_id_a comes from
        # the chatNewConversation push event below.
        assert npc_evt["data"][1] == 0, f"newPrivateConversation failed: {npc_evt!r}"

        try:
            convo_id_a = extract_convo_id(parse_json_field(nc_a.next(timeout=20.0), 0))
        except EventTimeout as e:
            raise AssertionError(
                "Alice did not receive her own chatNewConversation push event "
                "after newPrivateConversation accepted (RET_OK). Likely "
                "newPrivateConversation failed inside liblogoschat without "
                "surfacing through the result event. Check daemon logs."
            ) from e
        try:
            convo_id_b = extract_convo_id(parse_json_field(nc_b.next(timeout=20.0), 0))
        except EventTimeout as e:
            raise AssertionError(
                "Bob did not receive chatNewConversation push event from "
                "Alice's introduction. Likely waku relay-mesh between Alice → "
                "bootstrap → Bob did not establish; check nwaku-bootstrap logs."
            ) from e

        first_msg = nm_b.next(timeout=20.0)
        assert extract_message_content(parse_json_field(first_msg, 0)) == "hello-from-A-1"

    # ── 3. B → A reply ─────────────────────────────────────────────────
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
        a_received = nm_a.next(timeout=20.0)
    assert extract_message_content(parse_json_field(a_received, 0)) == "hello-from-B-1"

    # ── 4a. A → B (round 2) ────────────────────────────────────────────
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
        b_received_2 = nm_b2.next(timeout=20.0)
    assert extract_message_content(parse_json_field(b_received_2, 0)) == "hello-from-A-2"

    # ── 4b. B → A (round 2) ────────────────────────────────────────────
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
        a_received_2 = nm_a2.next(timeout=20.0)
    assert extract_message_content(parse_json_field(a_received_2, 0)) == "hello-from-B-2"
