Logos Chat Module
=================

.. note::

   | The main Logos Messaging documentation is at
     `docs.logos.co/messaging <https://docs.logos.co/messaging>`_.
   | Start there for the concepts and the wider stack.

   **This site is the API reference** and the internal docs.

The Logos Chat Module gives your application end-to-end-encrypted
conversations -- one-to-one and group -- without implementing any of the
cryptography or the transport. It is a Logos Core ``core`` module: it wraps
`libchat <https://github.com/logos-messaging/libchat>`_ and reaches the network
through
`delivery_module <https://github.com/logos-co/logos-delivery-module>`_, so any
other module -- or a UI -- can open a conversation and exchange messages by
calling methods on ``chat_module``.

Using the API
-------------

1. ``init`` -- start the module and its delivery node (once per instance).
2. ``get_address`` -- read this installation's address, and share it with a peer.
3. ``create_conversation`` / ``create_group_conversation`` -- open a
   conversation with a peer, or start a group.
4. ``send_message`` -- publish into a conversation.
5. ``shutdown`` -- stop the module.

``init`` returns as soon as the request is dispatched; the module is ready to
exchange messages once ``delivery_state_changed`` reports ``online``. What
arrives from the network is delivered as an **event** -- subscribe to those
rather than polling.

API Reference
-------------

- :doc:`Using the API <pages/using-the-api>`
- :doc:`API reference <pages/api_reference>`

Internal docs
-------------

- :doc:`Architecture <pages/architecture>`
- :doc:`Logging <pages/logging>`
- :doc:`Versioning <pages/versioning>`

.. Hidden: the lists above are the visible index. This only builds the page
   hierarchy; these entries are what the top bar shows, and pages/internal owns
   the internal docs listed above it.

.. toctree::
   :hidden:
   :maxdepth: 2

   pages/using-the-api
   pages/api_reference
   pages/internal
