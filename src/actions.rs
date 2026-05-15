//! Business operations against `ModuleState`. Each function is a single
//! semantic operation that parses no C and locks no mutex; `lib.rs`
//! marshals them across the C ABI.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use client::{ChatClient, ConversationIdOwned, StorageConfig};

use crate::delivery::SdkDelivery;
use crate::events::{Event, EventQueue};
use crate::module::{
    effective_installation_name, now_ms, short_label, DeliveryState, DeliveryStateKind, ModuleState,
};
use crate::persistence::{load_state, save_state, ChatSession, DisplayMessage};

/// Failure modes for the steady-state C ABI surface (post-`initialize`).
#[derive(Debug, thiserror::Error)]
pub(crate) enum CoreError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Delivery(String),
    #[error("{0}")]
    Internal(String),
}

impl CoreError {
    pub(crate) fn to_code(&self) -> i32 {
        match self {
            CoreError::NotFound => crate::CHAT_MODULE_ERR_NOT_FOUND,
            CoreError::Delivery(_) => crate::CHAT_MODULE_ERR_DELIVERY,
            CoreError::Internal(_) => crate::CHAT_MODULE_ERR_INTERNAL,
        }
    }
}

/// Failure modes for [`initialize`].
#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Delivery(String),
}

impl InitError {
    pub(crate) fn to_code(&self) -> i32 {
        match self {
            InitError::Internal(_) => crate::CHAT_MODULE_ERR_INTERNAL,
            InitError::Delivery(_) => crate::CHAT_MODULE_ERR_DELIVERY,
        }
    }
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

pub(crate) fn initialize(
    instance_path: &str,
    preset: &str,
    tcp_port: i32,
) -> Result<ModuleState, InitError> {
    fs::create_dir_all(instance_path)
        .map_err(|e| InitError::Internal(format!("cannot create instance_path: {e}")))?;

    let db_path = format!("{instance_path}/identity.db");
    // Static key derived from the instance path. Not secret; satisfies
    // SQLCipher's keying requirement. A user-provided passphrase is a
    // future enhancement.
    let key = format!("rust-chat-{}", instance_path.replace('/', "_"));
    let storage = StorageConfig::Encrypted { path: db_path, key };

    // Bootstrap delivery_module. The contract (`delivery_module_plugin.h`)
    // says all calls are synchronous; no readiness wait needed once `start`
    // returns OK.
    //
    // TODO: delivery_module's lifecycle should be owned by the host, not
    // the consumer. createNode rejects duplicates and start is not
    // idempotent, so chat_module can't coexist with another delivery_module
    // consumer today. Drop these calls once the host bootstraps
    // delivery_module and exposes it ready-to-use.
    let config_json = serde_json::json!({
        "mode": "Core",
        "preset": preset,
        "tcpPort": tcp_port,
        "discv5UdpPort": tcp_port,
        "logLevel": "ERROR",
    })
    .to_string();

    let sdk = logos_rust_sdk::LogosModuleSDK::new();
    let mut dm = sdk.plugin("delivery_module");

    // Register listeners before `start` — `connectionStateChanged` fires
    // during start and is not re-emitted, so a late subscribe misses it.
    let messages_rx = dm
        .on("messageReceived")
        .map_err(|e| InitError::Delivery(format!("subscribe(messageReceived) failed: {e}")))?;
    let conn_rx = match dm.on("connectionStateChanged") {
        Ok(rx) => Some(rx),
        Err(e) => {
            // Non-fatal: messaging still works; we just won't surface
            // delivery_state changes pushed by the node.
            eprintln!("chat_module_init: subscribe(connectionStateChanged) failed: {e}");
            None
        }
    };

    let no_args: &[&str] = &[];
    for (method, args) in [
        ("createNode", &[config_json.as_str()] as &[&str]),
        ("start", no_args),
    ] {
        match dm.call_sync(method, args) {
            Ok(r) if r.success => {}
            Ok(r) => {
                return Err(InitError::Delivery(format!(
                    "delivery_module.{method} failed: {}",
                    r.message
                )));
            }
            Err(e) => {
                return Err(InitError::Delivery(format!(
                    "delivery_module.{method} IPC error: {e}"
                )));
            }
        }
    }

    // TODO: forward libchat's future `DeliveryService::subscribe(addr)` to
    // delivery_module and drop this literal.
    let inbound_topic = "/logos-chat/1/delivery_address/proto";
    match dm.call_sync("subscribe", &[inbound_topic]) {
        Ok(r) if r.success => {}
        Ok(r) => {
            return Err(InitError::Delivery(format!(
                "subscribe failed: {}",
                r.message
            )))
        }
        Err(e) => return Err(InitError::Delivery(format!("subscribe IPC error: {e}"))),
    }

    let client = ChatClient::open("logos-chat", storage, SdkDelivery)
        .map_err(|e| InitError::Internal(format!("ChatClient::open failed: {e:?}")))?;

    let state_path = PathBuf::from(format!("{instance_path}/history.json"));
    let state = load_state(&state_path);

    let stop = Arc::new(AtomicBool::new(false));
    let thread = crate::inbound::spawn(stop.clone(), messages_rx, conn_rx);

    Ok(ModuleState {
        client,
        state,
        state_path,
        event_queue: EventQueue::new(),
        delivery_state: DeliveryState::initialising(),
        inbound_stop: stop,
        inbound_thread: Some(thread),
        _delivery_subscriptions: dm,
    })
}

/// Consumes `ms`: signals the inbound worker to stop, joins it, and writes
/// final state. Called by `lib.rs` after taking the singleton out of the
/// module lock so the worker doesn't deadlock on its own next acquire.
pub(crate) fn shutdown(mut ms: ModuleState) {
    ms.inbound_stop.store(true, Ordering::Relaxed);
    if let Some(handle) = ms.inbound_thread.take() {
        // Bounded by inbound::POLL_INTERVAL; ~50 ms worst case.
        let _ = handle.join();
    }
    save_state(&ms.state, &ms.state_path);
}

// ── Identity ─────────────────────────────────────────────────────────────────

pub(crate) fn set_installation_name(ms: &mut ModuleState, name: &str) {
    ms.state.installation_name = if name.is_empty() {
        None
    } else {
        Some(name.to_owned())
    };
    save_state(&ms.state, &ms.state_path);
}

pub(crate) fn create_intro_bundle(ms: &mut ModuleState) -> Result<String, CoreError> {
    let bytes = ms
        .client
        .create_intro_bundle()
        .map_err(|e| CoreError::Internal(format!("create_intro_bundle failed: {e:?}")))?;
    String::from_utf8(bytes)
        .map_err(|_| CoreError::Internal("create_intro_bundle: bundle bytes are not UTF-8".into()))
}

// ── Conversations ────────────────────────────────────────────────────────────

pub(crate) fn create_conversation(
    ms: &mut ModuleState,
    bundle: &str,
    content: &str,
) -> Result<String, CoreError> {
    let convo_id = ms
        .client
        .create_conversation(bundle.as_bytes(), content.as_bytes())
        .map_err(|e| CoreError::Internal(format!("create_conversation failed: {e:?}")))?;

    let chat_id = convo_id.to_string();
    let ts = now_ms();

    let mut session = ChatSession {
        chat_id: chat_id.clone(),
        nickname: None,
        messages: Vec::new(),
    };
    if !content.is_empty() {
        session.messages.push(DisplayMessage {
            from_self: true,
            content: content.to_string(),
            timestamp_ms: ts,
        });
    }
    let peer_label = short_label(&chat_id).to_owned();
    ms.state.chats.insert(chat_id.clone(), session);
    ms.event_queue.push(Event::ConversationCreated {
        convo_id: chat_id.clone(),
        is_outgoing: true,
        peer_label,
    });
    save_state(&ms.state, &ms.state_path);

    Ok(chat_id)
}

pub(crate) fn list_conversations(ms: &ModuleState) -> serde_json::Value {
    let items: Vec<serde_json::Value> = ms
        .state
        .chats
        .values()
        .map(|s| {
            let last_ts = s.messages.last().map(|m| m.timestamp_ms).unwrap_or(0);
            serde_json::json!({
                "convo_id": s.chat_id,
                "nickname": s.nickname,
                "message_count": s.messages.len(),
                "last_ts": last_ts,
            })
        })
        .collect();
    serde_json::Value::Array(items)
}

pub(crate) fn get_messages(ms: &ModuleState, convo_id: &str) -> serde_json::Value {
    let msgs = ms
        .state
        .chats
        .get(convo_id)
        .map(|s| s.messages.as_slice())
        .unwrap_or(&[]);
    serde_json::to_value(msgs).unwrap_or_else(|_| serde_json::Value::Array(vec![]))
}

pub(crate) fn send_message(
    ms: &mut ModuleState,
    convo_id: &str,
    content: &str,
) -> Result<(), CoreError> {
    if !ms.state.chats.contains_key(convo_id) {
        return Err(CoreError::NotFound);
    }
    let cid: ConversationIdOwned = convo_id.into();
    ms.client
        .send_message(&cid, content.as_bytes())
        .map_err(|e| CoreError::Delivery(format!("send_message failed: {e:?}")))?;

    let ts = now_ms();
    if let Some(session) = ms.state.chats.get_mut(convo_id) {
        session.messages.push(DisplayMessage {
            from_self: true,
            content: content.to_string(),
            timestamp_ms: ts,
        });
    }
    ms.event_queue.push(Event::MessageSent {
        convo_id: convo_id.to_owned(),
        content: content.to_owned(),
        timestamp_ms: ts,
    });
    save_state(&ms.state, &ms.state_path);
    Ok(())
}

pub(crate) fn set_conversation_nickname(
    ms: &mut ModuleState,
    convo_id: &str,
    nickname: &str,
) -> Result<(), CoreError> {
    let session = ms
        .state
        .chats
        .get_mut(convo_id)
        .ok_or(CoreError::NotFound)?;
    session.nickname = if nickname.is_empty() {
        None
    } else {
        Some(nickname.to_string())
    };
    save_state(&ms.state, &ms.state_path);
    ms.event_queue.push(Event::ConversationUpdated {
        convo_id: convo_id.to_string(),
    });
    Ok(())
}

pub(crate) fn delete_conversation(ms: &mut ModuleState, convo_id: &str) -> Result<(), CoreError> {
    if ms.state.chats.remove(convo_id).is_none() {
        return Err(CoreError::NotFound);
    }
    ms.state.deleted.insert(convo_id.to_owned());
    save_state(&ms.state, &ms.state_path);
    ms.event_queue.push(Event::ConversationDeleted {
        convo_id: convo_id.to_string(),
    });
    Ok(())
}

// ── Status ───────────────────────────────────────────────────────────────────

pub(crate) fn status(ms: Option<&ModuleState>) -> serde_json::Value {
    match ms {
        None => serde_json::json!({
            "identity": serde_json::Value::Null,
            "convo_count": 0,
            "delivery_state": DeliveryStateKind::Stopped,
            "detail": "",
        }),
        Some(ms) => serde_json::json!({
            "identity": effective_installation_name(ms),
            "convo_count": ms.state.chats.len(),
            "delivery_state": ms.delivery_state.state,
            "detail": ms.delivery_state.detail,
        }),
    }
}

// ── Inbound-side helpers (called by inbound.rs worker) ───────────────────────

/// Update delivery state and emit a plugin event. No-op if `state` matches
/// the current value.
pub(crate) fn set_delivery_state(ms: &mut ModuleState, state: DeliveryStateKind, detail: &str) {
    if ms.delivery_state.state == state && ms.delivery_state.detail == detail {
        return;
    }
    ms.delivery_state = DeliveryState {
        state,
        detail: detail.to_owned(),
    };
    ms.event_queue.push(Event::DeliveryStateChanged {
        state,
        detail: detail.to_owned(),
    });
}

/// Decrypt a single inbound payload and reflect it in local state. Called
/// from the inbound worker thread under the module mutex.
pub(crate) fn process_payload(ms: &mut ModuleState, payload: &[u8]) {
    match ms.client.receive(payload) {
        Ok(Some(content)) => {
            let chat_id = content.conversation_id.clone();

            // libchat retains crypto state across local deletes, so we
            // still receive decryptable payloads for deleted convos.
            if ms.state.deleted.contains(&chat_id) {
                return;
            }

            let is_new = content.is_new_convo && !ms.state.chats.contains_key(&chat_id);

            if is_new {
                let session = ChatSession {
                    chat_id: chat_id.clone(),
                    nickname: None,
                    messages: Vec::new(),
                };
                ms.state.chats.insert(chat_id.clone(), session);

                ms.event_queue.push(Event::ConversationCreated {
                    convo_id: chat_id.clone(),
                    is_outgoing: false,
                    peer_label: short_label(&chat_id).to_owned(),
                });
            }

            if !content.data.is_empty() {
                let text = String::from_utf8_lossy(&content.data).to_string();
                let ts = now_ms();

                if let Some(session) = ms.state.chats.get_mut(&chat_id) {
                    session.messages.push(DisplayMessage {
                        from_self: false,
                        content: text.clone(),
                        timestamp_ms: ts,
                    });
                }

                ms.event_queue.push(Event::MessageReceived {
                    convo_id: chat_id,
                    from_self: false,
                    content: text,
                    timestamp_ms: ts,
                    is_new_convo: is_new,
                });
            }

            save_state(&ms.state, &ms.state_path);
        }
        Ok(None) => {}
        Err(e) => eprintln!("chat_module: receive error: {e:?}"),
    }
}
