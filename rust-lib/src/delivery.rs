//! `Transport` impl bridging the client's delivery boundary to delivery_module.
//!
//! `publish` forwards each outbound envelope to delivery_module; `subscribe`
//! queues the core's interest in a delivery address (forwarded once the node is
//! started, see `inbound.rs`); `inbound` hands the client the channel the module
//! feeds with received payloads.

use crossbeam_channel::{Receiver, Sender};
use logos_generic_chat::{AddressedEnvelope, DeliveryService, Transport};

/// The single home for chat's content-topic scheme. Both the outbound topic
/// ([`content_topic_for`]) and the inbound prefix filter (`inbound.rs`) derive
/// from it, so the wire scheme lives in exactly one place.
pub(crate) const TOPIC_PREFIX: &str = "/logos-chat/1/";

pub(crate) fn content_topic_for(delivery_address: &str) -> String {
    format!("{TOPIC_PREFIX}{delivery_address}/proto")
}

/// Carries each direction of the client's delivery boundary: outbound publishing
/// and subscription forwarding to delivery_module, plus the inbound payload
/// stream the client's worker drains.
#[derive(Debug)]
pub(crate) struct SdkDelivery {
    /// Handed to the client once via [`Transport::inbound`]. The module feeds the
    /// matching sender from delivery_module's `messageReceived` events.
    inbound_rx: Option<Receiver<Vec<u8>>>,
    /// Subscription requests from the core, drained by the inbound worker.
    subscribe_tx: Sender<String>,
}

impl SdkDelivery {
    pub(crate) fn new(inbound_rx: Receiver<Vec<u8>>, subscribe_tx: Sender<String>) -> Self {
        Self {
            inbound_rx: Some(inbound_rx),
            subscribe_tx,
        }
    }
}

impl DeliveryService for SdkDelivery {
    type Error = String;

    fn publish(&mut self, envelope: AddressedEnvelope) -> Result<(), String> {
        // Topic derived from the recipient's delivery address; send_async base64url-
        // encodes the envelope onto the lp_* wire.
        let topic = content_topic_for(&envelope.delivery_address);
        // Fire-and-forget: the synchronous `send` would block the dispatch thread on
        // delivery's accept handshake, so hand off async and return. A failed send is
        // only logged, not surfaced to the caller; a future "sent" confirmation will
        // close that gap.
        crate::modules()
            .delivery_module
            .send_async(&topic, &envelope.data, move |res| {
                if let Err(e) = res {
                    eprintln!("chat_module: delivery_module.send failed: {e}");
                }
            });
        Ok(())
    }

    fn subscribe(&mut self, delivery_address: &str) -> Result<(), String> {
        // The core subscribes its inbound addresses at construction, before the
        // delivery node exists. Queue the topic; the inbound worker forwards it to
        // delivery_module once the node is started (see `inbound::forward_subscriptions`).
        self.subscribe_tx
            .send(content_topic_for(delivery_address))
            .map_err(|e| e.to_string())
    }
}

impl Transport for SdkDelivery {
    fn inbound(&mut self) -> Receiver<Vec<u8>> {
        self.inbound_rx
            .take()
            .expect("SdkDelivery::inbound called more than once")
    }
}
