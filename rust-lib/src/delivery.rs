//! `Transport` impl bridging the client's delivery boundary to delivery_module.
//!
//! `publish` forwards each outbound envelope to delivery_module; `subscribe` sends
//! the core's interest in a delivery address to delivery_module, queued until the
//! node has started (see `inbound.rs`); `inbound` hands the client the channel the
//! module feeds with received payloads, and `delivery_reports` the one it feeds
//! with the ids of published payloads. `fetch_messages` looks payloads up by hash
//! on the Store nodes delivery_module names, and hands what it finds to the bridge.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use crossbeam_channel::{Receiver, Sender};
use logos_generic_chat::{
    AddressedEnvelope, DeliveryInfo, DeliveryMsgId, DeliveryReport, DeliveryService,
    InboundPayload, PublishReceipt, PublishedId, Transport,
};
use logos_rust_sdk::LogosError;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::delivery_module::MessageReceivedEvent;

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
    /// Subscription requests the core makes before the node has started, drained
    /// by the inbound worker once it has.
    subscribe_tx: Sender<String>,
    /// The subscriptions sent to delivery_module, until it answers them.
    subscribes: Subscribes,
    /// The message hash delivery_module returned for each send, with the receipt
    /// `publish` returned for it, for the inbound worker to match against what
    /// delivery_module reports of the send later.
    accepted_tx: Sender<(String, PublishReceipt)>,
    /// Where a send delivery_module refused is reported.
    reports_tx: Sender<DeliveryReport>,
    /// Where the messages a fetch finds go, for the bridge to forward.
    fetched_tx: Sender<MessageReceivedEvent>,
    /// Numbers the receipts `publish` returns.
    sends: Arc<AtomicU64>,
}

impl SdkPublisher {
    pub(crate) fn new(
        subscribe_tx: Sender<String>,
        subscribes: Subscribes,
        accepted_tx: Sender<(String, PublishReceipt)>,
        reports_tx: Sender<DeliveryReport>,
        fetched_tx: Sender<MessageReceivedEvent>,
    ) -> Self {
        Self {
            subscribe_tx,
            subscribes,
            accepted_tx,
            reports_tx,
            fetched_tx,
            sends: Arc::default(),
        }
    }
}

/// The subscriptions sent to delivery_module that it has not answered yet. The
/// event consumer lists a conversation the core reports started only once this
/// is empty (`inbound::run_events`), so the conversation's topic is live by
/// then.
#[derive(Clone, Debug, Default)]
pub(crate) struct Subscribes(Arc<(Mutex<SubscribesState>, Condvar)>);

#[derive(Debug, Default)]
struct SubscribesState {
    unanswered: usize,
    closed: bool,
}

impl Subscribes {
    /// Subscribes `topic` with delivery_module, counted until it answers.
    pub(crate) fn send(&self, topic: &str) {
        self.sent();
        let subscribes = self.clone();
        crate::modules()
            .delivery_module
            .subscribe_async(topic, move |res| {
                if let Err(e) = delivery_outcome(res) {
                    tracing::error!("delivery_module.subscribe failed: {e}");
                }
                subscribes.answered();
            });
    }

    /// Returns once delivery_module has answered every subscription sent so far,
    /// or once [`Self::close`] is called.
    pub(crate) fn wait_answered(&self) {
        let (state, changed) = &*self.0;
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        let _state = changed
            .wait_while(state, |s| s.unanswered > 0 && !s.closed)
            .unwrap_or_else(|e| e.into_inner());
    }

    /// Ends every wait for good. Shutdown calls it before joining the event
    /// consumer: delivery_module answers on the thread shutdown blocks.
    pub(crate) fn close(&self) {
        self.update(|s| s.closed = true);
    }

    fn sent(&self) {
        self.update(|s| s.unanswered += 1);
    }

    fn answered(&self) {
        self.update(|s| s.unanswered = s.unanswered.saturating_sub(1));
    }

    fn update(&self, change: impl FnOnce(&mut SubscribesState)) {
        let (state, changed) = &*self.0;
        change(&mut state.lock().unwrap_or_else(|e| e.into_inner()));
        changed.notify_all();
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
        // becomes of the payload is reported under it: its id once delivery_module
        // reports the message hash the callback receives as propagated, or none
        // when the send fails.
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
                    Ok(Value::String(message_hash)) => {
                        if accepted_tx.send((message_hash, sent.clone())).is_ok() {
                            return;
                        }
                        // The bridge has stopped, so nothing else will report the send.
                        tracing::warn!("delivery_module accepted a send nothing will report");
                    }
                    Ok(other) => {
                        tracing::error!("delivery_module.send returned no message hash: {other}")
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
        let topic = content_topic_for(delivery_address);
        // delivery_module refuses a subscribe until its node has started, and the
        // core subscribes its inbound addresses at construction, before the node
        // exists. Those wait for the inbound worker (`inbound::forward_subscriptions`).
        if !crate::module::with_display(|d| d.delivery_state.started) {
            return self.subscribe_tx.send(topic).map_err(|e| e.to_string());
        }
        // The core subscribes a conversation it joins before it reports it, so a
        // subscribe sent now is counted before the conversation is listed.
        self.subscribes.send(&topic);
        Ok(())
    }

    fn fetch_messages(
        &mut self,
        _: &str,
        delivery_msg_ids: &[DeliveryMsgId],
    ) -> Result<(), String> {
        // A store query without hashes matches every message.
        if delivery_msg_ids.is_empty() {
            return Ok(());
        }
        let fetch = Fetch {
            query: store_query(delivery_msg_ids),
            fetched_tx: self.fetched_tx.clone(),
        };
        crate::modules()
            .delivery_module
            .get_node_info_async("StoreNodes", move |res| {
                match delivery_outcome(res).and_then(|nodes| store_nodes(&nodes)) {
                    Ok(nodes) => fetch.ask(nodes),
                    Err(e) => tracing::warn!("fetch: delivery_module named no Store node: {e}"),
                }
            });
        Ok(())
    }
}

/// How long one Store node gets to answer, as libchat's own delivery adapter
/// gives it.
const FETCH_TIMEOUT_MS: i64 = 10_000;

/// A by-hash Store lookup of the payloads libchat is missing.
struct Fetch {
    query: Value,
    fetched_tx: Sender<MessageReceivedEvent>,
}

impl Fetch {
    /// Asks the first of `nodes`, and the next once it fails. A page with a cursor
    /// sends the query for the next page to the same node.
    fn ask(mut self, mut nodes: VecDeque<String>) {
        let Some(node) = nodes.front().cloned() else {
            tracing::warn!("fetch: no Store node answered");
            return;
        };
        let query = self.query.to_string();
        crate::modules().delivery_module.store_query_async(
            &query,
            &node,
            FETCH_TIMEOUT_MS,
            move |res| match delivery_outcome(res).and_then(|page| store_page(&page)) {
                Ok(page) => {
                    for message in page
                        .messages
                        .into_iter()
                        .filter_map(StoredMessage::received)
                    {
                        // The receiver is the bridge; if it has gone away the module
                        // is stopping, so a lost fetch is benign.
                        let _ = self.fetched_tx.send(message);
                    }
                    if let Some(cursor) = page.pagination_cursor {
                        self.query["paginationCursor"] = cursor.into();
                        self.ask(nodes);
                    }
                }
                Err(e) => {
                    if let Some(failed) = nodes.pop_front() {
                        tracing::warn!("fetch: Store node {failed} failed: {e}");
                    }
                    self.ask(nodes);
                }
            },
        );
    }
}

/// The `storeQuery` JSON that returns the messages `ids` name, with their data.
fn store_query(ids: &[DeliveryMsgId]) -> Value {
    let hashes: Vec<String> = ids
        .iter()
        .map(|id| format!("0x{}", hex::encode(id.as_bytes())))
        .collect();
    // logos-delivery requires the request id and the paging direction; it fills in
    // an empty request id itself.
    json!({
        "requestId": "",
        "includeData": true,
        "paginationForward": false,
        "messageHashes": hashes,
    })
}

/// The multiaddrs delivery_module's `StoreNodes` node info lists, as JSON text.
fn store_nodes(value: &Value) -> Result<VecDeque<String>, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("StoreNodes is not text: {value}"))?;
    serde_json::from_str(text).map_err(|e| format!("StoreNodes is not a list of multiaddrs: {e}"))
}

/// One page of a `storeQuery` answer, as JSON text.
fn store_page(value: &Value) -> Result<StorePage, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("storeQuery's answer is not text: {value}"))?;
    serde_json::from_str(text).map_err(|e| format!("storeQuery's answer is unreadable: {e}"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorePage {
    messages: Vec<StoredMessage>,
    /// Set while more pages follow.
    pagination_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMessage {
    message_hash: String,
    message: Option<StoredPayload>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredPayload {
    content_topic: String,
    /// Standard base64.
    payload: String,
    #[serde(default)]
    timestamp: i64,
}

impl StoredMessage {
    /// The message as the bridge takes a received one.
    fn received(self) -> Option<MessageReceivedEvent> {
        let Some(message) = self.message else {
            tracing::warn!(
                "fetch: Store returned {} without its data",
                self.message_hash
            );
            return None;
        };
        let payload = STANDARD
            .decode(&message.payload)
            .map_err(|e| tracing::warn!("fetch: unreadable payload of {}: {e}", self.message_hash))
            .ok()?;
        Some(MessageReceivedEvent {
            message_hash: self.message_hash,
            content_topic: message.content_topic,
            payload,
            source: "fetch".to_owned(),
            timestamp: message.timestamp,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use serde_json::json;

    fn result(success: bool, value: Value, error: Value) -> Result<Value, LogosError> {
        Ok(json!({ "success": success, "value": value, "error": error }))
    }

    /// Whether `subscribes.wait_answered()` returns within `within`, waited for
    /// on another thread.
    fn wait_ends(subscribes: &Subscribes, within: Duration) -> bool {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let waiting = subscribes.clone();
        std::thread::spawn(move || {
            waiting.wait_answered();
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(within).is_ok()
    }

    #[test]
    fn a_wait_ends_once_every_subscribe_is_answered() {
        let subscribes = Subscribes::default();
        subscribes.sent();
        subscribes.sent();
        subscribes.answered();
        assert!(!wait_ends(&subscribes, Duration::from_millis(100)));
        subscribes.answered();
        assert!(wait_ends(&subscribes, Duration::from_secs(5)));
    }

    #[test]
    fn close_ends_a_wait_on_an_unanswered_subscribe() {
        let subscribes = Subscribes::default();
        subscribes.sent();
        subscribes.close();
        assert!(wait_ends(&subscribes, Duration::from_secs(5)));
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

    #[test]
    fn a_fetch_asks_for_the_data_of_each_hash() {
        let ids = [
            DeliveryMsgId::from(vec![0x0a, 0x0b]),
            DeliveryMsgId::from(vec![0xff]),
        ];
        assert_eq!(
            store_query(&ids),
            json!({
                "requestId": "",
                "includeData": true,
                "paginationForward": false,
                "messageHashes": ["0x0a0b", "0xff"],
            })
        );
    }

    #[test]
    fn store_nodes_lists_each_multiaddr_in_order() {
        let listed = json!(r#"["/dns4/a/tcp/30303/p2p/16A","/dns4/b/tcp/30303/p2p/16B"]"#);
        assert_eq!(
            store_nodes(&listed),
            Ok(VecDeque::from([
                "/dns4/a/tcp/30303/p2p/16A".to_owned(),
                "/dns4/b/tcp/30303/p2p/16B".to_owned(),
            ]))
        );
        assert!(store_nodes(&json!(["/dns4/a/tcp/30303/p2p/16A"])).is_err());
    }

    #[test]
    fn a_stored_message_reads_as_a_received_one() {
        let answer = json!({
            "requestId": "r", "statusCode": 200, "statusDesc": "OK",
            "messages": [
                {"messageHash": "0x0a0b", "pubsubTopic": "/waku/2/rs/1/0", "message": {
                    "payload": STANDARD.encode(b"hi"), "contentTopic": "/logos-chat/1/a/proto",
                    "version": 0, "timestamp": 42, "ephemeral": false}},
                {"messageHash": "0x0c0d"},
            ],
            "paginationCursor": "0x0c0d",
        });
        let page = store_page(&Value::String(answer.to_string())).unwrap();
        assert_eq!(page.pagination_cursor.as_deref(), Some("0x0c0d"));
        let received: Vec<_> = page
            .messages
            .into_iter()
            .filter_map(StoredMessage::received)
            .collect();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].message_hash, "0x0a0b");
        assert_eq!(received[0].content_topic, "/logos-chat/1/a/proto");
        assert_eq!(received[0].payload, b"hi");
        assert_eq!(received[0].timestamp, 42);
    }

    #[test]
    fn a_last_page_has_no_cursor() {
        let answer =
            json!({"requestId": "", "statusCode": 200, "statusDesc": "OK", "messages": []});
        let page = store_page(&Value::String(answer.to_string())).unwrap();
        assert!(page.messages.is_empty());
        assert_eq!(page.pagination_cursor, None);
    }
}
