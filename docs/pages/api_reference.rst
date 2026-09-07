API reference
=============

The module's public surface is its LIDL contract,
`rust-lib/chat_module.lidl
<https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/chat_module.lidl>`_.
Everything on this page is rendered from that file, so it says exactly what a
generated client can call. See :doc:`Using the API <using-the-api>` for how the
pieces fit together.

Types are LIDL's, not any one language's: ``tstr`` is a text string, ``bstr``
binary data, ``int`` a signed integer, ``bool`` a boolean, ``[T]`` an array of
``T``, and ``result`` a structured success-or-error. How each maps onto a
concrete type is up to the SDK generating your client.

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
return value.

.. include:: ../_generated/events.rst

Records
-------

The structured payloads the methods above exchange. A field marked *optional*
may be absent.

.. include:: ../_generated/records.rst
