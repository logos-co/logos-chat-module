//! Background workers bridging delivery_module and the libchat client.
//!
//! Two workers run alongside the client's own inbound worker:
//! * The bridge ([`run_bridge`]) drains delivery_module's events: `messageReceived`
//!   payloads are pushed to the client's inbound channel; `connectionStateChanged`
//!   drives local `delivery_state`; and the core's queued subscription requests are
//!   forwarded to delivery_module once its node is started.
//! * The event consumer ([`run_events`]) drains the client's `Event` stream and
//!   records each observation in the display history, emitting the matching plugin
//!   events.
//!
//! The delivery_module subscriptions are set up in `init` (before the node starts,
//! so the `connectionStateChanged` emitted during start isn't missed) and the
//! resulting `EventSubscription`s are moved into the bridge.

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
    set_delivery_state,
};
use crate::module::{with_display, with_display_mut, DeliveryStateKind};
use crate::persistence::ConversationKind;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn spawn_bridge(
    stop: Arc<AtomicBool>,
    messages: EventSubscription,
    conn: Option<EventSubscription>,
    inbound_tx: Sender<Vec<u8>>,
    subscribe_rx: Receiver<String>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("rust-chat-bridge".into())
        .spawn(move || run_bridge(stop, messages, conn, inbound_tx, subscribe_rx))
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
    mut conn: Option<EventSubscription>,
    inbound_tx: Sender<Vec<u8>>,
    subscribe_rx: Receiver<String>,
) {
    while !stop.load(Ordering::Relaxed) {
        match messages.receiver().recv_timeout(POLL_INTERVAL) {
            Ok(evt) => forward_message(&evt, &inbound_tx),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // On Disconnected, drop the subscription so we stop re-polling a dead one.
        let mut disconnected = false;
        if let Some(events) = conn.as_ref() {
            loop {
                match events.receiver().try_recv() {
                    Ok(evt) => {
                        if let Some(state) =
                            crate::delivery_module::DeliveryModuleClient::decode_connection_state_changed(&evt)
                        {
                            handle_connection_state(&state.status);
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            conn = None;
        }

        forward_subscriptions(&subscribe_rx);
    }
}

/// Decode a `messageReceived` event and push its payload to the client's inbound
/// channel. The loose topic-prefix filter stays: libp2p delivers every message in
/// the shard regardless of subscribed topic, so non-chat traffic is dropped here
/// and `Core::handle_payload` (inside the client) discriminates the rest.
fn forward_message(evt: &EventData, inbound_tx: &Sender<Vec<u8>>) {
    let Some(msg) = crate::delivery_module::DeliveryModuleClient::decode_message_received(evt)
    else {
        eprintln!("chat_module inbound: messageReceived payload missing or malformed");
        return;
    };
    if !msg.content_topic.starts_with(crate::delivery::TOPIC_PREFIX) {
        return;
    }
    // The receiver is the client's worker; if it has gone away the client is being
    // dropped and the bridge is about to stop, so a failed send is benign.
    let _ = inbound_tx.send(msg.payload);
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
                if let Err(e) = res {
                    eprintln!("chat_module: delivery_module.subscribe failed: {e}");
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
            } => {
                // The account is directory-verified by the client; a sender
                // that claims none surfaces as its device id.
                let sender_addr = sender.account.as_ref().unwrap_or(&sender.local_identity);
                record_message_received(&convo_id, &content, sender_addr.as_str());
            }
            Event::ConversationMembersChanged { convo_id } => {
                record_members_changed(&convo_id);
            }
            Event::InboundError { message } => {
                eprintln!("chat_module: inbound error: {message}");
            }
            // `Event` is `#[non_exhaustive]`; ignore variants added upstream.
            _ => {}
        }
    }
}

/// Map libchat's display class to the module's contract kind: the pairwise
/// shape (PrivateV1 / DirectV1) is `direct`, GroupV2 is `group`.
fn kind_for_class(class: ConversationClass) -> ConversationKind {
    match class {
        ConversationClass::Private => ConversationKind::Direct,
        ConversationClass::Group => ConversationKind::Group,
    }
}

fn handle_connection_state(status: &str) {
    // delivery_module's `connectionStateChanged` carries only a status (its
    // second field is a timestamp, not a human detail), so detail stays empty.
    with_display_mut(|d| {
        if let Some(next) = connection_transition(d.delivery_state.state, status) {
            set_delivery_state(d, next, "");
        }
    });
}

/// The delivery state to move to for an upstream connectivity `status`, or
/// `None` to ignore the event. While still `Initialising`, connectivity is
/// ignored: delivery reports `Connected` mid-bootstrap, ~tens of seconds before
/// the transport can service a call, so readiness is gated on the start
/// handshake (see `actions::start_delivery_bootstrap`) — not on this event. Once
/// started, connectivity drives online/offline for reconnect handling.
pub(crate) fn connection_transition(
    current: DeliveryStateKind,
    status: &str,
) -> Option<DeliveryStateKind> {
    if current == DeliveryStateKind::Initialising {
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
        // Online — readiness is gated on the start handshake.
        assert_eq!(
            connection_transition(DeliveryStateKind::Initialising, "Connected"),
            None
        );
        assert_eq!(
            connection_transition(DeliveryStateKind::Initialising, "Disconnected"),
            None
        );
    }

    #[test]
    fn connectivity_drives_state_once_started() {
        // After startup, connectivity drives online/offline for reconnect.
        assert_eq!(
            connection_transition(DeliveryStateKind::Online, "Disconnected"),
            Some(DeliveryStateKind::Error)
        );
        assert_eq!(
            connection_transition(DeliveryStateKind::Error, "Connected"),
            Some(DeliveryStateKind::Online)
        );
    }

    #[test]
    fn class_maps_to_contract_kind() {
        assert_eq!(
            kind_for_class(ConversationClass::Private),
            ConversationKind::Direct
        );
        assert_eq!(
            kind_for_class(ConversationClass::Group),
            ConversationKind::Group
        );
    }
}
