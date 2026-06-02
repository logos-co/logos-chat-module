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
//! Subscription itself must run on the main thread (QtRO deadlocks
//! otherwise), so it's done in `chat_module_init` and the resulting
//! `Receiver`s are moved into the worker.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use logos_rust_sdk::EventData;

use crate::actions::{process_payload, set_delivery_state};
use crate::module::{module, DeliveryStateKind};

// libp2p delivers every message in the shard regardless of subscribed
// topic, so filter loosely on prefix and let `ChatClient::receive`
// discriminate.
pub(crate) const INBOUND_TOPIC_PREFIX: &str = "/logos-chat/1/";

const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn spawn(
    stop: Arc<AtomicBool>,
    messages: Receiver<EventData>,
    conn: Option<Receiver<EventData>>,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("rust-chat-inbound".into())
        .spawn(move || run(stop, messages, conn))
        .expect("failed to spawn inbound thread")
}

fn run(
    stop: Arc<AtomicBool>,
    messages: Receiver<EventData>,
    mut conn: Option<Receiver<EventData>>,
) {
    while !stop.load(Ordering::Relaxed) {
        match messages.recv_timeout(POLL_INTERVAL) {
            Ok(evt) => handle_message_received(&evt),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        // On Disconnected, drop the channel so we stop re-polling a dead one.
        let mut disconnected = false;
        if let Some(events) = conn.as_ref() {
            loop {
                match events.try_recv() {
                    Ok(evt) => {
                        let status = evt.get_str(0).unwrap_or("");
                        handle_connection_state(status);
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
    let topic = evt.get_str(1).unwrap_or("");
    if !topic.starts_with(INBOUND_TOPIC_PREFIX) {
        return;
    }
    let Some(bytes) = evt.get_bytes(2) else {
        eprintln!("chat_module inbound: messageReceived payload missing or not base64");
        return;
    };

    let _ = module().with_state_mut(|ms| process_payload(ms, &bytes));
}

fn handle_connection_state(status: &str) {
    // delivery_module's `connectionStateChanged` carries only a status (its
    // second field is a timestamp, not a human detail), so detail stays empty.
    let mapped = map_connection_status(status);
    let _ = module().with_state_mut(|ms| set_delivery_state(ms, mapped, ""));
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
}
