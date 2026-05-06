//! `DeliveryService` impl that forwards `publish()` to delivery_module.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use client::{AddressedEnvelope, DeliveryService};
use logos_rust_sdk::LogosModuleSDK;

pub fn content_topic_for(delivery_address: &str) -> String {
    format!("/logos-chat/1/{}/proto", delivery_address)
}

/// Stateless delivery service.
/// Constructs a fresh `LogosModuleSDK` per call; the SDK is cheap to instantiate.
pub(crate) struct SdkDelivery;

impl DeliveryService for SdkDelivery {
    type Error = String;

    fn publish(&mut self, envelope: AddressedEnvelope) -> Result<(), String> {
        let topic = content_topic_for(&envelope.delivery_address);
        // envelope.data is a libchat-encrypted ciphertext blob — arbitrary
        // binary, not UTF-8. The SDK marshals params as JSON (which requires
        // valid UTF-8) and delivery_module::send takes QString (which requires
        // valid UTF-16), so the bytes need a string-safe wrapper to cross both
        // boundaries. inbound.rs decodes on the receive side.
        //
        // TODO: drop once delivery_module exposes binary send / messageReceived
        // (QByteArray) and logos-rust-sdk / logos-cpp-sdk add a bytes Param type.
        let payload_b64 = BASE64.encode(&envelope.data);

        let sdk = LogosModuleSDK::new();
        let delivery = sdk.plugin("delivery_module");

        match delivery.call_sync("send", &[topic.as_str(), payload_b64.as_str()]) {
            Ok(r) if r.success => Ok(()),
            Ok(r) => Err(format!("delivery_module.send failed: {}", r.message)),
            Err(e) => Err(format!("delivery_module IPC error: {e}")),
        }
    }
}
