//! Background workers bridging delivery_module and the libchat client.
//!
//! Two workers run alongside the client's own inbound worker:
//! * The bridge ([`run_bridge`]) drains delivery_module's events: `messageReceived`
//!   payloads are pushed to the client's inbound channel; `nodeStarted` and
//!   `connectionStateChanged` drive local `delivery_state`; `messageError` for a
//!   send of this module's that never reached the network is reported as
//!   `delivery_send_failed`; and the core's queued subscription requests are
//!   forwarded to delivery_module once its node is started.
//! * The event consumer ([`run_events`]) drains the client's `Event` stream and
//!   records each observation in the display history, emitting the matching plugin
//!   events.
//!
//! The delivery_module subscriptions are set up in `init` (before the node starts,
//! so the events emitted during start aren't missed) and the resulting
//! `EventSubscription`s are moved into the bridge.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use logos_generic_chat::{ConversationClass, Event};
use logos_rust_sdk::{EventData, EventSubscription};

use crate::actions::{
    record_conversation_started, record_members_changed, record_message_received,
    record_node_started, set_delivery_state,
};
use crate::delivery::report_send_failed;
use crate::module::{with_display, with_display_mut, DeliveryStateKind};
use crate::persistence::ConversationKind;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What the bridge hears about sends: the request id of each one delivery_module
/// accepted from this module, and delivery_module's `messagePropagated` and
/// `messageError`, which every subscriber gets for every module's sends.
pub(crate) struct SendEvents {
    pub(crate) accepted: Receiver<String>,
    pub(crate) propagated: EventSubscription,
    pub(crate) failed: EventSubscription,
}

pub(crate) fn spawn_bridge(
    stop: Arc<AtomicBool>,
    messages: EventSubscription,
    node_started: EventSubscription,
    conn: Option<EventSubscription>,
    inbound_tx: Sender<Vec<u8>>,
    subscribe_rx: Receiver<String>,
    sends: SendEvents,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("rust-chat-bridge".into())
        .spawn(move || {
            run_bridge(
                stop,
                messages,
                node_started,
                conn,
                inbound_tx,
                subscribe_rx,
                sends,
            )
        })
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
    messages: EventSubscription,
    node_started: EventSubscription,
    mut conn: Option<EventSubscription>,
    inbound_tx: Sender<Vec<u8>>,
    subscribe_rx: Receiver<String>,
    sends: SendEvents,
) {
    let mut node_started = Some(node_started);
    let mut seen = SeenMessages::default();
    let mut propagated = Some(sends.propagated);
    let mut failed = Some(sends.failed);
    let mut outcomes = SendOutcomes::default();
    while !stop.load(Ordering::Relaxed) {
        match messages.receiver().recv_timeout(POLL_INTERVAL) {
            Ok(evt) => forward_message(&evt, &inbound_tx, &mut seen),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
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

        while let Ok(request_id) = sends.accepted.try_recv() {
            if let Some(reason) = outcomes.accepted(request_id) {
                report_send_failed(&reason);
            }
        }
        drain(&mut propagated, |evt| {
            let Some(propagated) =
                crate::delivery_module::DeliveryModuleClient::decode_message_propagated(evt)
            else {
                tracing::warn!("inbound: messagePropagated payload missing or malformed");
                return;
            };
            outcomes.propagated(propagated.request_id);
        });
        drain(&mut failed, |evt| {
            let Some(failed) =
                crate::delivery_module::DeliveryModuleClient::decode_message_error(evt)
            else {
                tracing::warn!("inbound: messageError payload missing or malformed");
                return;
            };
            if outcomes.failed(failed.request_id, &failed.error) {
                report_send_failed(&failed.error);
            }
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
/// channel. The loose topic-prefix filter stays: libp2p delivers every message in
/// the shard regardless of subscribed topic, so non-chat traffic is dropped here
/// and `Core::handle_payload` (inside the client) discriminates the rest.
fn forward_message(evt: &EventData, inbound_tx: &Sender<Vec<u8>>, seen: &mut SeenMessages) {
    let Some(msg) = crate::delivery_module::DeliveryModuleClient::decode_message_received(evt)
    else {
        tracing::warn!("inbound: messageReceived payload missing or malformed");
        return;
    };
    if !msg.content_topic.starts_with(crate::delivery::TOPIC_PREFIX) {
        return;
    }
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
    let _ = inbound_tx.send(msg.payload);
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

/// delivery_module's send service tracks at most 1000 sends at once and refuses
/// the rest (`DefaultMaxTaskCacheSize`), and a send has at most two outcomes,
/// `messagePropagated` and then `messageError`. This holds both for every send
/// in flight while the answer naming one of them is on its way.
const UNCLAIMED_OUTCOMES: usize = 2 * 1_000;

/// What delivery_module reported for a send before it was known to be this
/// module's.
enum SendOutcome {
    Propagated,
    Failed(String),
}

/// Which of delivery_module's send failures are this module's to report: those
/// of a request it accepted from here that never reached the network. An
/// outcome can arrive before the answer that names its request.
#[derive(Default)]
struct SendOutcomes {
    /// Requests accepted from this module, neither propagated nor failed yet.
    pending: HashSet<String>,
    /// Outcomes of requests not known to be this module's, oldest first.
    unclaimed: VecDeque<(String, SendOutcome)>,
}

impl SendOutcomes {
    /// Record a request delivery_module accepted from this module. Returns the
    /// reason when it has already failed.
    fn accepted(&mut self, request_id: String) -> Option<String> {
        let Some(at) = self.unclaimed.iter().position(|(id, _)| *id == request_id) else {
            self.pending.insert(request_id);
            return None;
        };
        match self.unclaimed.remove(at) {
            Some((_, SendOutcome::Failed(reason))) => Some(reason),
            _ => None,
        }
    }

    /// Record a request that reached the network. delivery_module fails it later
    /// when no store node confirms it, which is not a failed send.
    fn propagated(&mut self, request_id: String) {
        if !self.pending.remove(&request_id) {
            self.hold(request_id, SendOutcome::Propagated);
        }
    }

    /// Record a failed request. Returns whether it is this module's to report.
    fn failed(&mut self, request_id: String, reason: &str) -> bool {
        if self.pending.remove(&request_id) {
            return true;
        }
        self.hold(request_id, SendOutcome::Failed(reason.to_owned()));
        false
    }

    fn hold(&mut self, request_id: String, outcome: SendOutcome) {
        if self.unclaimed.len() == UNCLAIMED_OUTCOMES {
            self.unclaimed.pop_front();
        }
        self.unclaimed.push_back((request_id, outcome));
    }
}

/// Forward the core's queued subscription requests to delivery_module, but only
/// once its node is online. `subscribe` rejects until the node is started, so the
/// requests queued at client construction wait in the channel until then; later
/// requests are forwarded as they arrive.
fn forward_subscriptions(subscribe_rx: &Receiver<String>) {
    if with_display(|d| d.delivery_state.state) != DeliveryStateKind::Online {
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
/// shape (DirectV1) is `direct`, GroupV2 is `group`.
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

/// Unknown statuses map to `Error` so a degraded state isn't silently
/// reported as healthy.
pub(crate) fn map_connection_status(status: &str) -> DeliveryStateKind {
    match status {
        "Connected" | "PartiallyConnected" => DeliveryStateKind::Online,
        _ => DeliveryStateKind::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_delivery_message_with_source_field() {
        let payload = b"chat invite";
        let event = EventData::new(
            "messageReceived",
            serde_json::json!([
                "hash",
                "/logos-chat/1/alice/proto",
                logos_rust_sdk::bytes::encode(payload),
                "live",
                42,
            ]),
        );
        let (tx, rx) = crossbeam_channel::unbounded();

        forward_message(&event, &tx, &mut SeenMessages::default());

        assert_eq!(rx.try_recv().unwrap(), payload);
    }

    #[test]
    fn a_message_is_forwarded_once() {
        let mut seen = SeenMessages::default();

        assert!(seen.first_sighting("0xa"));
        assert!(seen.first_sighting("0xb"));
        assert!(!seen.first_sighting("0xa"));
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
    fn a_failure_of_an_accepted_request_is_reported_once() {
        let mut outcomes = SendOutcomes::default();
        assert_eq!(outcomes.accepted("ours".into()), None);

        assert!(outcomes.failed("ours".into(), "no peers"));
        assert!(!outcomes.failed("ours".into(), "no peers"));
    }

    /// delivery_module emits `messageError` to every subscriber, whoever sent.
    #[test]
    fn a_failure_of_another_modules_request_is_not() {
        let mut outcomes = SendOutcomes::default();
        assert_eq!(outcomes.accepted("ours".into()), None);

        assert!(!outcomes.failed("theirs".into(), "no peers"));
    }

    #[test]
    fn a_failure_that_arrives_before_its_request_id_is_kept_for_it() {
        let mut outcomes = SendOutcomes::default();
        assert!(!outcomes.failed("ours".into(), "no peers"));

        assert_eq!(outcomes.accepted("ours".into()), Some("no peers".into()));
        assert_eq!(outcomes.accepted("ours".into()), None);
    }

    /// delivery_module raises `messageError` for a message that left and that no
    /// store node confirmed, which its peers may well have received.
    #[test]
    fn a_failure_after_the_request_reached_the_network_is_not_reported() {
        let mut outcomes = SendOutcomes::default();
        assert_eq!(outcomes.accepted("ours".into()), None);
        outcomes.propagated("ours".into());

        assert!(!outcomes.failed("ours".into(), "no store confirmation"));
    }

    #[test]
    fn propagation_that_arrives_before_its_request_id_settles_it() {
        let mut outcomes = SendOutcomes::default();
        outcomes.propagated("ours".into());
        assert_eq!(outcomes.accepted("ours".into()), None);

        assert!(!outcomes.failed("ours".into(), "no store confirmation"));
    }

    #[test]
    fn the_oldest_unclaimed_outcome_is_forgotten_at_capacity() {
        let mut outcomes = SendOutcomes::default();
        for i in 0..=UNCLAIMED_OUTCOMES {
            assert!(!outcomes.failed(i.to_string(), "no peers"));
        }

        assert_eq!(outcomes.accepted("0".into()), None);
        assert_eq!(outcomes.accepted("1".into()), Some("no peers".into()));
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
            DeliveryStateKind::Error
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
            Some(DeliveryStateKind::Error)
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
