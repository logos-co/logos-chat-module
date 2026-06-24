//! Background worker that processes delivery_module's events.
//!
//! Two events drive the worker:
//! * `messageReceived` — inbound envelopes; decode the payload bytes and
//!   pass to `ChatClient::receive`.
//! * `connectionStateChanged` — drives local `delivery_state` so consumers
//!   don't have to poll delivery_module.
//!
//! `messageReceived` triggers a libchat decryption + sqlcipher write that
//! is too long to run on the Qt main thread; the worker takes it off-thread.
//!
//! Subscription is set up in `init` (before the node starts, so the
//! `connectionStateChanged` emitted during start isn't missed) and the
//! resulting `EventSubscription`s are moved into the worker — each owns its lp
//! subscription and a share of the client, keeping events flowing after the
//! proxy is dropped.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use logos_rust_sdk::{EventData, EventSubscription};

use crate::actions::{process_payload, set_delivery_state};
use crate::module::{with_display_mut, DeliveryStateKind};

const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn spawn(
    stop: Arc<AtomicBool>,
    messages: EventSubscription,
    conn: Option<EventSubscription>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("rust-chat-inbound".into())
        .spawn(move || run(stop, messages, conn))
        .expect("failed to spawn inbound thread")
}

fn run(stop: Arc<AtomicBool>, messages: EventSubscription, mut conn: Option<EventSubscription>) {
    while !stop.load(Ordering::Relaxed) {
        match messages.receiver().recv_timeout(POLL_INTERVAL) {
            Ok(evt) => handle_message_received(&evt),
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
    }
}

fn handle_message_received(evt: &EventData) {
    let Some(msg) = crate::delivery_module::DeliveryModuleClient::decode_message_received(evt)
    else {
        eprintln!("chat_module inbound: messageReceived payload missing or malformed");
        return;
    };
    // libp2p delivers every message in the shard regardless of subscribed
    // topic, so filter loosely on the chat topic prefix and let
    // `ChatClient::receive` discriminate.
    if !msg.content_topic.starts_with(crate::delivery::TOPIC_PREFIX) {
        return;
    }

    process_payload(&msg.payload);
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
/// the transport can service a call, so readiness is gated on the start/subscribe
/// handshake (see `actions::initialize`) — not on this event. Once started,
/// connectivity drives online/offline for reconnect handling.
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
        // Online — readiness is gated on the start/subscribe handshake.
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
}
