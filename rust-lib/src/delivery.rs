//! `Transport` impl bridging the client's delivery boundary to delivery_module.
//!
//! `publish` forwards each outbound envelope to delivery_module; `subscribe`
//! queues the core's interest in a delivery address (forwarded once the node is
//! started, see `inbound.rs`); `inbound` hands the client the channel the module
//! feeds with received payloads.

use crossbeam_channel::{Receiver, Sender};
use logos_generic_chat::{AddressedEnvelope, DeliveryService, Transport};
use logos_rust_sdk::LogosError;
use serde_json::Value;

/// The single home for chat's content-topic scheme. Both the outbound topic
/// ([`content_topic_for`]) and the inbound prefix filter (`inbound.rs`) derive
/// from it, so the wire scheme lives in exactly one place.
pub(crate) const TOPIC_PREFIX: &str = "/logos-chat/1/";

pub(crate) fn content_topic_for(delivery_address: &str) -> String {
    format!("{TOPIC_PREFIX}{delivery_address}/proto")
}

/// A delivery_module `result` as delivery_module decided it: the returned value
/// on success, its reason otherwise.
///
/// The generated client's `Ok` says only that the call arrived; delivery_module's
/// own verdict is the `{success, value, error}` envelope it carries.
///
/// TODO: drop once the generated client maps a failed `result` to `Err`
/// (logos-co/logos-rust-sdk#63).
pub(crate) fn delivery_outcome(res: Result<Value, LogosError>) -> Result<Value, String> {
    let mut envelope = res.map_err(|e| e.to_string())?;
    match envelope.get("success").and_then(Value::as_bool) {
        Some(true) => Ok(envelope
            .get_mut("value")
            .map(Value::take)
            .unwrap_or_default()),
        Some(false) => Err(envelope
            .get("error")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("no reason given")
            .to_owned()),
        None => Err(format!("not a result envelope: {envelope}")),
    }
}

/// Carries each direction of the client's delivery boundary: the outbound
/// [`SdkPublisher`], plus the inbound payload stream the client's worker drains.
#[derive(Debug)]
pub(crate) struct SdkDelivery {
    /// Handed to the client once via [`Transport::inbound`]. The module feeds the
    /// matching sender from delivery_module's `messageReceived` events.
    inbound_rx: Option<Receiver<Vec<u8>>>,
    publisher: SdkPublisher,
}

impl SdkDelivery {
    pub(crate) fn new(inbound_rx: Receiver<Vec<u8>>, subscribe_tx: Sender<String>) -> Self {
        Self {
            inbound_rx: Some(inbound_rx),
            publisher: SdkPublisher { subscribe_tx },
        }
    }

    /// A handle on the outbound half alone, for a consumer that publishes over
    /// the same delivery node but never reads the inbound stream.
    pub(crate) fn publisher(&self) -> SdkPublisher {
        self.publisher.clone()
    }
}

impl DeliveryService for SdkDelivery {
    type Error = String;

    fn publish(&mut self, envelope: AddressedEnvelope) -> Result<(), String> {
        self.publisher.publish(envelope)
    }

    fn subscribe(&mut self, delivery_address: &str) -> Result<(), String> {
        self.publisher.subscribe(delivery_address)
    }
}

impl Transport for SdkDelivery {
    fn inbound(&mut self) -> Receiver<Vec<u8>> {
        self.inbound_rx
            .take()
            .expect("SdkDelivery::inbound called more than once")
    }
}

/// The outbound half of the delivery boundary. Clonable, so a consumer that only
/// publishes holds its own handle without a second claim on the inbound stream.
#[derive(Clone, Debug)]
pub(crate) struct SdkPublisher {
    /// Subscription requests from the core, drained by the inbound worker.
    subscribe_tx: Sender<String>,
}

impl DeliveryService for SdkPublisher {
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
                if let Err(e) = delivery_outcome(res) {
                    tracing::error!("delivery_module.send failed: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn result(success: bool, value: Value, error: Value) -> Result<Value, LogosError> {
        Ok(json!({ "success": success, "value": value, "error": error }))
    }

    #[test]
    fn a_successful_result_yields_its_value() {
        assert_eq!(
            delivery_outcome(result(true, json!("enr:-abc"), Value::Null)),
            Ok(json!("enr:-abc"))
        );
    }

    /// The call arrived and delivery_module said no, which the generated client
    /// alone reports as `Ok`.
    #[test]
    fn a_refused_call_fails_with_delivery_modules_reason() {
        assert_eq!(
            delivery_outcome(result(false, Value::Null, json!("Context not initialized"))),
            Err("Context not initialized".to_owned())
        );
    }

    #[test]
    fn a_refusal_without_a_reason_still_fails() {
        for error in [Value::Null, json!("")] {
            assert_eq!(
                delivery_outcome(result(false, Value::Null, error)),
                Err("no reason given".to_owned())
            );
        }
    }

    #[test]
    fn a_call_that_never_arrived_keeps_its_error() {
        let failed = delivery_outcome(Err(LogosError::Other("no such module".into())));
        assert!(failed.unwrap_err().contains("no such module"));
    }

    #[test]
    fn a_value_with_no_envelope_fails() {
        assert!(delivery_outcome(Ok(json!(true))).is_err());
    }
}
