//! `DeliveryService` impl that forwards `publish()` to delivery_module.

use client::{AddressedEnvelope, DeliveryService};
use logos_rust_sdk::{LogosModuleSDK, Param};

pub fn content_topic_for(delivery_address: &str) -> String {
    format!("/logos-chat/1/{}/proto", delivery_address)
}

/// Stateless delivery service.
/// Constructs a fresh `LogosModuleSDK` per call; the SDK is cheap to instantiate.
pub(crate) struct SdkDelivery;

impl DeliveryService for SdkDelivery {
    type Error = String;

    fn publish(&mut self, envelope: AddressedEnvelope) -> Result<(), String> {
        // The recipient's delivery address selects the content topic; the
        // encrypted envelope travels as a binary param (`Param::bytes` base64-
        // encodes it for the IPC channel, delivery_module receives raw bytes).
        let topic = content_topic_for(&envelope.delivery_address);

        let sdk = LogosModuleSDK::new();
        let delivery = sdk.plugin("delivery_module");

        let params = [
            Param::string("arg0", topic),
            Param::bytes("arg1", &envelope.data),
        ];
        match delivery.call_sync_with_params("send", &params) {
            Ok(r) if r.success => Ok(()),
            Ok(r) => Err(format!("delivery_module.send failed: {}", r.message)),
            Err(e) => Err(format!("delivery_module IPC error: {e}")),
        }
    }
}
