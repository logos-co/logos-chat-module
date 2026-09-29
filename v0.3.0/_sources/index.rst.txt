Logos Chat Module
=================

.. note::

   | The main Logos Messaging documentation is at
     `docs.logos.co/messaging <https://docs.logos.co/messaging>`_.
   | Start there for the concepts, the wider stack, and a walk-through of
     `building a module that uses this API
     <https://docs.logos.co/messaging/chat-module/build-logos-module-that-uses-chat-module-api>`_.

   **This site is the API reference** and the internal docs.

The Logos Chat Module gives your application end-to-end-encrypted
conversations -- one-to-one and group -- without implementing any of the
cryptography or the transport. It is a Logos Core ``core`` module: it wraps
`libchat <https://github.com/logos-messaging/logos-chat>`_ and reaches the network
through
`delivery_module <https://github.com/logos-co/logos-delivery-module>`_, so any
other module -- or a UI -- can open a conversation and exchange messages by
calling methods on ``chat_module``.

Its public surface is a single `LIDL
<https://github.com/logos-co/logos-lidl/blob/master/docs/spec.md>`_ contract,
`rust-lib/chat_module.lidl
<https://github.com/logos-co/logos-chat-module/blob/master/rust-lib/chat_module.lidl>`_.
Consumers generate a typed client from it, and the reference below is rendered
from the same file.

API Reference
-------------

- :doc:`API reference <pages/api_reference>` -- every method, event and record,
  generated from the contract.

Internal docs
-------------

- :doc:`Logging <pages/logging>` -- what the module logs, where it goes, and
  how to turn it up.

.. Hidden: the lists above are the visible index. This only builds the page
   hierarchy; these entries are what the top bar shows, and pages/internal owns
   the internal docs listed above it.

.. toctree::
   :hidden:
   :maxdepth: 2

   pages/api_reference
   pages/internal
