//! Singleton module state plus pure helpers.
//!
//! `ModuleState` is reached only via [`ModuleHandle`]'s closure-based
//! accessors, which hold the mutex for the closure's duration. That's
//! the invariant that makes `unsafe impl Send for ModuleState` sound.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use client::ChatClient;
use logos_rust_sdk::PluginProxy;
use serde::Serialize;

use crate::delivery::SdkDelivery;
use crate::events::EventQueue;
use crate::persistence::AppState;

// ── Delivery state ──────────────────────────────────────────────────────────

/// Serialises lowercase on the wire. `Initialising` covers the gap
/// between a successful init and the first `connectionStateChanged`
/// event — distinct from `Stopped`, which means not initialised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DeliveryStateKind {
    Initialising,
    Online,
    Error,
    Stopped,
}

/// Mirrors the `state` payload of the `deliveryStateChanged` plugin event.
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
}

// ── ModuleState ───────────────────────────────────────────────────────────────

pub(crate) struct ModuleState {
    pub client: ChatClient<SdkDelivery>,
    pub state: AppState,
    pub state_path: PathBuf,
    pub event_queue: EventQueue,
    pub delivery_state: DeliveryState,
    /// Signal flag for the inbound worker. The worker observes this between
    /// poll iterations so shutdown() bounds wait time at one poll period.
    pub inbound_stop: Arc<AtomicBool>,
    /// Inbound worker handle. `Option` so `shutdown()` can `take()` it before
    /// `join`-ing while still under the module mutex.
    pub inbound_thread: Option<JoinHandle<()>>,
    /// Dropping this proxy frees `Box<EventCallbackData>` allocations that
    /// the C side holds raw pointers into — keep alive for the module's
    /// lifetime.
    pub _delivery_subscriptions: PluginProxy,
}

// `ChatClient` holds `Rc<…>` so `ModuleState` isn't `Send` by auto-derive.
// Sound because every access goes through `ModuleHandle` under the mutex,
// which both serialises Rc refcount updates and provides the happens-before
// barrier the non-atomic refcount needs. `Mutex<T>: Sync` falls out from
// `T: Send`, so we assert `Send` only.
unsafe impl Send for ModuleState {}

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

    pub(crate) fn with_state<R>(
        &self,
        f: impl FnOnce(&ModuleState) -> R,
    ) -> Result<Option<R>, LockPoisoned> {
        let guard = self.inner.lock().map_err(|_| LockPoisoned)?;
        Ok(guard.as_ref().map(f))
    }

    pub(crate) fn with_state_mut<R>(
        &self,
        f: impl FnOnce(&mut ModuleState) -> R,
    ) -> Result<Option<R>, LockPoisoned> {
        let mut guard = self.inner.lock().map_err(|_| LockPoisoned)?;
        Ok(guard.as_mut().map(f))
    }

    /// Like `with_state` but invokes `f` even when no state is installed.
    pub(crate) fn with_state_optional<R>(
        &self,
        f: impl FnOnce(Option<&ModuleState>) -> R,
    ) -> Result<R, LockPoisoned> {
        let guard = self.inner.lock().map_err(|_| LockPoisoned)?;
        Ok(f(guard.as_ref()))
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

// ── Pure helpers ──────────────────────────────────────────────────────────────

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Short hex-ish label derived from a `chat_id` — used as a fallback when
/// no nickname is set, and as the `peer_label` on `ConversationCreated`
/// events.
pub(crate) fn short_label(chat_id: &str) -> &str {
    &chat_id[..8.min(chat_id.len())]
}

/// Display name for the local installation: user override if set, otherwise
/// libchat's intrinsic `installation_name()`.
pub(crate) fn effective_installation_name(ms: &ModuleState) -> String {
    ms.state
        .installation_name
        .clone()
        .unwrap_or_else(|| ms.client.installation_name().to_owned())
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
