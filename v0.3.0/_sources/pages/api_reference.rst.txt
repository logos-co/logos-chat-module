API reference
=============

The module's public surface is its contract,
`rust-lib/chat_module.lidl
<https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/chat_module.lidl>`_.
Everything on this page is rendered from that file, so it says exactly what a
generated client can call.

For a walk-through of the calls in order -- bring-up, identity, opening a
conversation, sending -- with working code, see `building a module that uses
the Chat module API
<https://docs.logos.co/messaging/chat-module/build-logos-module-that-uses-chat-module-api>`_
on docs.logos.co.

Types are `LIDL
<https://github.com/logos-co/logos-lidl/blob/master/docs/spec.md>`_'s, not any
one language's. The primitives are ``tstr`` (text), ``bstr`` (binary), ``int``,
``uint``, ``float64``, ``bool``, ``any``, and ``result`` -- a structured
success-or-error. They compose as ``[T]`` for an array, ``{K: V}`` for a map,
and ``?T`` for a value that may be absent. That set is fixed by the
specification; how each maps onto a concrete type is up to the SDK generating
your client.

Methods
-------

Calls are dispatched over the Logos IPC bus. Status-bearing methods return
``result``: success carries any payload -- a conversation id, or nothing --
and failure a human-readable reason. Collection getters return an array of the
named record. A method that returns a plain string returns it empty when the
module has not been initialised.

.. include:: ../_generated/methods.rst

Events
------

Events are pushed over the ``lp_*`` IPC event channel; a consumer subscribes
with ``on_<event>()`` rather than polling. They are fire-and-forget: they carry
their arguments positionally, in the order listed here, and return nothing.

This is where the results of the network arrive. A call returns once the
request is dispatched, so an inbound message, a peer accepting an invite, or
the delivery node coming online each reach you as an event rather than as a
return value. Subscribe before calling :lidl:method:`init`, or the earliest
ones are missed.

.. include:: ../_generated/events.rst

Records
-------

The structured payloads the methods above exchange. A field marked *optional*
may be absent.

.. include:: ../_generated/records.rst
