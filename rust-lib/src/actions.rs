//! Business operations. Each is a single semantic operation that owns its
//! locking: operations that call into libchat take the client lock ([`module`])
//! for the call; the read methods and the recording of results take the display
//! lock ([`with_display`]). A mutation takes the client lock then the display
//! lock — never the reverse — so the two can't deadlock. `lib.rs` invokes these
//! from the `ChatModule` trait implementation.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use client::{ChatClient, ConversationIdOwned, StorageConfig};
use serde::Serialize;

use crate::delivery::SdkDelivery;
use crate::module::{
    module, now_ms, short_label, with_display, with_display_mut, DeliveryState, DeliveryStateKind,
    Display, ModuleState,
};
use crate::persistence::{load_state, save_state, ChatSession, DisplayMessage};

/// Failure modes for the steady-state methods (post-`initialize`).
#[derive(Debug, thiserror::Error)]
pub(crate) enum CoreError {
    #[error("module not initialised")]
    NotInit,
    #[error("conversation not found")]
    NotFound,
    #[error("{0}")]
    Delivery(String),
    #[error("{0}")]
    Internal(String),
}

/// Failure modes for [`initialize`].
#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Delivery(String),
}

// ── Contract view types ──────────────────────────────────────────────────────
//
// Serde shapes matching the `Conversation` / `Status` records in
// chat_module.lidl — the payloads `list_conversations` and `status` return. The
// messages list reuses `persistence::DisplayMessage`, which already matches the
// `Message` record.

/// One element of `list_conversations` — mirrors the `Conversation` record.
#[derive(Serialize)]
struct ConversationSummary {
    convo_id: String,
    nickname: Option<String>,
    message_count: usize,
    last_activity_ms: u64,
}

/// `status` payload — mirrors the `Status` record.
#[derive(Serialize)]
struct StatusView {
    convo_count: usize,
    delivery_state: DeliveryStateKind,
    detail: String,
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

pub(crate) fn initialize(instance_path: &str) -> Result<ModuleState, InitError> {
    fs::create_dir_all(instance_path)
        .map_err(|e| InitError::Internal(format!("cannot create instance_path: {e}")))?;

    let db_path = format!("{instance_path}/identity.db");
    // Static key derived from the instance path. Not secret; satisfies
    // SQLCipher's keying requirement. A user-provided passphrase is a
    // future enhancement.
    let key = format!("rust-chat-{}", instance_path.replace('/', "_"));
    let storage = StorageConfig::Encrypted { path: db_path, key };

    // Do the fallible *local* setup (DB open, state load) before touching
    // delivery_module. The node's lifecycle is irreversible — createNode
    // rejects duplicates and start is not idempotent (see the TODO in
    // `start_delivery_bootstrap`) — so an open/load failure must abort init
    // before any node exists; otherwise a partial init would strand a started,
    // unowned node with no inbound worker and no way to stop it.
    let client = ChatClient::open("logos-chat", storage, SdkDelivery)
        .map_err(|e| InitError::Internal(format!("ChatClient::open failed: {e:?}")))?;
    let intrinsic_name = client.installation_name().to_owned();

    let state_path = PathBuf::from(format!("{instance_path}/history.json"));
    let state = load_state(&state_path);

    // Register listeners before the node starts — `connectionStateChanged`
    // fires during start and is not re-emitted, so a late subscribe misses it.
    // The subscriptions are handed to the worker, which polls them; nothing
    // arrives until `start_delivery_bootstrap` starts the node.
    let mut dm = crate::modules().delivery_module;
    let messages_sub = dm
        .on_message_received()
        .map_err(|e| InitError::Delivery(format!("subscribe(messageReceived) failed: {e}")))?;
    let conn_sub = match dm.on_connection_state_changed() {
        Ok(sub) => Some(sub),
        Err(e) => {
            // Non-fatal: messaging still works; we just won't surface
            // delivery_state changes pushed by the node.
            eprintln!("chat_module init: subscribe(connectionStateChanged) failed: {e}");
            None
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let thread = crate::inbound::spawn(stop.clone(), messages_sub, conn_sub);

    // Seed the display state read by the getters (the client lives behind the
    // other lock; its intrinsic name is cached here for get_installation_name).
    with_display_mut(|d| {
        d.state = state;
        d.state_path = state_path;
        d.delivery_state = DeliveryState::initialising();
        d.intrinsic_name = intrinsic_name;
    });

    Ok(ModuleState {
        client,
        inbound_stop: stop,
        inbound_thread: Some(thread),
    })
}

/// Bootstrap delivery_module's node and report readiness, asynchronously.
///
/// Called by `lib.rs` *after* the module state is installed and the module lock
/// is released, so the async completion callbacks acquire a free lock and never
/// re-enter it. createNode → start → subscribe are chained — each step needs the
/// previous (start rejects until the node exists; the content-topic subscribe
/// needs a started node) — and every step runs off the dispatch (Qt event-loop)
/// thread, so bootstrap, which can take tens of seconds, never blocks it.
///
/// Readiness (`online`) is reported only once the start/subscribe handshake
/// completes; we do NOT use delivery's earlier `connectionStateChanged=Connected`,
/// which fires mid-bootstrap ~tens of seconds before the transport can service a
/// call (gating the UI on it lets actions run into the IPC timeout). The inbound
/// worker keeps consuming connectionStateChanged for reconnect/offline handling
/// once we're started.
///
/// TODO: delivery_module's lifecycle should be owned by the host, not the
/// consumer. createNode rejects duplicates and start is not idempotent, so
/// chat_module can't coexist with another delivery_module consumer today. Drop
/// these calls once the host bootstraps delivery_module and exposes it
/// ready-to-use.
pub(crate) fn start_delivery_bootstrap(preset: &str, tcp_port: i32) {
    let config_json = serde_json::json!({
        "mode": "Core",
        "preset": preset,
        "tcpPort": tcp_port,
        "discv5UdpPort": tcp_port,
        "logLevel": "ERROR",
    })
    .to_string();

    // "delivery_address" is a placeholder segment, not a real address: the
    // inbound worker filters loosely on the topic prefix and libchat has no
    // accessor for our own inbound address yet.
    // TODO: forward libchat's future `DeliveryService::subscribe(addr)` to
    // delivery_module and drop this literal.
    let inbound_topic = crate::delivery::content_topic_for("delivery_address");

    crate::modules()
        .delivery_module
        .create_node_async(&config_json, move |res| match res {
            Ok(_) => start_node(inbound_topic),
            Err(e) => set_delivery_error(format!("delivery_module.createNode failed: {e}")),
        });
}

/// Bootstrap step 2 of 3: start the node, then chain the subscribe.
fn start_node(inbound_topic: String) {
    crate::modules()
        .delivery_module
        .start_async(move |res| match res {
            Ok(_) => subscribe_inbound(inbound_topic),
            Err(e) => set_delivery_error(format!("delivery_module.start failed: {e}")),
        });
}

/// Bootstrap step 3 of 3: subscribe to the inbound topic and report readiness.
fn subscribe_inbound(inbound_topic: String) {
    crate::modules()
        .delivery_module
        .subscribe_async(&inbound_topic, move |res| {
            if let Err(e) = res {
                // Receiving inbound needs this subscription; log but don't withhold
                // readiness — sending and identity work without it, and the node started.
                eprintln!("chat_module: delivery_module.subscribe failed: {e}");
            }
            with_display_mut(|d| set_delivery_state(d, DeliveryStateKind::Online, ""));
        });
}

/// Record an async-bootstrap failure: log it and reflect it in delivery_state.
fn set_delivery_error(detail: String) {
    eprintln!("chat_module: {detail}");
    with_display_mut(|d| set_delivery_state(d, DeliveryStateKind::Error, &detail));
}

/// Consumes `ms`: signals the inbound worker to stop, joins it, writes final
/// state, and resets the display so a re-init starts clean. Called by `lib.rs`
/// after taking the singleton out of the module lock so the worker doesn't
/// deadlock on its own next acquire.
pub(crate) fn shutdown(mut ms: ModuleState) {
    ms.inbound_stop.store(true, Ordering::Relaxed);
    if let Some(handle) = ms.inbound_thread.take() {
        // Bounded by inbound::POLL_INTERVAL; ~50 ms worst case.
        let _ = handle.join();
    }
    with_display_mut(|d| {
        // Final write; nothing left to propagate to, so log a failure.
        if let Err(e) = save_state(&d.state, &d.state_path) {
            eprintln!("chat_module: save_state failed on shutdown: {e}");
        }
        *d = Display::default();
    });
}

/// Run `f` with the libchat client under the module lock, mapping "no client"
/// (not initialised) and the unreachable poisoned lock to a [`CoreError`].
fn with_client<R>(f: impl FnOnce(&mut ChatClient<SdkDelivery>) -> R) -> Result<R, CoreError> {
    match module().with_state_mut(|ms| f(&mut ms.client)) {
        Ok(Some(r)) => Ok(r),
        Ok(None) => Err(CoreError::NotInit),
        Err(_) => Err(CoreError::Internal("module lock poisoned".into())),
    }
}

/// Persist the display state, mapping an I/O failure into a steady-state error
/// so a failed write surfaces to the caller instead of being silently reported
/// as success and then vanishing on the next `load_state`.
fn persist(d: &Display) -> Result<(), CoreError> {
    save_state(&d.state, &d.state_path)
        .map_err(|e| CoreError::Internal(format!("save_state failed: {e}")))
}

// ── Identity ─────────────────────────────────────────────────────────────────

pub(crate) fn set_installation_name(name: &str) -> Result<(), CoreError> {
    with_display_mut(|d| {
        d.state.installation_name = if name.is_empty() {
            None
        } else {
            Some(name.to_owned())
        };
        persist(d)
    })
}

pub(crate) fn installation_name() -> String {
    with_display(crate::module::effective_installation_name)
}

pub(crate) fn create_intro_bundle() -> Result<String, CoreError> {
    let bytes = with_client(|client| client.create_intro_bundle())?
        .map_err(|e| CoreError::Internal(format!("create_intro_bundle failed: {e:?}")))?;
    String::from_utf8(bytes)
        .map_err(|_| CoreError::Internal("create_intro_bundle: bundle bytes are not UTF-8".into()))
}

// ── Conversations ────────────────────────────────────────────────────────────

pub(crate) fn create_conversation(bundle: &str, content: &str) -> Result<String, CoreError> {
    // libchat op under the client lock (on the dispatch thread, where the
    // delivery replica lives). Publish is async (see SdkDelivery), so this
    // returns without blocking on the network.
    let convo_id =
        with_client(|client| client.create_conversation(bundle.as_bytes(), content.as_bytes()))?
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
    with_display_mut(|d| {
        d.state.chats.insert(chat_id.clone(), session);
        persist(d)
    })?;
    crate::emit_conversation_created(&chat_id, true, &peer_label);

    Ok(chat_id)
}

pub(crate) fn list_conversations() -> serde_json::Value {
    with_display(|d| {
        let items: Vec<ConversationSummary> = d
            .state
            .chats
            .values()
            .map(|s| ConversationSummary {
                convo_id: s.chat_id.clone(),
                nickname: s.nickname.clone(),
                message_count: s.messages.len(),
                last_activity_ms: s.messages.last().map(|m| m.timestamp_ms).unwrap_or(0),
            })
            .collect();
        serde_json::to_value(items).unwrap_or_else(|_| serde_json::Value::Array(vec![]))
    })
}

pub(crate) fn get_messages(convo_id: &str) -> serde_json::Value {
    with_display(|d| {
        let msgs = d
            .state
            .chats
            .get(convo_id)
            .map(|s| s.messages.as_slice())
            .unwrap_or(&[]);
        serde_json::to_value(msgs).unwrap_or_else(|_| serde_json::Value::Array(vec![]))
    })
}

pub(crate) fn send_message(convo_id: &str, content: &str) -> Result<(), CoreError> {
    // The convo must exist before we encrypt+send. A concurrent delete between
    // this check and the record below is a benign race (the message goes out but
    // isn't kept for a convo the user just removed).
    if !with_display(|d| d.state.chats.contains_key(convo_id)) {
        return Err(CoreError::NotFound);
    }

    let cid: ConversationIdOwned = convo_id.into();
    with_client(|client| client.send_message(&cid, content.as_bytes()))?
        .map_err(|e| CoreError::Delivery(format!("send_message failed: {e:?}")))?;

    let ts = now_ms();
    // Persist before emitting: the message is already on the wire, but if the
    // local write fails we report failure rather than paint a "sent" bubble
    // the next restart would lose.
    with_display_mut(|d| {
        if let Some(session) = d.state.chats.get_mut(convo_id) {
            session.messages.push(DisplayMessage {
                from_self: true,
                content: content.to_string(),
                timestamp_ms: ts,
            });
        }
        persist(d)
    })?;
    crate::emit_message_sent(convo_id, content, ts as i64);
    Ok(())
}

pub(crate) fn set_conversation_nickname(convo_id: &str, nickname: &str) -> Result<(), CoreError> {
    with_display_mut(|d| {
        let session = d.state.chats.get_mut(convo_id).ok_or(CoreError::NotFound)?;
        session.nickname = if nickname.is_empty() {
            None
        } else {
            Some(nickname.to_string())
        };
        persist(d)
    })?;
    crate::emit_conversation_updated(convo_id);
    Ok(())
}

pub(crate) fn delete_conversation(convo_id: &str) -> Result<(), CoreError> {
    with_display_mut(|d| {
        if d.state.chats.remove(convo_id).is_none() {
            return Err(CoreError::NotFound);
        }
        d.state.deleted.insert(convo_id.to_owned());
        persist(d)
    })?;
    crate::emit_conversation_deleted(convo_id);
    Ok(())
}

// ── Status ───────────────────────────────────────────────────────────────────

pub(crate) fn status() -> serde_json::Value {
    with_display(|d| {
        let view = StatusView {
            convo_count: d.state.chats.len(),
            delivery_state: d.delivery_state.state,
            detail: d.delivery_state.detail.clone(),
        };
        serde_json::to_value(view).unwrap_or(serde_json::Value::Null)
    })
}

// ── Inbound-side helpers (called by inbound.rs worker) ───────────────────────

/// Update delivery state and emit a plugin event. No-op if `state` matches
/// the current value. Operates on the display, which holds delivery_state.
pub(crate) fn set_delivery_state(d: &mut Display, state: DeliveryStateKind, detail: &str) {
    if d.delivery_state.state == state && d.delivery_state.detail == detail {
        return;
    }
    d.delivery_state = DeliveryState {
        state,
        detail: detail.to_owned(),
    };
    crate::emit_delivery_state_changed(state.as_str(), detail);
}

/// Decrypt a single inbound payload and reflect it in local state. Called from
/// the inbound worker thread. The decrypt runs under the client lock (the slow
/// part); recording the result runs under the display lock, so the read methods
/// aren't held up by the decrypt.
pub(crate) fn process_payload(payload: &[u8]) {
    let content = match module().with_state_mut(|ms| ms.client.receive(payload)) {
        Ok(Some(Ok(Some(content)))) => content,
        Ok(Some(Ok(None))) => return, // protocol frame, nothing to record
        Ok(Some(Err(e))) => {
            eprintln!("chat_module: receive error: {e:?}");
            return;
        }
        Ok(None) | Err(_) => return, // not initialised / poisoned (unreachable)
    };

    let chat_id = content.conversation_id.clone();
    with_display_mut(|d| {
        // libchat retains crypto state across local deletes, so we still
        // receive decryptable payloads for deleted convos.
        if d.state.deleted.contains(&chat_id) {
            return;
        }

        let is_new = content.is_new_convo && !d.state.chats.contains_key(&chat_id);
        if is_new {
            d.state.chats.insert(
                chat_id.clone(),
                ChatSession {
                    chat_id: chat_id.clone(),
                    nickname: None,
                    messages: Vec::new(),
                },
            );
            crate::emit_conversation_created(&chat_id, false, short_label(&chat_id));
        }

        if !content.data.is_empty() {
            let text = String::from_utf8_lossy(&content.data).to_string();
            let ts = now_ms();
            if let Some(session) = d.state.chats.get_mut(&chat_id) {
                session.messages.push(DisplayMessage {
                    from_self: false,
                    content: text.clone(),
                    timestamp_ms: ts,
                });
            }
            crate::emit_message_received(&chat_id, &text, ts as i64);
        }

        // Inbound worker has no caller to return to; log a failed write.
        if let Err(e) = save_state(&d.state, &d.state_path) {
            eprintln!("chat_module: save_state failed after inbound message: {e}");
        }
    });
}
