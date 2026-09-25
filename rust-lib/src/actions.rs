//! Business operations. Each is a single semantic operation that owns its
//! locking: operations that call into libchat take the client lock ([`module`])
//! for the call; the read methods and the recording of results take the display
//! lock ([`with_display`]). A mutation takes the client lock then the display
//! lock — never the reverse — so the two can't deadlock. `lib.rs` invokes these
//! from the `ChatModule` trait implementation.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use components::HttpAuthClient;
use logos_account::{Account, AccountError, Ed25519VerifyingKey, CHATSIGNER_CONTEXT};
use logos_generic_chat::{
    ChatClientBuilder, ClientError, ContactRegistry, GroupMetadata, Installation,
    PendingInstallation, RegistryPublishMode, SqliteStore, StorageConfig,
};
use message_store::Direction;

use crate::delivery::{delivery_outcome, SdkDelivery};
use crate::{Conversation, GroupMember, Message, Status};

/// The devnet registry: DirectV1 publishes this installation's key package there
/// and fetches a peer's, and every account's log lives there too. Hardcoded for
/// now; a configurable endpoint is a future enhancement (the wiring is behind
/// libchat's `RegistrationService` and `AuthService`, so swapping it later is
/// localized).
const DEFAULT_REGISTRY_URL: &str = "https://devnet.chat-kc.logos.co";

use crate::module::{
    module, now_ms, short_label, with_display, with_display_mut, Client, DeliveryState,
    DeliveryStateKind, Display, ModuleState, LIBCHAT_PERSISTENCE_ENABLED,
};
use crate::persistence::{self, display_message, ChatSession, ConversationKind};

/// Failure modes for the steady-state methods (post-`initialize`).
#[derive(Debug, thiserror::Error)]
pub(crate) enum CoreError {
    #[error("module not initialised")]
    NotInit,
    #[error("conversation not found")]
    NotFound,
    #[error("{0}")]
    Internal(String),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("conversation is from a previous session and kept for its history only")]
    HistoryOnly,
    #[error("chat.db: {0}")]
    Store(#[from] rusqlite::Error),
}

/// Failure modes for [`initialize`].
#[derive(Debug, thiserror::Error)]
pub(crate) enum InitError {
    #[error("host did not assign an instance persistence path; start the host with a session or config dir")]
    NoPersistencePath,
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Delivery(String),
}

/// Character cap for a conversation-list preview. Mirrored on the UI so live and
/// rehydrated previews agree.
const PREVIEW_MAX_CHARS: usize = 160;

// ── Lifecycle ────────────────────────────────────────────────────────────────

pub(crate) fn initialize() -> Result<ModuleState, InitError> {
    // A host that never set a persistence base path still stamps a context, with
    // the path left empty, so emptiness is the "host not configured" signal.
    let persistence_path = crate::context()
        .map(|ctx| ctx.instance_persistence_path)
        .filter(|path| !path.is_empty())
        .ok_or(InitError::NoPersistencePath)?;
    fs::create_dir_all(&persistence_path).map_err(|e| {
        InitError::Internal(format!("cannot create instance persistence path: {e}"))
    })?;

    // Static key derived from the persistence path, for `identity.db` and
    // `chat.db`. Not secret; satisfies SQLCipher's keying requirement. A
    // user-provided passphrase is a future enhancement.
    let key = format!("rust-chat-{}", persistence_path.replace('/', "_"));

    // Storage backs libchat's identity and MLS/crypto state. Ephemeral by
    // default (see `LIBCHAT_PERSISTENCE_ENABLED`): DirectV1 has no reload path yet, so an
    // in-memory store is honest about conversations not surviving a restart. The
    // SQLCipher path stays here, behind the switch, for when reload lands.
    let storage = if LIBCHAT_PERSISTENCE_ENABLED {
        let db_path = format!("{persistence_path}/identity.db");
        SqliteStore::new(StorageConfig::Encrypted {
            path: db_path,
            key: key.clone(),
        })
        .map_err(|e| InitError::Internal(format!("open store failed: {e}")))?
    } else {
        SqliteStore::in_memory()
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
    // Identity is ephemeral (see `LIBCHAT_PERSISTENCE_ENABLED`): each launch
    // publishes a fresh account whose log endorses a new installation key, so a
    // peer given only the account address resolves this installation and opens
    // a DirectV1 conversation. The same server checks every other participant
    // against their account's log.
    let auth = HttpAuthClient::new(DEFAULT_REGISTRY_URL);
    let installation = register_installation(auth.clone())
        .map_err(|e| InitError::Internal(format!("publish account failed: {e}")))?;
    let transport = SdkDelivery::new(inbound_rx, subscribe_tx);
    // Submit over the registry's HTTP API, which acknowledges each bundle. The
    // delivery wire it offers instead is fire-and-forget, so a rejected bundle
    // would surface only as a peer failing to resolve us much later.
    let registry = ContactRegistry::new(
        transport.publisher(),
        DEFAULT_REGISTRY_URL,
        RegistryPublishMode::Http,
    );
    let (client, events) = ChatClientBuilder::new(installation)
        .transport(transport)
        .registration(registry)
        .auth(auth)
        .storage(storage)
        .build()
        .map_err(|e| InitError::Internal(format!("client build failed: {e}")))?;

    let intrinsic_name = client.installation_name();
    // The address a peer needs to open a DirectV1 conversation with us: the
    // account address. Cached in the display so `get_address` needn't take the
    // client lock.
    let address = client.addr().to_string();

    // The chat list and its messages outlive the client's in-memory state, so a
    // conversation from a previous session reads back, kept for its history only
    // unless the client still holds it.
    let chat_db = persistence::open(&PathBuf::from(format!("{persistence_path}/chat.db")), &key)
        .map_err(|e| InitError::Internal(format!("open chat.db failed: {e}")))?;
    let mut state = persistence::load_state(&chat_db)
        .map_err(|e| InitError::Internal(format!("read chat.db failed: {e}")))?;
    let live = client
        .list_all_conversations()
        .map_err(|e| InitError::Internal(format!("list_all_conversations failed: {e}")))?;
    for session in state.chats.values_mut() {
        session.history_only = !live.contains(&session.chat_id);
    }

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
            tracing::error!("init: subscribe(connectionStateChanged) failed: {e}");
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
        d.chat_db = Some(chat_db);
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

/// A new installation of a new account, whose published log endorses it. The
/// account's key goes out of scope here, so no other installation can join it.
fn register_installation(auth: HttpAuthClient) -> Result<Installation, AccountError> {
    let pending = PendingInstallation::generate();
    let key = Ed25519VerifyingKey::from_canonical_slice(&pending.endorsement_request())?;
    let mut account = Account::new(auth);
    account
        .update()
        .endorse_ed25519_key(CHATSIGNER_CONTEXT.clone(), &key)
        .publish()?;
    Ok(pending.complete(account.addr()))
}

/// The preset this process last created delivery_module's node with. This module
/// never stops the node, so an init after `shutdown` is refused by the node an
/// earlier init created: this module's own, not an adopted one.
static OWN_NODE_PRESET: Mutex<Option<String>> = Mutex::new(None);

fn own_node_preset() -> MutexGuard<'static, Option<String>> {
    OWN_NODE_PRESET.lock().unwrap_or_else(|e| e.into_inner())
}

/// Bootstrap delivery_module's node and report readiness, asynchronously.
///
/// Called by `lib.rs` *after* the module state is installed and the module lock
/// is released, so the async completion callbacks acquire a free lock and never
/// re-enter it. createNode → start are chained (start rejects until the node
/// exists), with a getNodeInfo between them when createNode is refused, and
/// every step runs off the dispatch (Qt event-loop) thread, so bootstrap, which
/// can take tens of seconds, never blocks it.
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
/// consumer. createNode rejects duplicates and start is not idempotent, so the
/// first consumer to bootstrap configures the node for every consumer after it;
/// this module then joins that node and reports it as adopted. Drop these calls
/// once the host bootstraps delivery_module and exposes it ready-to-use.
pub(crate) fn start_delivery_bootstrap(preset: &str) {
    // The layered app-developer shape from delivery_module's docs. Only wrapper
    // keys may sit at the top level: any bare key (a top-level logLevel included)
    // reroutes the config to the legacy flat parser, whose port defaults are
    // fixed values — the layered path defaults every unpinned listening port to
    // 0 (OS-assigned), which is what keeps instances sharing a host apart.
    let config_json = serde_json::json!({
        "mode": "Core",
        "preset": preset,
        "messagingOverrides": { "logLevel": "ERROR" },
    })
    .to_string();

    let preset = preset.to_owned();
    crate::modules()
        .delivery_module
        .create_node_async(&config_json, move |res| match delivery_outcome(res) {
            Ok(_) => {
                *own_node_preset() = Some(preset);
                start_node();
            }
            Err(reason) => join_existing_node(preset, reason),
        });
}

/// After a refused createNode: start the node that already exists, adopted
/// unless this process created it with `preset`, or fail with createNode's
/// `reason` when there is none. delivery_module refuses a second createNode
/// before it reads the config, and answers getNodeInfo only while a node
/// exists, so an answer means the refusal was for that node.
fn join_existing_node(preset: String, reason: String) {
    crate::modules()
        .delivery_module
        .get_node_info_async("Version", move |res| match delivery_outcome(res) {
            Ok(_) => {
                let adopted = own_node_preset().as_deref() != Some(preset.as_str());
                if adopted {
                    tracing::warn!(
                        "delivery_module already has a node this module did not create \
                         with preset {preset}; joining it with the settings it was created with"
                    );
                }
                with_display_mut(|d| d.delivery_state.adopted = adopted);
                start_node();
            }
            Err(_) => set_delivery_error(format!("delivery_module.createNode failed: {reason}")),
        });
}

/// Bootstrap step 2 of 2: start the node and report readiness. Once online, the
/// bridge worker forwards the core's queued subscriptions (see
/// `inbound::forward_subscriptions`).
fn start_node() {
    crate::modules()
        .delivery_module
        .start_async(move |res| match delivery_outcome(res) {
            Ok(_) => with_display_mut(|d| {
                d.delivery_state.started = true;
                set_delivery_state(d, DeliveryStateKind::Online, "");
            }),
            Err(e) => set_delivery_error(format!("delivery_module.start failed: {e}")),
        });
}

/// Record an async-bootstrap failure in delivery_state, which is what logs it.
fn set_delivery_error(detail: String) {
    with_display_mut(|d| set_delivery_state(d, DeliveryStateKind::Error, &detail));
}

/// Consumes `ms`: signals the inbound worker to stop, joins it, and resets the
/// display, closing `chat.db`, so a re-init starts clean. Called by `lib.rs`
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
    with_display_mut(|d| *d = Display::default());
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

/// Write a chat's row to `chat.db`, so a failed write surfaces to the caller
/// instead of being reported as success and then vanishing on the next
/// restart.
fn persist_chat(d: &Display, chat_id: &str) -> Result<(), CoreError> {
    let chat_db = d.chat_db.as_ref().ok_or(CoreError::NotInit)?;
    let session = d.state.chats.get(chat_id).ok_or(CoreError::NotFound)?;
    persistence::save_chat(chat_db, session)?;
    Ok(())
}

/// `None` for an empty string, the empty-means-unset convention shared with
/// nicknames and a group's optional name/description.
fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// A message of the conversation `convo_id`, recorded in the chat of the same
/// id: each conversation is its own chat until a rule bundles several.
fn chat_message(
    convo_id: &str,
    direction: Direction,
    timestamp_ms: u64,
    content: &[u8],
) -> message_store::Message {
    message_store::Message {
        chat_id: convo_id.to_owned(),
        convo_id: convo_id.to_owned(),
        direction,
        sender_account: None,
        sender_installation: None,
        message_id: None,
        timestamp_ms: timestamp_ms as i64,
        content: content.to_vec(),
    }
}

/// Record `message` in `chat.db`, with its chat's row when `chat_changed`, and
/// append it to the chat's list. The list takes it even when the write fails,
/// as the message was already sent or decrypted. No-op for a chat that is gone.
fn record_message(
    d: &mut Display,
    message: message_store::Message,
    chat_changed: bool,
) -> Result<(), CoreError> {
    let Some(session) = d.state.chats.get_mut(&message.chat_id) else {
        return Ok(());
    };
    let chat_db = d.chat_db.as_mut().ok_or(CoreError::NotInit)?;
    let recorded =
        persistence::record_message(chat_db, chat_changed.then_some(&*session), &message);
    session.messages.push(display_message(message));
    recorded?;
    Ok(())
}

/// Fails unless `convo_id` is a conversation of this session:
/// [`CoreError::NotFound`] for an unknown one, [`CoreError::HistoryOnly`] for
/// one kept from a previous session.
fn check_live(convo_id: &str) -> Result<(), CoreError> {
    match with_display(|d| d.state.chats.get(convo_id).map(|s| s.history_only)) {
        None => Err(CoreError::NotFound),
        Some(true) => Err(CoreError::HistoryOnly),
        Some(false) => Ok(()),
    }
}

// ── Identity ─────────────────────────────────────────────────────────────────

pub(crate) fn set_installation_name(name: &str) -> Result<(), CoreError> {
    with_display_mut(|d| {
        let name = non_empty(name);
        let chat_db = d.chat_db.as_ref().ok_or(CoreError::NotInit)?;
        persistence::save_installation_name(chat_db, name.as_deref())?;
        d.state.installation_name = name;
        Ok(())
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
    let chat_id = with_client(|client| client.create_direct_conversation(peer_address))??;

    tracing::info!("created direct conversation {chat_id}");
    let peer_label = short_label(&chat_id).to_owned();
    with_display_mut(|d| {
        d.state.chats.insert(
            chat_id.clone(),
            ChatSession {
                chat_id: chat_id.clone(),
                nickname: None,
                kind: ConversationKind::Direct,
                name: None,
                description: None,
                peer: Some(peer_address.to_owned()),
                messages: Vec::new(),
                older_messages: 0,
                history_only: false,
            },
        );
        persist_chat(d, &chat_id)
    })?;
    crate::emit_conversation_created(
        &chat_id,
        true,
        &peer_label,
        ConversationKind::Direct.as_str(),
        "",
        "",
    );

    Ok(chat_id)
}

/// Create a GroupV2 conversation with this installation as its only member;
/// peers are invited afterwards via [`add_group_member`]. `name` and `desc` are
/// the group's shared metadata, carried to every joiner; both may be empty.
/// Returns the conversation id, which every member observes once joined.
pub(crate) fn create_group_conversation(name: &str, desc: &str) -> Result<String, CoreError> {
    let chat_id = with_client(|client| {
        client.create_group_conversation(&[], GroupMetadata::new(name, desc))
    })??;

    tracing::info!("created group conversation {chat_id}");

    let label = short_label(&chat_id).to_owned();
    with_display_mut(|d| {
        d.state.chats.insert(
            chat_id.clone(),
            ChatSession {
                chat_id: chat_id.clone(),
                nickname: None,
                kind: ConversationKind::Group,
                name: non_empty(name),
                description: non_empty(desc),
                peer: None,
                messages: Vec::new(),
                older_messages: 0,
                history_only: false,
            },
        );
        persist_chat(d, &chat_id)
    })?;
    crate::emit_conversation_created(
        &chat_id,
        true,
        &label,
        ConversationKind::Group.as_str(),
        name,
        desc,
    );

    Ok(chat_id)
}

/// Invite every installation the account at `peer_address` endorses into an
/// existing group conversation. The group's steward commits the add and the
/// welcome is delivered asynchronously, so the peer joins some time after
/// this returns.
pub(crate) fn add_group_member(convo_id: &str, peer_address: &str) -> Result<(), CoreError> {
    check_live(convo_id)?;

    with_client(|client| client.add_group_participants(convo_id, &[peer_address]))??;
    crate::emit_conversation_updated(convo_id);
    Ok(())
}

/// The roster of the conversation `convo_id`, one [`GroupMember`] per
/// installation: its committed members, then the invites whose commit has not
/// landed; a direct conversation reports both participants. This is a plain
/// list with no error channel, mirroring `get_messages`: an unknown
/// conversation, or a client error, yields an empty array (the client error is
/// logged).
pub(crate) fn list_group_members(convo_id: &str) -> Vec<GroupMember> {
    if check_live(convo_id).is_err() {
        return Vec::new();
    }
    let roster = with_client(|client| {
        Ok::<_, ClientError>((client.members(convo_id)?, client.pending_members(convo_id)?))
    });
    match roster {
        Ok(Ok((committed, invited))) => committed
            .into_iter()
            .map(|member| (member, false))
            .chain(invited.into_iter().map(|member| (member, true)))
            .map(|(member, pending)| GroupMember {
                address: member.account.to_string(),
                pending,
            })
            .collect(),
        Ok(Err(e)) => {
            tracing::warn!("list_group_members failed: {e}");
            Vec::new()
        }
        Err(e) => {
            tracing::warn!("list_group_members: {e}");
            Vec::new()
        }
    }
}

pub(crate) fn list_conversations() -> Vec<Conversation> {
    with_display(|d| {
        d.state
            .chats
            .values()
            .map(|s| Conversation {
                convo_id: s.chat_id.clone(),
                nickname: s.nickname.clone(),
                message_count: (s.older_messages + s.messages.len()) as i64,
                last_activity_ms: s.messages.last().map_or(0, |m| m.timestamp_ms as i64),
                kind: s.kind.as_str().to_string(),
                name: s.name.clone(),
                description: s.description.clone(),
                preview: s
                    .messages
                    .last()
                    .map(|m| m.content.chars().take(PREVIEW_MAX_CHARS).collect()),
                history_only: s.history_only,
            })
            .collect()
    })
}

pub(crate) fn get_messages(convo_id: &str) -> Vec<Message> {
    with_display(|d| {
        d.state
            .chats
            .get(convo_id)
            .map(|s| s.messages.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|m| Message {
                from_self: m.from_self,
                content: m.content.clone(),
                timestamp_ms: m.timestamp_ms as i64,
                sender: m.sender.clone(),
            })
            .collect()
    })
}

pub(crate) fn send_message(convo_id: &str, content: &str) -> Result<(), CoreError> {
    // The convo must exist before we encrypt+send. A concurrent delete between
    // this check and the record below is a benign race (the message goes out but
    // isn't kept for a convo the user just removed).
    check_live(convo_id)?;

    with_client(|client| client.send_message(convo_id, content.as_bytes()))??;

    // Size, never the text: this file is handed to whoever is diagnosing a run,
    // and the one thing a chat log must not leak is what was said.
    tracing::info!("sent {} bytes to {convo_id}", content.len());

    let ts = now_ms();
    // Record before emitting: the message is already on the wire, but if the
    // local write fails we report failure rather than paint a "sent" bubble
    // the next restart would lose.
    with_display_mut(|d| {
        record_message(
            d,
            chat_message(convo_id, Direction::Sent, ts, content.as_bytes()),
            false,
        )
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
        persist_chat(d, convo_id)
    })?;
    crate::emit_conversation_updated(convo_id);
    Ok(())
}

pub(crate) fn delete_conversation(convo_id: &str) -> Result<(), CoreError> {
    with_display_mut(|d| {
        if !d.state.chats.contains_key(convo_id) {
            return Err(CoreError::NotFound);
        }
        let chat_db = d.chat_db.as_mut().ok_or(CoreError::NotInit)?;
        persistence::delete_chat(chat_db, convo_id)?;
        d.state.chats.remove(convo_id);
        d.state.deleted.insert(convo_id.to_owned());
        Ok(())
    })?;
    crate::emit_conversation_deleted(convo_id);
    Ok(())
}

// ── Status ───────────────────────────────────────────────────────────────────

pub(crate) fn status() -> Status {
    with_display(|d| Status {
        convo_count: d.state.chats.len() as i64,
        delivery_state: d.delivery_state.state.as_str().to_string(),
        detail: d.delivery_state.detail.clone(),
        delivery_adopted: d.delivery_state.adopted,
    })
}

// ── Inbound-side helpers (called by inbound.rs worker) ───────────────────────

/// Update delivery state and emit a plugin event. No-op if `state` matches
/// the current value. Operates on the display, which holds delivery_state.
pub(crate) fn set_delivery_state(d: &mut Display, state: DeliveryStateKind, detail: &str) {
    if d.delivery_state.state == state && d.delivery_state.detail == detail {
        return;
    }
    d.delivery_state.state = state;
    d.delivery_state.detail = detail.to_owned();
    // The transitions, not the polling: this returns early while the state
    // stands, so a line here is one thing actually changing.
    match (state, detail) {
        (DeliveryStateKind::Error, _) => tracing::error!("delivery failed: {detail}"),
        (_, "") => tracing::info!("delivery is {}", state.as_str()),
        _ => tracing::info!("delivery is {}: {detail}", state.as_str()),
    }
    crate::emit_delivery_state_changed(state.as_str(), detail, d.delivery_state.adopted);
}

/// Record a newly-observed conversation (the client's `ConversationStarted`
/// event) and surface it, classed by `kind`. No-op for a locally-deleted or
/// already-known conversation. Called from the event consumer thread; a group
/// first reads its shared metadata under the client lock, then records under the
/// display lock (the two are never held at once).
pub(crate) fn record_conversation_started(convo_id: &str, kind: ConversationKind) {
    // A joiner learns a group's name and description from the client, not from a
    // local argument; a direct conversation carries none. Read it before taking
    // the display lock so the client and display locks are never nested.
    let (name, description) = if kind == ConversationKind::Group {
        match with_client(|client| client.group_metadata(convo_id)) {
            Ok(Ok(meta)) => (non_empty(&meta.name), non_empty(&meta.desc)),
            Ok(Err(e)) => {
                tracing::warn!("group_metadata failed: {e}");
                (None, None)
            }
            Err(e) => {
                tracing::warn!("group_metadata: {e}");
                (None, None)
            }
        }
    } else {
        (None, None)
    };
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
                name: name.clone(),
                description: description.clone(),
                peer: None,
                messages: Vec::new(),
                older_messages: 0,
                history_only: false,
            },
        );
        crate::emit_conversation_created(
            convo_id,
            false,
            short_label(convo_id),
            kind.as_str(),
            name.as_deref().unwrap_or(""),
            description.as_deref().unwrap_or(""),
        );

        // Event consumer has no caller to return to; log a failed write.
        if let Err(e) = persist_chat(d, convo_id) {
            tracing::error!("saving a conversation failed after it started: {e}");
        }
    });
}

/// Record an inbound message (the client's `MessageReceived` event) and surface
/// it. `account` is the sender's account address, `installation` the key of
/// the installation it sent from. No-op for a locally-deleted conversation; an
/// unknown conversation is created defensively (the preceding
/// `ConversationStarted` normally creates it first). Called from the event
/// consumer thread; takes only the display lock.
pub(crate) fn record_message_received(
    convo_id: &str,
    content: &[u8],
    account: &str,
    installation: &str,
) {
    with_display_mut(|d| {
        if d.state.deleted.contains(convo_id) {
            return;
        }
        tracing::info!(
            "received {} bytes in {convo_id} from {}",
            content.len(),
            short_label(account)
        );
        let ts = now_ms();
        let mut chat_changed = !d.state.chats.contains_key(convo_id);
        let session = d
            .state
            .chats
            .entry(convo_id.to_owned())
            .or_insert_with(|| ChatSession {
                chat_id: convo_id.to_owned(),
                nickname: None,
                // Defensive fallback: ConversationStarted normally creates the
                // session with the real kind and metadata before any message
                // lands here.
                kind: ConversationKind::default(),
                name: None,
                description: None,
                peer: None,
                messages: Vec::new(),
                older_messages: 0,
                history_only: false,
            });
        if session.kind == ConversationKind::Direct && session.peer.is_none() {
            session.peer = Some(account.to_owned());
            chat_changed = true;
        }
        let message = message_store::Message {
            sender_account: Some(account.to_owned()),
            sender_installation: Some(installation.to_owned()),
            ..chat_message(convo_id, Direction::Received, ts, content)
        };
        // Event consumer has no caller to return to; log a failed write.
        if let Err(e) = record_message(d, message, chat_changed) {
            tracing::error!("recording an inbound message failed: {e}");
        }
        crate::emit_message_received(
            convo_id,
            &String::from_utf8_lossy(content),
            ts as i64,
            account,
        );
    });
}

/// Surface a group roster change (the client's `ConversationMembersChanged`
/// event). No-op for a locally-deleted or unknown conversation. Called from the
/// event consumer thread; takes only the display lock, so it never waits on the
/// client.
pub(crate) fn record_members_changed(convo_id: &str) {
    with_display(|d| {
        if d.state.deleted.contains(convo_id) || !d.state.chats.contains_key(convo_id) {
            return;
        }
        crate::emit_members_changed(convo_id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "rust-chat-test";

    fn direct_chat(chat_id: &str) -> ChatSession {
        ChatSession {
            chat_id: chat_id.into(),
            nickname: None,
            kind: ConversationKind::Direct,
            name: None,
            description: None,
            peer: None,
            messages: Vec::new(),
            older_messages: 0,
            history_only: false,
        }
    }

    #[test]
    fn recorded_messages_read_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        let mut d = Display {
            chat_db: Some(persistence::open(&path, KEY).unwrap()),
            ..Display::default()
        };
        d.state
            .chats
            .insert("convo-1".into(), direct_chat("convo-1"));
        record_message(
            &mut d,
            chat_message("convo-1", Direction::Sent, 1, b"hi raya"),
            true,
        )
        .unwrap();
        let received = message_store::Message {
            sender_account: Some("raya-account".into()),
            sender_installation: Some("raya-device".into()),
            ..chat_message("convo-1", Direction::Received, 2, b"hi saro")
        };
        record_message(&mut d, received, false).unwrap();
        drop(d);

        let state = persistence::load_state(&persistence::open(&path, KEY).unwrap()).unwrap();

        let read_back: Vec<_> = state.chats["convo-1"]
            .messages
            .iter()
            .map(|m| {
                (
                    m.from_self,
                    m.content.as_str(),
                    m.timestamp_ms,
                    m.sender.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            read_back,
            [
                (true, "hi raya", 1, None),
                (false, "hi saro", 2, Some("raya-account")),
            ]
        );
    }
}
