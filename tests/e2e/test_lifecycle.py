"""Lifecycle e2e checks: stopChat/destroyChat reach + initChat sync-False branch."""

from __future__ import annotations

import pytest

from libs.constants import LIFECYCLE_USER_PORT
from libs.helpers import (
    MODULE,
    BareChatClientFactory,
    ChatUserFactory,
    assert_no_event,
    call_and_wait,
    parse_event,
)


def test_stop_destroy_clean_teardown(chat_user_factory: ChatUserFactory) -> None:
    """Run a fully-initialised user through stopChat + destroyChat."""
    user = chat_user_factory("Solo", LIFECYCLE_USER_PORT)

    # Tight 5s deadline: a 20s `waitForFinished` timeout under event-loop
    # flush starvation is the regression the deferred-emit fix prevents.
    stop_evt = call_and_wait(
        user.client,
        "stopChat",
        event="chatStopResult",
        timeout=5.0,
    )
    stop_body = parse_event(stop_evt)
    assert stop_body.get("success") is True, f"stopChat failed: {stop_body!r}"
    assert set(stop_body.keys()) == {"success", "statusCode", "message", "timestamp"}, (
        f"chatStopResult shape drift: {stop_body!r}"
    )

    # `chatDestroyResult` is documented as emitted only when the SDK supplies
    # a message — its absence is acceptable. The load-bearing signal is the
    # sync return + a clean container teardown afterwards.
    assert user.client.call(MODULE, "destroyChat") is not False, (
        "destroyChat returned False after a successful initChat"
    )


@pytest.mark.xfail(
    reason="initChat contract mismatch: chat_new is documented to return null on "
    "failure (tests/stubs/lib/liblogoschat.h), so the plugin promises sync False + "
    "no event for bad config. The real libchat instead returns a non-null context "
    "for this malformed config and reports the parse error asynchronously via "
    "chatInitResult(success=false).",
    strict=True,
)
def test_init_chat_bad_config_returns_false_sync(
    bare_chat_client_factory: BareChatClientFactory,
) -> None:
    """initChat with an unparseable config must return sync False and emit no event.

    Do NOT weaken the trigger to anything libchat could parse (e.g. `"{}"`):
    that path goes through start-up validation, not the NULL-check branch.
    """
    client = bare_chat_client_factory("badcfg")

    ok = assert_no_event(
        client,
        event="chatInitResult",
        trigger=lambda: client.call(MODULE, "initChat", "{not valid json"),
        timeout=3.0,
        op="initChat with malformed JSON",
    )
    assert ok is False, (
        f"initChat with malformed JSON should return literal False, got {ok!r}"
    )
