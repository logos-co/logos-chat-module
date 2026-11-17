//! `Transport` impl bridging the client's delivery boundary to delivery_module.
//!
//! `publish` forwards each outbound envelope to delivery_module; `subscribe`
//! queues the core's interest in a delivery address (forwarded once the node is
//! started, see `inbound.rs`); `inbound` hands the client the channel the module
//! feeds with received payloads, and `delivery_reports` the one it feeds with the
//! ids of published payloads. `fetch_messages` fails: delivery_module's store
//! query needs a store peer's address, and nothing delivery_module offers names
//! one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use logos_generic_chat::{
    AddressedEnvelope, DeliveryInfo, DeliveryMsgId, DeliveryReport, DeliveryService,
    InboundPayload, PublishReceipt, PublishedId, Transport,
};
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

/// The delivery id of the payload delivery_module's 0x-hex `message_hash`
/// names: the hash's bytes, as libchat's own delivery adapter reads it, so the
/// ids agree across clients.
pub(crate) fn delivery_info(message_hash: &str) -> Option<DeliveryInfo> {
    let bytes = message_hash
        .strip_prefix("0x")
        .and_then(|hex| hex::decode(hex).ok());
    if bytes.is_none() {
        tracing::warn!("delivery_module reported an unreadable message hash: {message_hash}");
    }
    bytes.map(|bytes| DeliveryInfo {
        delivery_msg_id: DeliveryMsgId::from(bytes),
    })
}

/// Carries each direction of the client's delivery boundary: the outbound
/// [`SdkPublisher`], plus the inbound payload stream and the delivery reports
/// the client's worker drains.
#[derive(Debug)]
pub(crate) struct SdkDelivery {
    /// Handed to the client once via [`Transport::inbound`]. The module feeds the
    /// matching sender from delivery_module's `messageReceived` events.
    inbound_rx: Option<Receiver<InboundPayload>>,
    /// Handed to the client once via [`Transport::delivery_reports`]. The module
    /// feeds the matching sender from what delivery_module reports of each send.
    reports_rx: Option<Receiver<DeliveryReport>>,
    publisher: SdkPublisher,
}

impl SdkDelivery {
    pub(crate) fn new(
        inbound_rx: Receiver<InboundPayload>,
        reports_rx: Receiver<DeliveryReport>,
        publisher: SdkPublisher,
    ) -> Self {
        Self {
            inbound_rx: Some(inbound_rx),
            reports_rx: Some(reports_rx),
            publisher,
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

    fn publish(&mut self, envelope: AddressedEnvelope) -> Result<PublishedId, String> {
        self.publisher.publish(envelope)
    }

    fn subscribe(&mut self, delivery_address: &str) -> Result<(), String> {
        self.publisher.subscribe(delivery_address)
    }

    fn fetch_messages(
        &mut self,
        delivery_address: &str,
        delivery_msg_ids: &[DeliveryMsgId],
    ) -> Result<(), String> {
        self.publisher
            .fetch_messages(delivery_address, delivery_msg_ids)
    }
}

impl Transport for SdkDelivery {
    fn inbound(&mut self) -> Receiver<InboundPayload> {
        self.inbound_rx
            .take()
            .expect("SdkDelivery::inbound called more than once")
    }

    fn delivery_reports(&mut self) -> Receiver<DeliveryReport> {
        self.reports_rx
            .take()
            .expect("SdkDelivery::delivery_reports called more than once")
    }
}

/// The outbound half of the delivery boundary. Clonable, so a consumer that only
/// publishes holds its own handle without a second claim on the inbound stream.
#[derive(Clone, Debug)]
pub(crate) struct SdkPublisher {
    /// Subscription requests from the core, drained by the inbound worker.
    subscribe_tx: Sender<String>,
    /// The request id delivery_module accepted each send under, with the receipt
    /// `publish` returned for it, for the inbound worker to match against what
    /// delivery_module reports of the send later.
    accepted_tx: Sender<(String, PublishReceipt)>,
    /// Where a send delivery_module refused is reported.
    reports_tx: Sender<DeliveryReport>,
    /// Numbers the receipts `publish` returns.
    sends: Arc<AtomicU64>,
}

impl SdkPublisher {
    pub(crate) fn new(
        subscribe_tx: Sender<String>,
        accepted_tx: Sender<(String, PublishReceipt)>,
        reports_tx: Sender<DeliveryReport>,
    ) -> Self {
        Self {
            subscribe_tx,
            accepted_tx,
            reports_tx,
            sends: Arc::default(),
        }
    }
}

impl DeliveryService for SdkPublisher {
    type Error = String;

    fn publish(&mut self, envelope: AddressedEnvelope) -> Result<PublishedId, String> {
        // Topic derived from the recipient's delivery address; send_async base64url-
        // encodes the envelope onto the lp_* wire.
        let topic = content_topic_for(&envelope.delivery_address);
        // Fire-and-forget: the synchronous `send` would block the dispatch thread on
        // delivery's accept handshake, so hand off async and return a receipt. What
        // becomes of the payload is reported under it: its id once delivery_module's
        // events name the request id the callback receives, or none when the send
        // fails.
        let receipt = PublishReceipt::from(
            self.sends
                .fetch_add(1, Ordering::Relaxed)
                .to_be_bytes()
                .to_vec(),
        );
        let accepted_tx = self.accepted_tx.clone();
        let reports_tx = self.reports_tx.clone();
        let sent = receipt.clone();
        crate::modules()
            .delivery_module
            .send_async(&topic, &envelope.data, move |res| {
                match delivery_outcome(res) {
                    Ok(Value::String(request_id)) => {
                        if accepted_tx.send((request_id, sent.clone())).is_ok() {
                            return;
                        }
                        // The bridge has stopped, so nothing else will report the send.
                        tracing::warn!("delivery_module accepted a send nothing will report");
                    }
                    Ok(other) => {
                        tracing::error!("delivery_module.send returned no request id: {other}")
                    }
                    Err(e) => tracing::error!("delivery_module.send failed: {e}"),
                }
                // The receiver is the client's worker; if it has gone away the client is
                // being dropped, so a failed send is benign.
                let _ = reports_tx.send(DeliveryReport {
                    receipt: sent,
                    delivery_info: None,
                });
            });
        Ok(PublishedId::Pending(receipt))
    }

    fn subscribe(&mut self, delivery_address: &str) -> Result<(), String> {
        // The core subscribes its inbound addresses at construction, before the
        // delivery node exists. Queue the topic; the inbound worker forwards it to
        // delivery_module once the node is started (see `inbound::forward_subscriptions`).
        self.subscribe_tx
            .send(content_topic_for(delivery_address))
            .map_err(|e| e.to_string())
    }

    fn fetch_messages(&mut self, _: &str, _: &[DeliveryMsgId]) -> Result<(), String> {
        Err("delivery_module names no store peer to fetch from".to_owned())
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
    fn a_message_hash_names_its_bytes() {
        assert_eq!(
            delivery_info("0x0a0bff"),
            Some(DeliveryInfo {
                delivery_msg_id: DeliveryMsgId::from(vec![0x0a, 0x0b, 0xff]),
            })
        );
    }

    #[test]
    fn an_unreadable_message_hash_names_nothing() {
        assert_eq!(delivery_info("0a0bff"), None);
        assert_eq!(delivery_info("0xzz"), None);
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
