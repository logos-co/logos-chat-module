//! Singleton module state plus pure helpers.
//!
//! State is split across two independent locks so the fast read methods never
//! wait on slow libchat work:
//! * [`ModuleHandle`] (the [`module`] singleton) guards the [`Client`] and the
//!   background worker handles. The outbound calls (create/send) take this lock
//!   for the libchat crypto; they run on the dispatch thread and return without
//!   blocking on the network (publish is async).
//! * [`with_display`]/[`with_display_mut`] guard the display history
//!   ([`Display`]): the conversation log, delivery state, and cached identity
//!   name. The read methods (`get_messages`/`list_conversations`/`status`/
//!   `get_installation_name`) lock only this, so they return promptly even while
//!   the client lock is held for a long send.
//!
//! A mutation locks the client (for the libchat call) then the display (to
//! record the result) — never the reverse — so the two locks can't deadlock.
//! Inbound decryption runs inside the client's own worker; the event consumer
//! takes only the display lock to record the result, so it never waits on the
//! client lock either.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use libchat::ChatStorage;
use logos_chat::{ChatClient, HttpRegistry};
use serde::Serialize;

use crate::delivery::ModuleTransport;
use crate::persistence::AppState;

/// The chat client as this module configures it: a delegate identity associated
/// with an ephemeral account, the [`ModuleTransport`] chosen at init (delivery_module
/// or the centralized relay), the devnet HTTP registry, and an in-memory store.
/// Chats are ephemeral (see [`PERSISTENCE_ENABLED`]).
pub(crate) type Client = ChatClient<ModuleTransport, HttpRegistry, ChatStorage>;

/// Whether chat state persists across restarts. Off: identity, MLS/crypto state,
/// and the display history are all ephemeral. DirectV1 has no reload path in
/// libchat yet (a DirectV1 conversation's MLS state is never reloaded), so
/// persisting would strand crypto state a restart can't resume. The persistence
/// code (SQLCipher store, `history.json`) is kept behind this switch; flip it on
/// once libchat can reload DirectV1 state.
pub(crate) const PERSISTENCE_ENABLED: bool = false;

// ── Delivery state ──────────────────────────────────────────────────────────

/// Serialises lowercase on the wire. `Initialising` covers the gap between a
/// successful init and delivery finishing startup (the start/subscribe handshake
/// in `actions::initialize`), at which point we report `Online` — distinct from
/// `Stopped`, which means not initialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DeliveryStateKind {
    Initialising,
    Online,
    Error,
    Stopped,
}

impl DeliveryStateKind {
    /// Lowercase wire form — the `delivery_state` value carried by the
    /// `delivery_state_changed` event. Matches the serde `rename_all` form
    /// used when this enum is serialised inside `status`.
    pub fn as_str(self) -> &'static str {
        match self {
            DeliveryStateKind::Initialising => "initialising",
            DeliveryStateKind::Online => "online",
            DeliveryStateKind::Error => "error",
            DeliveryStateKind::Stopped => "stopped",
        }
    }
}

/// Mirrors the `delivery_state`/`detail` payload of the `delivery_state_changed`
/// event and the same fields in `status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryState {
    pub state: DeliveryStateKind,
    pub detail: String,
}

impl DeliveryState {
    pub fn initialising() -> Self {
        Self {
            state: DeliveryStateKind::Initialising,
            detail: String::new(),
        }
    }

    pub fn stopped() -> Self {
        Self {
            state: DeliveryStateKind::Stopped,
            detail: String::new(),
        }
    }
}

// ── ModuleState (client lock) ──────────────────────────────────────────────────

pub(crate) struct ModuleState {
    pub client: Client,
    /// Signal flag for the stop-driven inbound worker. It observes this between
    /// iterations so shutdown() bounds the wait: one poll period for the
    /// delivery_module bridge, one long-poll for the mailbox poller.
    pub inbound_stop: Arc<AtomicBool>,
    /// The stop-driven inbound worker handle: the delivery_module bridge
    /// (events → client, connection state, subscription forwarding), or the
    /// mailbox poller (relay long-poll → client). `Option` so `shutdown()` can
    /// `take()` it before `join`-ing while still under the module mutex.
    pub inbound_thread: Option<JoinHandle<()>>,
    /// Mailbox sender worker handle (outbound queue → relay); `None` on the
    /// delivery_module path. Joined after the client drops, since it ends when its
    /// outbound sender disconnects.
    pub mailbox_send_thread: Option<JoinHandle<()>>,
    /// Event consumer worker handle. Drains the client's `Receiver<Event>`; it
    /// exits once the client is dropped and the event sender disconnects.
    pub event_thread: Option<JoinHandle<()>>,
}

/// Unreachable under `panic = "abort"` — the process dies before a
/// poisoning panic can return.
#[derive(Debug)]
pub(crate) struct LockPoisoned;

pub(crate) struct ModuleHandle {
    inner: Mutex<Option<ModuleState>>,
}

impl ModuleHandle {
    const fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub(crate) fn with_state_mut<R>(
        &self,
        f: impl FnOnce(&mut ModuleState) -> R,
    ) -> Result<Option<R>, LockPoisoned> {
        let mut guard = self.inner.lock().map_err(|_| LockPoisoned)?;
        Ok(guard.as_mut().map(f))
    }

    /// Install a fresh state via `f`. No-op if one is already installed.
    pub(crate) fn install_with<E>(
        &self,
        f: impl FnOnce() -> Result<ModuleState, E>,
    ) -> Result<Result<InstallOutcome, E>, LockPoisoned> {
        let mut guard = self.inner.lock().map_err(|_| LockPoisoned)?;
        if guard.is_some() {
            return Ok(Ok(InstallOutcome::AlreadyInstalled));
        }
        match f() {
            Ok(state) => {
                *guard = Some(state);
                Ok(Ok(InstallOutcome::Installed))
            }
            Err(e) => Ok(Err(e)),
        }
    }

    pub(crate) fn take(&self) -> Result<Option<ModuleState>, LockPoisoned> {
        let mut guard = self.inner.lock().map_err(|_| LockPoisoned)?;
        Ok(guard.take())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

static MODULE: OnceLock<ModuleHandle> = OnceLock::new();

pub(crate) fn module() -> &'static ModuleHandle {
    MODULE.get_or_init(ModuleHandle::new)
}

// ── Display (display lock) ──────────────────────────────────────────────────

/// Everything the read methods need, behind its own lock so they never wait on
/// the client lock. Populated by `actions::initialize`, mutated after each
/// libchat op (and by the client-free mutators), reset on shutdown.
pub(crate) struct Display {
    pub state: AppState,
    pub state_path: PathBuf,
    pub delivery_state: DeliveryState,
    /// libchat's intrinsic installation name, cached so `get_installation_name`
    /// needn't touch the client (which is behind the other lock).
    pub intrinsic_name: String,
    /// The local installation address (libchat `ChatClient::addr`), cached so
    /// `get_address` needn't touch the client. A peer needs this value to open a
    /// DirectV1 conversation with this installation.
    pub address: String,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            state: AppState::default(),
            state_path: PathBuf::new(),
            delivery_state: DeliveryState::stopped(),
            intrinsic_name: String::new(),
            address: String::new(),
        }
    }
}

static DISPLAY: OnceLock<Mutex<Display>> = OnceLock::new();

fn display() -> &'static Mutex<Display> {
    DISPLAY.get_or_init(|| Mutex::new(Display::default()))
}

/// Read the display state. `panic = "abort"` makes lock poisoning unreachable;
/// the `into_inner` recovery is a belt-and-braces no-op rather than a panic.
pub(crate) fn with_display<R>(f: impl FnOnce(&Display) -> R) -> R {
    let guard = display().lock().unwrap_or_else(|e| e.into_inner());
    f(&guard)
}

/// Mutate the display state. See [`with_display`] on poisoning.
pub(crate) fn with_display_mut<R>(f: impl FnOnce(&mut Display) -> R) -> R {
    let mut guard = display().lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

// ── Pure helpers ──────────────────────────────────────────────────────────────

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Short label derived from a `chat_id` — its first 8 characters, used as a
/// fallback when no nickname is set and as the `peer_label` on
/// `ConversationCreated` events. Slices on a char boundary so a future
/// non-ASCII id can't panic (which, under `panic = "abort"`, would abort the
/// host process).
pub(crate) fn short_label(chat_id: &str) -> &str {
    let end = chat_id
        .char_indices()
        .nth(8)
        .map_or(chat_id.len(), |(i, _)| i);
    &chat_id[..end]
}

/// Display name for the local installation: user override if set, otherwise
/// libchat's intrinsic name (cached in `Display::intrinsic_name`, since the
/// owning client is behind the other lock).
pub(crate) fn effective_installation_name(d: &Display) -> String {
    d.state
        .installation_name
        .clone()
        .unwrap_or_else(|| d.intrinsic_name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pins the lowercase wire format consumers parse against.
    #[test]
    fn delivery_state_kind_serialises_to_lowercase() {
        let to_json = |k: DeliveryStateKind| serde_json::to_value(k).unwrap();
        assert_eq!(to_json(DeliveryStateKind::Initialising), "initialising");
        assert_eq!(to_json(DeliveryStateKind::Online), "online");
        assert_eq!(to_json(DeliveryStateKind::Error), "error");
        assert_eq!(to_json(DeliveryStateKind::Stopped), "stopped");
    }
}
