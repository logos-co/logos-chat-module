//! Business operations. Each is a single semantic operation that owns its
//! locking: operations that call into libchat take the client lock ([`module`])
//! for the call; the read methods and the recording of results take the display
//! lock ([`with_display`]). A mutation takes the client lock then the display
//! lock — never the reverse — so the two can't deadlock. `lib.rs` invokes these
//! from the `ChatModule` trait implementation.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use libchat::ChatStorage;
use logos_account::TestLogosAccount;
use logos_generic_chat::{ChatClientBuilder, DelegateSigner, HttpRegistry, StorageConfig};
use serde::Serialize;

use crate::delivery::SdkDelivery;

/// The devnet KeyPackage registry DirectV1 uses to publish this installation's
/// key package and fetch a peer's. Hardcoded for now; a configurable endpoint is
/// a future enhancement (the wiring is behind libchat's `RegistrationService`,
/// so swapping it later is localized).
const DEFAULT_REGISTRY_URL: &str = "https://devnet.chat-kc.logos.co";

use crate::module::{
    module, now_ms, short_label, with_display, with_display_mut, Client, DeliveryState,
    DeliveryStateKind, Display, ModuleState, PERSISTENCE_ENABLED,
};
use crate::persistence::{
    load_state, save_state, AppState, ChatSession, ConversationKind, DisplayMessage,
};

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
    kind: ConversationKind,
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

    // Storage backs libchat's identity and MLS/crypto state. Ephemeral by
    // default (see `PERSISTENCE_ENABLED`): DirectV1 has no reload path yet, so an
    // in-memory store is honest about chats not surviving a restart. The
    // SQLCipher path stays here, behind the switch, for when reload lands.
    let storage = if PERSISTENCE_ENABLED {
        let db_path = format!("{instance_path}/identity.db");
        // Static key derived from the instance path. Not secret; satisfies
        // SQLCipher's keying requirement. A user-provided passphrase is a
        // future enhancement.
        let key = format!("rust-chat-{}", instance_path.replace('/', "_"));
        ChatStorage::new(StorageConfig::Encrypted { path: db_path, key })
            .map_err(|e| InitError::Internal(format!("open store failed: {e:?}")))?
    } else {
        ChatStorage::in_memory()
    };

    // The transport's inbound channel: the bridge worker feeds `inbound_tx` from
    // delivery_module's `messageReceived`, the client's worker drains the rx (via
    // `Transport::inbound`). The subscribe channel carries the core's inbound-address
    // subscriptions to the bridge, which forwards them to delivery_module once the
    // node is started.
    let (inbound_tx, inbound_rx) = crossbeam_channel::unbounded();
    let (subscribe_tx, subscribe_rx) = crossbeam_channel::unbounded();

    // Do the fallible *local* setup (store open, client build) before touching
    // delivery_module. The node's lifecycle is irreversible — createNode rejects
    // duplicates and start is not idempotent (see the TODO in
    // `start_delivery_bootstrap`) — so a build failure must abort init before any
    // node exists; otherwise a partial init would strand a started, unowned node
    // with no workers and no way to stop it. Building the client subscribes the
    // core's inbound addresses, which queue on `subscribe_rx` until the node starts.
    //
    // Identity is ephemeral (see `PERSISTENCE_ENABLED`): a fresh account and
    // delegate are minted each launch. `TestLogosAccount` holds the account key;
    // the `DelegateSigner` is a pure device keypair, and the client composes
    // the account claim into its wire credential from the builder's account
    // address. The account signs a bundle endorsing the delegate's device key,
    // published to the registry's account directory below, so a peer given only
    // the account address resolves this device's key package and opens a
    // DirectV1 conversation. account != device: the client routes on the
    // delegate's signer id; the account address is what we share.
    let account = TestLogosAccount::new();
    let account_addr = account.address();
    let delegate = DelegateSigner::random();
    let device_key = delegate.public_key().clone();
    let (client, events) = ChatClientBuilder::new(account_addr.clone())
        .ident(delegate)
        .transport(SdkDelivery::new(inbound_rx, subscribe_tx))
        .registration(HttpRegistry::new(DEFAULT_REGISTRY_URL))
        .storage(storage)
        .build()
        .map_err(|e| InitError::Internal(format!("client build failed: {e:?}")))?;

    // Endorse the delegate's device key in the registry's account directory so
    // a peer holding only the account address resolves this device's key package.
    // (The client registers its own key package during build.)
    let mut directory = HttpRegistry::new(DEFAULT_REGISTRY_URL);
    account
        .add_delegate_signer(&mut directory, &device_key)
        .map_err(|e| InitError::Internal(format!("publish device bundle failed: {e:?}")))?;
    let intrinsic_name = client.installation_name();
    // The address a peer needs to open a DirectV1 conversation with us: the
    // account address (what `client.addr()` returns). Cached in the display
    // so `get_address` needn't take the client lock.
    let address = account_addr;

    let state_path = PathBuf::from(format!("{instance_path}/history.json"));
    let state = load_display(&state_path);

    // Register listeners before the node starts — `connectionStateChanged`
    // fires during start and is not re-emitted, so a late subscribe misses it.
    // The subscriptions are handed to the bridge worker, which polls them; nothing
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
    let inbound_thread = crate::inbound::spawn_bridge(
        stop.clone(),
        messages_sub,
        conn_sub,
        inbound_tx,
        subscribe_rx,
    );
    let event_thread = crate::inbound::spawn_events(events);

    // Seed the display state read by the getters (the client owns its identity;
    // its intrinsic name is cached here for get_installation_name).
    with_display_mut(|d| {
        d.state = state;
        d.state_path = state_path;
        d.delivery_state = DeliveryState::initialising();
        d.intrinsic_name = intrinsic_name;
        d.address = address;
    });

    Ok(ModuleState {
        client,
        inbound_stop: stop,
        inbound_thread: Some(inbound_thread),
        event_thread: Some(event_thread),
    })
}

/// Bootstrap delivery_module's node and report readiness, asynchronously.
///
/// Called by `lib.rs` *after* the module state is installed and the module lock
/// is released, so the async completion callbacks acquire a free lock and never
/// re-enter it. createNode → start are chained (start rejects until the node
/// exists), and every step runs off the dispatch (Qt event-loop) thread, so
/// bootstrap, which can take tens of seconds, never blocks it.
///
/// Readiness (`online`) is reported once the node has started; the bridge worker
/// then forwards the core's queued inbound-address subscriptions to delivery_module
/// (see `inbound::forward_subscriptions`). We do NOT use delivery's earlier
/// `connectionStateChanged=Connected`, which fires mid-bootstrap ~tens of seconds
/// before the transport can service a call (gating the UI on it lets actions run
/// into the IPC timeout). The bridge worker keeps consuming connectionStateChanged
/// for reconnect/offline handling once we're started.
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

    crate::modules()
        .delivery_module
        .create_node_async(&config_json, move |res| match res {
            Ok(_) => start_node(),
            Err(e) => set_delivery_error(format!("delivery_module.createNode failed: {e}")),
        });
}

/// Bootstrap step 2 of 2: start the node and report readiness. Once online, the
/// bridge worker forwards the core's queued subscriptions (see
/// `inbound::forward_subscriptions`).
fn start_node() {
    crate::modules()
        .delivery_module
        .start_async(move |res| match res {
            Ok(_) => with_display_mut(|d| set_delivery_state(d, DeliveryStateKind::Online, "")),
            Err(e) => set_delivery_error(format!("delivery_module.start failed: {e}")),
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
    // Drop the client so its worker stops and its event sender disconnects; the
    // event consumer then ends its loop and can be joined.
    drop(ms.client);
    if let Some(handle) = ms.event_thread.take() {
        let _ = handle.join();
    }
    with_display_mut(|d| {
        // Final write; nothing left to propagate to, so log a failure.
        if let Err(e) = save_display(d) {
            eprintln!("chat_module: save_state failed on shutdown: {e}");
        }
        *d = Display::default();
    });
}

/// Run `f` with the libchat client under the module lock, mapping "no client"
/// (not initialised) and the unreachable poisoned lock to a [`CoreError`].
fn with_client<R>(f: impl FnOnce(&mut Client) -> R) -> Result<R, CoreError> {
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
    save_display(d).map_err(|e| CoreError::Internal(format!("save_state failed: {e}")))
}

/// Persist the display state, unless persistence is disabled (ephemeral mode),
/// in which case this is a no-op reporting success. See
/// [`module::PERSISTENCE_ENABLED`](crate::module::PERSISTENCE_ENABLED).
fn save_display(d: &Display) -> io::Result<()> {
    if !PERSISTENCE_ENABLED {
        return Ok(());
    }
    save_state(&d.state, &d.state_path)
}

/// Load the display state, or start empty when persistence is disabled
/// (ephemeral mode). See
/// [`module::PERSISTENCE_ENABLED`](crate::module::PERSISTENCE_ENABLED).
fn load_display(path: &Path) -> AppState {
    if !PERSISTENCE_ENABLED {
        return AppState::default();
    }
    load_state(path)
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

/// The local installation address, which a peer needs to open a DirectV1
/// conversation with this installation (pass it to their `create_conversation`).
/// Read from the cached display value, so it returns the empty string before
/// `init`.
pub(crate) fn get_address() -> String {
    with_display(|d| d.address.clone())
}

// ── Conversations ────────────────────────────────────────────────────────────

/// Open a DirectV1 conversation with `peer_address` (the peer's installation
/// address from their `get_address`). This sends an MLS Welcome to the peer; the
/// first message is sent separately via `send_message` once the peer has joined.
/// Returns the local conversation id.
pub(crate) fn create_conversation(peer_address: &str) -> Result<String, CoreError> {
    // libchat op under the client lock. Publish is async (see SdkDelivery), so
    // this returns without blocking on the network.
    let chat_id = with_client(|client| client.create_direct_conversation(peer_address))?
        .map_err(|e| CoreError::Internal(format!("create_conversation failed: {e:?}")))?;

    let peer_label = short_label(&chat_id).to_owned();
    with_display_mut(|d| {
        d.state.chats.insert(
            chat_id.clone(),
            ChatSession {
                chat_id: chat_id.clone(),
                nickname: None,
                kind: ConversationKind::Direct,
                messages: Vec::new(),
            },
        );
        persist(d)
    })?;
    crate::emit_conversation_created(
        &chat_id,
        true,
        &peer_label,
        ConversationKind::Direct.as_str(),
    );

    Ok(chat_id)
}

/// Create a GroupV2 conversation with this installation as its only member;
/// peers are invited afterwards via [`add_group_member`]. Returns the
/// conversation id, which every member observes once joined.
pub(crate) fn create_group_conversation() -> Result<String, CoreError> {
    let chat_id = with_client(|client| client.create_group_conversation(&[]))?
        .map_err(|e| CoreError::Internal(format!("create_group_conversation failed: {e:?}")))?;

    let label = short_label(&chat_id).to_owned();
    with_display_mut(|d| {
        d.state.chats.insert(
            chat_id.clone(),
            ChatSession {
                chat_id: chat_id.clone(),
                nickname: None,
                kind: ConversationKind::Group,
                messages: Vec::new(),
            },
        );
        persist(d)
    })?;
    crate::emit_conversation_created(&chat_id, true, &label, ConversationKind::Group.as_str());

    Ok(chat_id)
}

/// Invite the peer at `peer_address` (all its endorsed devices) into an
/// existing group conversation. The group's steward commits the add and the
/// welcome is delivered asynchronously, so the peer joins some time after
/// this returns.
pub(crate) fn add_group_member(convo_id: &str, peer_address: &str) -> Result<(), CoreError> {
    if !with_display(|d| d.state.chats.contains_key(convo_id)) {
        return Err(CoreError::NotFound);
    }

    with_client(|client| client.add_group_members(convo_id, &[peer_address]))?
        .map_err(|e| CoreError::Internal(format!("add_group_member failed: {e:?}")))?;
    crate::emit_conversation_updated(convo_id);
    Ok(())
}

/// A group member's directory-verified account address, or an empty string when
/// no account is confirmed (an unassociated or unconfirmable member). The empty
/// string is the roster's "no account" signal, which the UI renders as an
/// unknown-account placeholder.
fn member_address(member: logos_generic_chat::GroupMember) -> String {
    member
        .account
        .map(|account| account.as_str().to_string())
        .unwrap_or_default()
}

/// One roster entry — mirrors the `GroupMember` record.
#[derive(Serialize)]
struct GroupMemberRow {
    address: String,
}

/// The roster of the group conversation `convo_id`, one [`GroupMemberRow`] per
/// element. This is a plain list with no error channel, mirroring `get_messages`:
/// an unknown or non-group conversation, or a client error, yields an empty
/// array (the client error is logged).
pub(crate) fn list_group_members(convo_id: &str) -> serde_json::Value {
    let empty = || serde_json::Value::Array(vec![]);
    if !with_display(|d| d.state.chats.contains_key(convo_id)) {
        return empty();
    }
    match with_client(|client| client.group_members(convo_id)) {
        Ok(Ok(members)) => {
            let rows: Vec<GroupMemberRow> = members
                .into_iter()
                .map(|m| GroupMemberRow {
                    address: member_address(m),
                })
                .collect();
            serde_json::to_value(rows).unwrap_or_else(|_| empty())
        }
        Ok(Err(e)) => {
            eprintln!("chat_module: list_group_members failed: {e:?}");
            empty()
        }
        Err(e) => {
            eprintln!("chat_module: list_group_members: {e}");
            empty()
        }
    }
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
                kind: s.kind,
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

    with_client(|client| client.send_message(convo_id, content.as_bytes()))?
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
                sender: None,
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

/// Record a newly-observed conversation (the client's `ConversationStarted`
/// event) and surface it, classed by `kind`. No-op for a locally-deleted or
/// already-known conversation. Called from the event consumer thread; takes only
/// the display lock, so it never waits on the client.
pub(crate) fn record_conversation_started(convo_id: &str, kind: ConversationKind) {
    with_display_mut(|d| {
        // libchat retains crypto state across local deletes, so we still observe
        // events for deleted convos.
        if d.state.deleted.contains(convo_id) || d.state.chats.contains_key(convo_id) {
            return;
        }
        d.state.chats.insert(
            convo_id.to_owned(),
            ChatSession {
                chat_id: convo_id.to_owned(),
                nickname: None,
                kind,
                messages: Vec::new(),
            },
        );
        crate::emit_conversation_created(convo_id, false, short_label(convo_id), kind.as_str());

        // Event consumer has no caller to return to; log a failed write.
        if let Err(e) = save_display(d) {
            eprintln!("chat_module: save_state failed after conversation started: {e}");
        }
    });
}

/// Record an inbound message (the client's `MessageReceived` event) and surface
/// it. `sender` is the sender's account address (device id if unassociated).
/// No-op for a locally-deleted conversation; an unknown conversation is
/// created defensively (the preceding `ConversationStarted` normally creates it
/// first). Called from the event consumer thread; takes only the display lock.
pub(crate) fn record_message_received(convo_id: &str, content: &[u8], sender: &str) {
    with_display_mut(|d| {
        if d.state.deleted.contains(convo_id) {
            return;
        }
        let text = String::from_utf8_lossy(content).to_string();
        let ts = now_ms();
        let session = d
            .state
            .chats
            .entry(convo_id.to_owned())
            .or_insert_with(|| ChatSession {
                chat_id: convo_id.to_owned(),
                nickname: None,
                // Defensive fallback: ConversationStarted normally creates the
                // session with the real kind before any message lands here.
                kind: ConversationKind::default(),
                messages: Vec::new(),
            });
        session.messages.push(DisplayMessage {
            from_self: false,
            content: text.clone(),
            timestamp_ms: ts,
            sender: Some(sender.to_owned()),
        });
        crate::emit_message_received(convo_id, &text, ts as i64, sender);

        if let Err(e) = save_display(d) {
            eprintln!("chat_module: save_state failed after inbound message: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::member_address;
    use libchat::IdentId;
    use logos_generic_chat::GroupMember;

    /// A verified account surfaces its address; a member with no confirmed
    /// account surfaces the empty "no account" signal, not its device id.
    #[test]
    fn member_address_is_account_or_empty() {
        let verified = GroupMember {
            account: Some(IdentId::new("acct-addr")),
            local_identity: IdentId::new("device-id"),
        };
        assert_eq!(member_address(verified), "acct-addr");

        let no_account = GroupMember {
            account: None,
            local_identity: IdentId::new("device-id"),
        };
        assert_eq!(member_address(no_account), "");
    }
}
