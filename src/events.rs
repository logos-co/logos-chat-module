//! Plugin-event queue.
//!
//! Logos modules normally push notifications to subscribers by emitting
//! plugin events through the SDK — the same mechanism we consume on the
//! receive side via `LogosModuleSDK::on(...)` (e.g. delivery_module's
//! `messageReceived`). The c-ffi module track has no story yet for
//! module-emitted events (logos-cpp-generator has no event-declaration
//! convention, the generated Qt glue has no emit forwarding, and
//! logos-rust-sdk has no Rust-side emit API), so we buffer events here
//! and consumers drain them via `chat_module_drain_events_json()`.

use std::collections::VecDeque;

use serde::Serialize;

use crate::module::DeliveryStateKind;

/// Soft cap on pending events. If the consumer stops draining (stuck
/// thread, never polls, bug), we drop the oldest events rather than grow
/// unbounded; the next drain prepends an `EventsDropped` event so the
/// consumer knows to refetch derived state.
const CAPACITY: usize = 4096;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub(crate) enum Event {
    #[serde(rename = "messageReceived")]
    MessageReceived {
        convo_id: String,
        from_self: bool,
        content: String,
        timestamp_ms: u64,
        is_new_convo: bool,
    },
    #[serde(rename = "messageSent")]
    MessageSent {
        convo_id: String,
        content: String,
        timestamp_ms: u64,
    },
    #[serde(rename = "conversationCreated")]
    ConversationCreated {
        convo_id: String,
        is_outgoing: bool,
        peer_label: String,
    },
    #[serde(rename = "conversationUpdated")]
    ConversationUpdated { convo_id: String },
    #[serde(rename = "conversationDeleted")]
    ConversationDeleted { convo_id: String },
    #[serde(rename = "deliveryStateChanged")]
    DeliveryStateChanged {
        state: DeliveryStateKind,
        detail: String,
    },
    /// Synthetic event prepended to a drain when overflow has discarded
    /// some earlier events; tells the consumer to refetch the lists it
    /// would otherwise rebuild incrementally.
    #[serde(rename = "eventsDropped")]
    EventsDropped { count: u64 },
}

/// Bounded FIFO queue of pending plugin events.
pub(crate) struct EventQueue {
    inner: VecDeque<Event>,
    /// Events evicted since the last drain; emitted as a single
    /// `EventsDropped` at the head of the next drain and then reset.
    dropped: u64,
}

impl EventQueue {
    pub(crate) fn new() -> Self {
        Self {
            inner: VecDeque::new(),
            dropped: 0,
        }
    }

    pub(crate) fn push(&mut self, event: Event) {
        if self.inner.len() >= CAPACITY {
            self.inner.pop_front();
            self.dropped += 1;
        }
        self.inner.push_back(event);
    }

    /// Drain the queue to a JSON array string. If overflow occurred since
    /// the last drain, an `EventsDropped` event is prepended. Always
    /// returns valid JSON; never returns an empty string. Empty queue ⇒
    /// `"[]"`.
    pub(crate) fn drain_to_json(&mut self) -> String {
        let mut out: Vec<Event> = Vec::with_capacity(self.inner.len() + 1);
        if self.dropped > 0 {
            out.push(Event::EventsDropped {
                count: self.dropped,
            });
            self.dropped = 0;
        }
        out.extend(self.inner.drain(..));
        serde_json::to_string(&out).unwrap_or_else(|_| "[]".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_to_json_clears_queue() {
        let mut q = EventQueue::new();
        q.push(Event::ConversationUpdated {
            convo_id: "c1".into(),
        });
        q.push(Event::DeliveryStateChanged {
            state: DeliveryStateKind::Online,
            detail: String::new(),
        });

        let json = q.drain_to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "conversationUpdated");
        assert_eq!(arr[0]["convo_id"], "c1");
        assert_eq!(arr[1]["type"], "deliveryStateChanged");
        assert_eq!(arr[1]["state"], "online");

        // Queue is now drained.
        assert_eq!(q.drain_to_json(), "[]");
    }

    #[test]
    fn drain_empty_returns_empty_array() {
        let mut q = EventQueue::new();
        assert_eq!(q.drain_to_json(), "[]");
    }

    #[test]
    fn message_received_serialises_all_fields() {
        let mut q = EventQueue::new();
        q.push(Event::MessageReceived {
            convo_id: "abc".into(),
            from_self: false,
            content: "hi".into(),
            timestamp_ms: 1234,
            is_new_convo: true,
        });
        let json = q.drain_to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let evt = &parsed[0];
        assert_eq!(evt["type"], "messageReceived");
        assert_eq!(evt["convo_id"], "abc");
        assert_eq!(evt["from_self"], false);
        assert_eq!(evt["content"], "hi");
        assert_eq!(evt["timestamp_ms"], 1234);
        assert_eq!(evt["is_new_convo"], true);
    }

    #[test]
    fn overflow_drops_oldest_and_reports_via_events_dropped() {
        let mut q = EventQueue::new();
        for i in 0..(CAPACITY + 3) {
            q.push(Event::ConversationUpdated {
                convo_id: format!("c{i}"),
            });
        }
        let json = q.drain_to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        // CAPACITY survivors + one EventsDropped at the head.
        assert_eq!(arr.len(), CAPACITY + 1);
        assert_eq!(arr[0]["type"], "eventsDropped");
        assert_eq!(arr[0]["count"], 3);
        // Oldest survivor is c3 (c0..c2 were evicted).
        assert_eq!(arr[1]["type"], "conversationUpdated");
        assert_eq!(arr[1]["convo_id"], "c3");

        // After drain, no further dropped count carries over.
        let json2 = q.drain_to_json();
        assert_eq!(json2, "[]");
    }
}
