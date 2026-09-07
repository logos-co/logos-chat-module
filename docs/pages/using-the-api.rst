Using the API
=============

The contract consumers call is
`rust-lib/chat_module.lidl
<https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/chat_module.lidl>`_
-- the single source of truth, and what the :doc:`API reference
<api_reference>` is generated from. This page is the narrative version: what to
call, in what order, and what comes back.

Bring-up
--------

Bring-up is ``init(config)``, taking a ``ChatConfig`` record whose every field
is optional: ``delivery_preset`` (empty or absent means ``logos.test``) and
``log_level``.

``init`` starts delivery asynchronously and returns immediately; readiness
arrives later as a ``delivery_state_changed`` event reaching ``online``. State
is written to the instance directory the host assigns, so running two instances
side by side is a matter of giving each host its own session directory
(``--config-dir`` under ``logoscore``); ``init`` fails when the host assigned no
such directory. The delivery node listens on ports it picks itself, so
instances need no port coordination.

End-to-end chat needs a ``delivery_module`` available to the host at runtime.
Load ``chat_module`` via ``logoscore`` or Basecamp.

A generated client passes the record itself. ``logoscore call`` cannot -- it
coerces an argument to a bool, a number or a string, never to an object -- so
from the CLI pass the record's JSON text and the module reads it back:

.. code-block:: bash

   logoscore call chat_module init '{"delivery_preset":"logos.test","log_level":"debug"}'

Identity
--------

``get_address`` returns the local installation address. It is shared
out-of-band -- there is no directory to look a peer up in -- and it is what a
peer passes to ``create_conversation`` to reach this installation.

``get_installation_name`` and ``set_installation_name`` carry a local label for
this installation.

Conversations
-------------

Two conversation shapes are exposed.

``create_conversation(peer_address)`` opens a 1:1 DirectV1 conversation with
the peer at that address.

``create_group_conversation(name, desc)`` creates a GroupV2 (de-MLS) group with
this installation as its only member, grown one peer at a time with
``add_group_member(convo_id, peer_address)``. Every member sees the same
conversation id, and adds are committed by the group's steward asynchronously,
so a peer joins some time after the call returns. A group's ``name`` and
``desc`` are shared metadata carried to every joiner, both optional.

``list_group_members(convo_id)`` returns a conversation's roster from libchat's
MLS state, including invites this instance has sent that the group has not
committed yet, flagged ``pending``; a direct conversation reports both
participants and never a pending one.

The ``Conversation`` record and the ``conversation_created`` event carry a
``kind`` (``"direct"`` or ``"group"``) distinguishing the two shapes, plus a
group's shared ``name`` and ``description`` (unset for direct conversations and
unnamed groups).

Messages
--------

``send_message(convo_id, content)`` publishes into a conversation;
``get_messages(convo_id)`` reads its history.

Received messages carry a ``sender`` -- on the ``Message`` record and the
``message_received`` event -- which is the sender's directory-verified account
address, or its device id when the sender claims no account.

What comes back
---------------

Status-bearing methods return ``result``: success carries any payload (a
conversation id, an intro bundle, or nothing), failure a human-readable reason.
Collection getters (``list_conversations``, ``get_messages``,
``list_group_members``) return arrays of the corresponding record.

``health()`` is the exception: it returns ``true`` and nothing else, needs no
``init``, and holds no lock, so what a caller learns is whether the call
arrived at all. It exists because a module that dies takes no part in noticing.
Nothing is pushed when the process goes, and a consumer otherwise finds out
only when the next thing it does runs out its own timeout, having looked
connected until then. Polling this turns that into a bounded delay, and a
failed call is the answer.

Events
------

The module pushes seven events over the ``lp_*`` IPC event channel; consumers
subscribe via ``on_<event>()`` rather than polling. Their names, arguments and
meanings are in the :doc:`API reference <api_reference>`.

Two of them shape the flow of everything above: ``delivery_state_changed`` is
how you learn that ``init`` has finished bringing the network up, and
``message_received`` is how inbound messages arrive. There is no polling
equivalent for either.
