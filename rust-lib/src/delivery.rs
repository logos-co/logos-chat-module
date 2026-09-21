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

/// The first is the default.
pub(crate) const DELIVERY_PRESETS: [&str; 2] = ["logos.test", "logos.dev"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AnonymityLevel {
    #[default]
    None,
    Preferred,
    Required,
}

impl AnonymityLevel {
    pub(crate) fn parse(level: &str) -> Option<Self> {
        match level.to_ascii_lowercase().as_str() {
            "" | "none" => Some(Self::None),
            "preferred" => Some(Self::Preferred),
            "required" => Some(Self::Required),
            _ => None,
        }
    }

    fn as_delivery(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Preferred => "Preferred",
            Self::Required => "Required",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeliverySettings {
    pub preset: &'static str,
    pub anonymity: AnonymityLevel,
}

impl DeliverySettings {
    pub(crate) fn from_config(preset: &str, anonymity: &str) -> Result<Self, String> {
        let preset = match preset {
            "" => DELIVERY_PRESETS[0],
            named => DELIVERY_PRESETS
                .into_iter()
                .find(|known| *known == named)
                .ok_or_else(|| {
                    format!(
                        "unknown delivery_preset {named:?}; expected one of {}",
                        DELIVERY_PRESETS.join(", ")
                    )
                })?,
        };
        let anonymity = AnonymityLevel::parse(anonymity).ok_or_else(|| {
            format!("unknown anonymity_level {anonymity:?}; expected none, preferred or required")
        })?;
        Ok(Self { preset, anonymity })
    }

    /// Only wrapper keys may sit at the top level: a bare key selects delivery's
    /// legacy flat parser, whose fixed ports collide between instances on a host.
    pub(crate) fn create_node_config(&self) -> String {
        serde_json::json!({
            "mode": "Core",
            "preset": self.preset,
            "messagingOverrides": {
                "logLevel": "ERROR",
                "anonymityLevel": self.anonymity.as_delivery(),
            },
        })
        .to_string()
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
                if let Err(e) = res {
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

    #[test]
    fn empty_settings_join_logos_test_without_anonymity() {
        let settings = DeliverySettings::from_config("", "").unwrap();

        assert_eq!(settings.preset, "logos.test");
        assert_eq!(settings.anonymity, AnonymityLevel::None);
    }

    #[test]
    fn both_networks_are_selectable() {
        for preset in ["logos.test", "logos.dev"] {
            assert_eq!(
                DeliverySettings::from_config(preset, "").unwrap().preset,
                preset
            );
        }
    }

    #[test]
    fn any_other_preset_is_refused() {
        for preset in ["logostest", "Logos.Test", "status.prod"] {
            assert!(
                DeliverySettings::from_config(preset, "").is_err(),
                "{preset}"
            );
        }
    }

    #[test]
    fn anonymity_level_reads_in_any_case() {
        assert_eq!(
            AnonymityLevel::parse("Required"),
            Some(AnonymityLevel::Required)
        );
        assert_eq!(
            AnonymityLevel::parse("preferred"),
            Some(AnonymityLevel::Preferred)
        );
        assert_eq!(AnonymityLevel::parse("NONE"), Some(AnonymityLevel::None));
        assert_eq!(AnonymityLevel::parse("on"), None);
    }

    #[test]
    fn create_node_config_is_layered_and_carries_the_anonymity_level() {
        let settings = DeliverySettings::from_config("logos.dev", "required").unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&settings.create_node_config()).unwrap();

        assert_eq!(
            config,
            serde_json::json!({
                "mode": "Core",
                "preset": "logos.dev",
                "messagingOverrides": { "logLevel": "ERROR", "anonymityLevel": "Required" },
            })
        );
    }
}
