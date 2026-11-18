//! Background workers bridging delivery_module and the libchat client.
//!
//! Two workers run alongside the client's own inbound worker:
//! * The bridge ([`run_bridge`]) drains delivery_module's events: `messageReceived`
//!   payloads, and those a fetch found in Store, are pushed to the client's
//!   inbound channel; `nodeStarted` and
//!   `connectionStateChanged` drive local `delivery_state`; `messagePropagated` and
//!   `messageError` for this module's sends are reported to the client; and the
//!   core's queued subscription requests are forwarded to delivery_module once its
//!   node is started.
//! * The event consumer ([`run_events`]) drains the client's `Event` stream and
//!   records each observation in the display history, emitting the matching plugin
//!   events.
//!
//! The delivery_module subscriptions are set up in `init` (before the node starts,
//! so the events emitted during start aren't missed) and the resulting
//! `EventSubscription`s are moved into the bridge.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use logos_generic_chat::{
    ConversationClass, DeliveryReport, Event, InboundPayload, PublishReceipt,
};
use logos_rust_sdk::{EventData, EventSubscription};

use crate::actions::{
    record_conversation_started, record_members_changed, record_message_received,
    record_node_started, set_delivery_state,
};
use crate::delivery_module::MessageReceivedEvent;
use crate::module::{with_display, with_display_mut, DeliveryStateKind};
use crate::persistence::ConversationKind;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What the bridge hears about sends: the request id delivery_module accepted
/// each of this module's sends under, with the receipt `publish` returned for
/// it, and delivery_module's `messagePropagated` and `messageError`, which every
/// subscriber gets for every module's sends. `reports` is where the client
/// learns the outcome.
pub(crate) struct SendEvents {
    pub(crate) accepted: Receiver<(String, PublishReceipt)>,
    pub(crate) propagated: EventSubscription,
    pub(crate) failed: EventSubscription,
    pub(crate) reports: Sender<DeliveryReport>,
}

/// What the bridge forwards to the client: delivery_module's `messageReceived`,
/// and the messages a fetch found in Store. `inbound_tx` is the client's inbound
/// channel.
pub(crate) struct ReceiveEvents {
    pub(crate) messages: EventSubscription,
    pub(crate) fetched: Receiver<MessageReceivedEvent>,
    pub(crate) inbound_tx: Sender<InboundPayload>,
}

pub(crate) fn spawn_bridge(
    stop: Arc<AtomicBool>,
    receives: ReceiveEvents,
    node_started: EventSubscription,
    conn: Option<EventSubscription>,
    subscribe_rx: Receiver<String>,
    sends: SendEvents,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("rust-chat-bridge".into())
        .spawn(move || run_bridge(stop, receives, node_started, conn, subscribe_rx, sends))
        .expect("failed to spawn bridge thread")
}

pub(crate) fn spawn_events(events: Receiver<Event>) -> JoinHandle<()> {
    thread::Builder::new()
        .name("rust-chat-events".into())
        .spawn(move || run_events(events))
        .expect("failed to spawn events thread")
}

fn run_bridge(
    stop: Arc<AtomicBool>,
    receives: ReceiveEvents,
    node_started: EventSubscription,
    mut conn: Option<EventSubscription>,
    subscribe_rx: Receiver<String>,
    sends: SendEvents,
) {
    let ReceiveEvents {
        messages,
        fetched,
        inbound_tx,
    } = receives;
    let mut node_started = Some(node_started);
    let mut seen = SeenMessages::default();
    let mut propagated = Some(sends.propagated);
    let mut failed = Some(sends.failed);
    let mut outcomes = SendOutcomes::default();
    // The receiver is the client's worker; if it has gone away the client is being
    // dropped and the bridge is about to stop, so a failed send is benign.
    let report = |report: Option<DeliveryReport>| {
        if let Some(report) = report {
            let _ = sends.reports.send(report);
        }
    };
    while !stop.load(Ordering::Relaxed) {
        match messages.receiver().recv_timeout(POLL_INTERVAL) {
            Ok(evt) => forward_message(&evt, &inbound_tx, &mut seen),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(msg) = fetched.try_recv() {
            forward_unseen(msg, &inbound_tx, &mut seen);
        }

        drain(&mut node_started, |evt| {
            if let Some(started) =
                crate::delivery_module::DeliveryModuleClient::decode_node_started(evt)
            {
                with_display_mut(|d| record_node_started(d, started.success, &started.message));
            }
        });
        drain(&mut conn, |evt| {
            if let Some(state) =
                crate::delivery_module::DeliveryModuleClient::decode_connection_state_changed(evt)
            {
                handle_connection_state(&state.connection_status);
            }
        });

        while let Ok((request_id, receipt)) = sends.accepted.try_recv() {
            report(outcomes.accepted(request_id, receipt));
        }
        drain(&mut propagated, |evt| {
            let Some(propagated) =
                crate::delivery_module::DeliveryModuleClient::decode_message_propagated(evt)
            else {
                tracing::warn!("inbound: messagePropagated payload missing or malformed");
                return;
            };
            report(outcomes.propagated(propagated.request_id, propagated.message_hash));
        });
        drain(&mut failed, |evt| {
            let Some(failed) =
                crate::delivery_module::DeliveryModuleClient::decode_message_error(evt)
            else {
                tracing::warn!("inbound: messageError payload missing or malformed");
                return;
            };
            report(outcomes.failed(failed.request_id, failed.error));
        });

        forward_subscriptions(&subscribe_rx);
    }
}

/// Hand each pending event of `sub` to `handle`. On Disconnected, drop the
/// subscription so we stop re-polling a dead one.
fn drain(sub: &mut Option<EventSubscription>, mut handle: impl FnMut(&EventData)) {
    let Some(events) = sub else {
        return;
    };
    let disconnected = loop {
        match events.receiver().try_recv() {
            Ok(evt) => handle(&evt),
            Err(TryRecvError::Empty) => break false,
            Err(TryRecvError::Disconnected) => break true,
        }
    };
    if disconnected {
        *sub = None;
    }
}

/// Decode a `messageReceived` event and push its payload to the client's inbound
/// channel. Only chat topics pass: delivery_module's node serves every module that
/// loads it, and its `messageReceived` carries each topic any of them subscribed.
/// `Core::handle_payload` (inside the client) discriminates the rest.
fn forward_message(evt: &EventData, inbound_tx: &Sender<InboundPayload>, seen: &mut SeenMessages) {
    let Some(msg) = crate::delivery_module::DeliveryModuleClient::decode_message_received(evt)
    else {
        tracing::warn!("inbound: messageReceived payload missing or malformed");
        return;
    };
    if !msg.content_topic.starts_with(crate::delivery::TOPIC_PREFIX) {
        return;
    }
    forward_unseen(msg, inbound_tx, seen);
}

/// Push a message's payload to the client's inbound channel, once per hash.
fn forward_unseen(
    msg: MessageReceivedEvent,
    inbound_tx: &Sender<InboundPayload>,
    seen: &mut SeenMessages,
) {
    if !seen.first_sighting(&msg.message_hash) {
        tracing::debug!(
            "inbound: dropped a repeat of {} ({})",
            msg.message_hash,
            msg.source
        );
        return;
    }
    // The receiver is the client's worker; if it has gone away the client is being
    // dropped and the bridge is about to stop, so a failed send is benign.
    let _ = inbound_tx.send(InboundPayload {
        data: msg.payload,
        delivery_info: crate::delivery::delivery_info(&msg.message_hash),
    });
}

const SEEN_CAPACITY: usize = 10_000;

/// Recently forwarded message hashes. delivery_module re-delivers a message
/// after its own few-minute dedupe window, e.g. on Store catch-up.
#[derive(Default)]
struct SeenMessages {
    hashes: HashSet<String>,
    order: VecDeque<String>,
}

impl SeenMessages {
    fn first_sighting(&mut self, hash: &str) -> bool {
        if self.hashes.contains(hash) {
            return false;
        }
        if self.order.len() == SEEN_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.hashes.remove(&oldest);
            }
        }
        self.hashes.insert(hash.to_owned());
        self.order.push_back(hash.to_owned());
        true
    }
}

/// An early outcome waits for one send's reply, so the queue needs room for what
/// every module's sends report in that time. delivery_module holds at most 1000
/// sends at once by default (`DefaultMaxTaskCacheSize`) and reports at most two
/// outcomes for each, `messagePropagated` and then `messageError`. Outcomes never
/// claimed, other modules' and the late failure of a propagated send, age out.
const UNCLAIMED_OUTCOMES: usize = 2 * 1_000;

/// What delivery_module reported for a send.
enum SendOutcome {
    /// It reached the network as the message with this hash.
    Propagated(String),
    /// delivery_module gave up on it before it propagated, for this reason. A
    /// mixed send can fail after its exit published it.
    Failed(String),
}

impl SendOutcome {
    /// What the client learns of this module's send under `receipt`.
    fn report(self, receipt: PublishReceipt) -> DeliveryReport {
        let delivery_info = match self {
            SendOutcome::Propagated(message_hash) => crate::delivery::delivery_info(&message_hash),
            SendOutcome::Failed(reason) => {
                tracing::warn!("delivery_module failed a send: {reason}");
                None
            }
        };
        DeliveryReport {
            receipt,
            delivery_info,
        }
    }
}

/// Matches delivery_module's send outcomes to the receipts of this module's
/// sends. An outcome can arrive before the answer that names its request.
#[derive(Default)]
struct SendOutcomes {
    /// This module's sends, neither propagated nor failed yet.
    pending: HashMap<String, PublishReceipt>,
    /// Outcomes of requests not known to be this module's, oldest first.
    unclaimed: VecDeque<(String, SendOutcome)>,
}

impl SendOutcomes {
    /// Record a send delivery_module accepted from this module under
    /// `request_id`. Returns its report when its outcome has already arrived.
    fn accepted(&mut self, request_id: String, receipt: PublishReceipt) -> Option<DeliveryReport> {
        let Some(at) = self.unclaimed.iter().position(|(id, _)| *id == request_id) else {
            self.pending.insert(request_id, receipt);
            return None;
        };
        let (_, outcome) = self.unclaimed.remove(at)?;
        Some(outcome.report(receipt))
    }

    /// Record a request that reached the network as `message_hash`. Returns the
    /// report for this module's send.
    fn propagated(&mut self, request_id: String, message_hash: String) -> Option<DeliveryReport> {
        self.settle(request_id, SendOutcome::Propagated(message_hash))
    }

    /// Record a failed request. Returns the report for this module's send that
    /// had not propagated. delivery_module also fails a request that did, once no
    /// store node confirms it, which by then is no longer pending.
    fn failed(&mut self, request_id: String, reason: String) -> Option<DeliveryReport> {
        self.settle(request_id, SendOutcome::Failed(reason))
    }

    fn settle(&mut self, request_id: String, outcome: SendOutcome) -> Option<DeliveryReport> {
        let Some(receipt) = self.pending.remove(&request_id) else {
            self.hold(request_id, outcome);
            return None;
        };
        Some(outcome.report(receipt))
    }

    fn hold(&mut self, request_id: String, outcome: SendOutcome) {
        if self.unclaimed.len() == UNCLAIMED_OUTCOMES {
            self.unclaimed.pop_front();
        }
        self.unclaimed.push_back((request_id, outcome));
    }
}

/// Forward the core's queued subscription requests to delivery_module, but only
/// once its node has started. `subscribe` rejects until then, so the requests
/// queued at client construction wait in the channel; later requests are
/// forwarded as they arrive, online or offline.
fn forward_subscriptions(subscribe_rx: &Receiver<String>) {
    if !with_display(|d| d.delivery_state.started) {
        return;
    }
    while let Ok(topic) = subscribe_rx.try_recv() {
        crate::modules()
            .delivery_module
            .subscribe_async(&topic, move |res| {
                if let Err(e) = crate::delivery::delivery_outcome(res) {
                    tracing::error!("delivery_module.subscribe failed: {e}");
                }
            });
    }
}

/// Drain the client's event stream until the client is dropped (the sender
/// disconnects and the iterator ends). Each event is recorded in the display
/// history, which re-emits it to consumers over IPC.
fn run_events(events: Receiver<Event>) {
    for event in events {
        match event {
            Event::ConversationStarted { convo_id, class } => {
                record_conversation_started(&convo_id, kind_for_class(class));
            }
            Event::MessageReceived {
                convo_id,
                content,
                sender,
                ..
            } => {
                // The client delivers only senders their account's log vouches for.
                record_message_received(
                    &convo_id,
                    &content,
                    &sender.account().to_string(),
                    &sender.signer().to_string(),
                );
            }
            Event::ConversationMembersChanged { convo_id } => {
                record_members_changed(&convo_id);
            }
            Event::InboundError { message } => {
                tracing::warn!("inbound error: {message}");
            }
            // `Event` is `#[non_exhaustive]`; ignore variants added upstream.
            _ => {}
        }
    }
}

/// Map libchat's display class to the module's contract kind: the pairwise
/// shape (DirectV1) is `direct`, a group is `group`.
fn kind_for_class(class: ConversationClass) -> ConversationKind {
    match class {
        ConversationClass::Dm => ConversationKind::Direct,
        ConversationClass::Group => ConversationKind::Group,
    }
}

fn handle_connection_state(status: &str) {
    // delivery_module's `connectionStateChanged` carries only a status (its
    // second field is a timestamp, not a human detail), so detail stays empty.
    with_display_mut(|d| {
        if let Some(next) = connection_transition(d.delivery_state.started, status) {
            set_delivery_state(d, next, "");
        }
    });
}

/// The delivery state to move to for an upstream connectivity `status`, or
/// `None` to ignore the event. Until this init's bootstrap has `started` the
/// node, connectivity is ignored: delivery reports `Connected` mid-bootstrap,
/// ~tens of seconds before the transport can service a call, so readiness is
/// gated on the start handshake (see `actions::start_delivery_bootstrap`) — not
/// on this event. After a failed bootstrap it reports a node another consumer
/// started, which this module never joined. Once started, connectivity drives
/// online/offline for reconnect handling.
pub(crate) fn connection_transition(started: bool, status: &str) -> Option<DeliveryStateKind> {
    if !started {
        return None;
    }
    Some(map_connection_status(status))
}

/// `Disconnected` means a subscribed shard has no mesh peer, which happens for a
/// few seconds after a start. Unknown statuses map to `Error`
/// so a degraded state isn't silently reported as healthy.
pub(crate) fn map_connection_status(status: &str) -> DeliveryStateKind {
    match status {
        "Connected" | "PartiallyConnected" => DeliveryStateKind::Online,
        "Disconnected" => DeliveryStateKind::Offline,
        _ => DeliveryStateKind::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use logos_generic_chat::{DeliveryInfo, DeliveryMsgId};

    #[test]
    fn forwards_delivery_message_with_source_field() {
        let payload = b"chat invite";
        let event = EventData::new(
            "messageReceived",
            serde_json::json!([
                "0x0a0b",
                "/logos-chat/1/alice/proto",
                logos_rust_sdk::bytes::encode(payload),
                "live",
                42,
            ]),
        );
        let (tx, rx) = crossbeam_channel::unbounded();

        forward_message(&event, &tx, &mut SeenMessages::default());

        let inbound = rx.try_recv().unwrap();
        assert_eq!(inbound.data, payload);
        assert_eq!(inbound.delivery_info, Some(info(&[0x0a, 0x0b])));
    }

    fn receipt(n: u8) -> PublishReceipt {
        PublishReceipt::from(vec![n])
    }

    fn info(bytes: &[u8]) -> DeliveryInfo {
        DeliveryInfo {
            delivery_msg_id: DeliveryMsgId::from(bytes.to_vec()),
        }
    }

    /// The receipt a report is under and the delivery id it names.
    fn names(report: Option<DeliveryReport>) -> Option<(PublishReceipt, Option<DeliveryInfo>)> {
        report.map(|r| (r.receipt, r.delivery_info))
    }

    #[test]
    fn a_propagated_send_reports_its_message_hash() {
        let mut outcomes = SendOutcomes::default();
        assert!(outcomes.accepted("ours".into(), receipt(1)).is_none());

        assert_eq!(
            names(outcomes.propagated("ours".into(), "0x0a".into())),
            Some((receipt(1), Some(info(&[0x0a]))))
        );
        assert!(outcomes
            .failed("ours".into(), "no store confirmation".into())
            .is_none());
    }

    #[test]
    fn a_failed_send_reports_no_delivery_id() {
        let mut outcomes = SendOutcomes::default();
        assert!(outcomes.accepted("ours".into(), receipt(1)).is_none());

        assert_eq!(
            names(outcomes.failed("ours".into(), "no peers".into())),
            Some((receipt(1), None))
        );
        assert!(outcomes.failed("ours".into(), "no peers".into()).is_none());
    }

    /// delivery_module emits its send events to every subscriber, whoever sent.
    #[test]
    fn another_modules_send_reports_nothing() {
        let mut outcomes = SendOutcomes::default();
        assert!(outcomes.accepted("ours".into(), receipt(1)).is_none());

        assert!(outcomes
            .propagated("theirs".into(), "0x0a".into())
            .is_none());
        assert!(outcomes
            .failed("theirs".into(), "no peers".into())
            .is_none());
    }

    #[test]
    fn an_outcome_that_arrives_before_its_request_id_is_kept_for_it() {
        let mut outcomes = SendOutcomes::default();
        assert!(outcomes.propagated("first".into(), "0x0a".into()).is_none());
        assert!(outcomes
            .failed("second".into(), "no peers".into())
            .is_none());

        assert_eq!(
            names(outcomes.accepted("second".into(), receipt(2))),
            Some((receipt(2), None))
        );
        assert_eq!(
            names(outcomes.accepted("first".into(), receipt(1))),
            Some((receipt(1), Some(info(&[0x0a]))))
        );
    }

    /// Propagation precedes a failure for want of store confirmation, so the
    /// earlier outcome is the one a late request id takes.
    #[test]
    fn a_late_request_id_takes_its_first_outcome() {
        let mut outcomes = SendOutcomes::default();
        assert!(outcomes.propagated("ours".into(), "0x0a".into()).is_none());
        assert!(outcomes
            .failed("ours".into(), "no store confirmation".into())
            .is_none());

        assert_eq!(
            names(outcomes.accepted("ours".into(), receipt(1))),
            Some((receipt(1), Some(info(&[0x0a]))))
        );
    }

    #[test]
    fn the_oldest_unclaimed_outcome_is_forgotten_at_capacity() {
        let mut outcomes = SendOutcomes::default();
        for i in 0..=UNCLAIMED_OUTCOMES {
            assert!(outcomes.failed(i.to_string(), "no peers".into()).is_none());
        }

        assert!(outcomes.accepted("0".into(), receipt(0)).is_none());
        assert_eq!(
            names(outcomes.accepted("1".into(), receipt(1))),
            Some((receipt(1), None))
        );
    }

    #[test]
    fn a_message_is_forwarded_once() {
        let mut seen = SeenMessages::default();

        assert!(seen.first_sighting("0xa"));
        assert!(seen.first_sighting("0xb"));
        assert!(!seen.first_sighting("0xa"));
    }

    fn fetched(hash: &str) -> MessageReceivedEvent {
        MessageReceivedEvent {
            message_hash: hash.into(),
            content_topic: "/logos-chat/1/alice/proto".into(),
            payload: b"hi".to_vec(),
            source: "fetch".into(),
            timestamp: 42,
        }
    }

    #[test]
    fn a_fetched_message_reaches_the_client_once() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut seen = SeenMessages::default();

        forward_unseen(fetched("0x0a0b"), &tx, &mut seen);
        forward_unseen(fetched("0x0a0b"), &tx, &mut seen);

        let inbound = rx.try_recv().unwrap();
        assert_eq!(inbound.data, b"hi");
        assert_eq!(inbound.delivery_info, Some(info(&[0x0a, 0x0b])));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_live_copy_after_a_fetch_is_a_repeat() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut seen = SeenMessages::default();

        forward_unseen(fetched("0x0a0b"), &tx, &mut seen);

        assert!(!seen.first_sighting("0x0a0b"));
    }

    #[test]
    fn the_oldest_hash_is_forgotten_at_capacity() {
        let mut seen = SeenMessages::default();
        for i in 0..SEEN_CAPACITY {
            assert!(seen.first_sighting(&i.to_string()));
        }

        assert!(seen.first_sighting("one more"));
        assert!(seen.first_sighting("0"));
        assert!(!seen.first_sighting(&(SEEN_CAPACITY - 1).to_string()));
    }

    #[test]
    fn connection_status_maps_each_upstream_variant() {
        assert_eq!(
            map_connection_status("Connected"),
            DeliveryStateKind::Online
        );
        assert_eq!(
            map_connection_status("PartiallyConnected"),
            DeliveryStateKind::Online
        );
        assert_eq!(
            map_connection_status("Disconnected"),
            DeliveryStateKind::Offline
        );
    }

    #[test]
    fn connection_status_unknown_maps_to_error() {
        assert_eq!(map_connection_status(""), DeliveryStateKind::Error);
        assert_eq!(
            map_connection_status("Reconnecting"),
            DeliveryStateKind::Error
        );
    }

    #[test]
    fn connectivity_ignored_until_started() {
        // Pre-startup, `Connected` fires mid-bootstrap and must NOT promote to
        // Online — readiness is gated on the start handshake. After a failed
        // bootstrap it is another consumer's node, and must not either.
        assert_eq!(connection_transition(false, "Connected"), None);
        assert_eq!(connection_transition(false, "Disconnected"), None);
    }

    #[test]
    fn connectivity_drives_state_once_started() {
        // After startup, connectivity drives online/offline for reconnect.
        assert_eq!(
            connection_transition(true, "Disconnected"),
            Some(DeliveryStateKind::Offline)
        );
        assert_eq!(
            connection_transition(true, "Connected"),
            Some(DeliveryStateKind::Online)
        );
    }

    #[test]
    fn class_maps_to_contract_kind() {
        assert_eq!(
            kind_for_class(ConversationClass::Dm),
            ConversationKind::Direct
        );
        assert_eq!(
            kind_for_class(ConversationClass::Group),
            ConversationKind::Group
        );
    }
}
