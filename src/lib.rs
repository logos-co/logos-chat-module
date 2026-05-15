//! `extern "C"` surface for the chat_module Logos Module. The C ABI
//! contract that consumers read is the `///` doc on
//! [`CHAT_MODULE_ABI_VERSION`] (propagated verbatim to the generated
//! `chat_module.h`).

mod actions;
mod delivery;
mod events;
mod inbound;
mod module;
mod panic_hook;
mod persistence;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use module::{module, InstallOutcome};

/// chat_module — C ABI boundary.
///
/// A `c-ffi` Logos Module loaded by liblogos_core. Wraps libchat for the
/// e2e-encrypted chat protocol and reaches the network via
/// delivery_module. The codegen-derived Qt plugin glue marshals each
/// function below as a Q_INVOKABLE method (with the `chat_module_`
/// prefix stripped).
///
/// Thread safety. Functions that touch module state serialise on an
/// internal mutex and are safe to call from any thread.
/// `chat_module_free_string` is stateless and never contends.
///
/// `chat_module_init` must run on a Qt event-loop thread for its first
/// successful call: it subscribes to delivery_module's plugin events,
/// and QtRO's `acquireDynamic` + `waitForSource` deadlock otherwise.
/// Re-init calls have no such restriction.
///
/// Memory ownership. Functions returning `char *` return a heap-allocated
/// UTF-8 string the caller must free with `chat_module_free_string`.
/// NULL means failure (per each function's doc).
///
/// Security. The identity store opened by `chat_module_init` is keyed
/// from `instance_path` — obfuscated, not protected. Passphrase UX is
/// planned.
pub const CHAT_MODULE_ABI_VERSION: u32 = 1;

// ── Error codes ──────────────────────────────────────────────────────────────
//
// These `pub const` items are emitted into the generated C header by cbindgen
// as `#define CHAT_MODULE_<NAME>` macros (see `cbindgen.toml`).

/// Success.
pub const CHAT_MODULE_OK: i32 = 0;
/// Module is not initialised (or has been shut down).
pub const CHAT_MODULE_ERR_NOT_INIT: i32 = -1;
/// A required argument was NULL or otherwise malformed.
pub const CHAT_MODULE_ERR_BAD_ARG: i32 = -3;
/// `delivery_module` IPC or operation failure.
pub const CHAT_MODULE_ERR_DELIVERY: i32 = -4;
/// Internal error (lock poison, allocation failure, persistence error, …).
pub const CHAT_MODULE_ERR_INTERNAL: i32 = -5;
/// `convo_id` (or other lookup key) is unknown.
pub const CHAT_MODULE_ERR_NOT_FOUND: i32 = -6;

// ── Lifecycle ────────────────────────────────────────────────────────────────

/// Initialise the module. Idempotent — subsequent calls return
/// `CHAT_MODULE_OK` and log a warning; arguments are ignored. Call
/// `chat_module_shutdown` first to reconfigure.
///
/// Opens `instance_path/identity.db`, bootstraps `delivery_module`
/// (`createNode` + `start` + `subscribe`), and spawns the inbound worker.
///
/// - `instance_path`   – writable directory owned by this module instance
/// - `delivery_preset` – e.g. "logos.dev" (defaults to "logos.dev" if NULL)
/// - `tcp_port`        – TCP port for the embedded Waku node (e.g. 60000)
///
/// Returns `CHAT_MODULE_OK`, or `CHAT_MODULE_ERR_BAD_ARG` (NULL or
/// non-UTF-8 `instance_path`), `CHAT_MODULE_ERR_DELIVERY` (delivery_module
/// setup failure), or `CHAT_MODULE_ERR_INTERNAL` (persistence / lock poison).
#[no_mangle]
pub extern "C" fn chat_module_init(
    instance_path: *const c_char,
    delivery_preset: *const c_char,
    tcp_port: i32,
) -> i32 {
    panic_hook::install_once();

    let instance_path = match unsafe_str(instance_path) {
        Some(s) => s,
        None => return CHAT_MODULE_ERR_BAD_ARG,
    };
    let preset = unsafe_str(delivery_preset).unwrap_or("logos.dev");

    let outcome = module().install_with(|| actions::initialize(instance_path, preset, tcp_port));
    match outcome {
        Err(_) => CHAT_MODULE_ERR_INTERNAL,
        Ok(Ok(InstallOutcome::Installed)) => CHAT_MODULE_OK,
        Ok(Ok(InstallOutcome::AlreadyInstalled)) => {
            eprintln!(
                "chat_module_init: already initialised; any new arguments are \
                 ignored, call chat_module_shutdown() first to reconfigure"
            );
            CHAT_MODULE_OK
        }
        Ok(Err(e)) => {
            eprintln!("chat_module_init: {e}");
            e.to_code()
        }
    }
}

/// Stop the inbound thread and persist pending state. Does **not** stop
/// `delivery_module` — shared-module lifecycle belongs to `liblogos_core`,
/// not to dependents. Idempotent.
#[no_mangle]
pub extern "C" fn chat_module_shutdown() -> i32 {
    // Take the module out of the lock before joining the inbound thread —
    // the worker may try to re-acquire the mutex to handle a pending event,
    // and holding it during `join` would deadlock.
    let taken = match module().take() {
        Ok(t) => t,
        Err(_) => return CHAT_MODULE_ERR_INTERNAL,
    };

    if let Some(ms) = taken {
        actions::shutdown(ms);
    }

    CHAT_MODULE_OK
}

// ── Identity ─────────────────────────────────────────────────────────────────

/// Return the installation name (identity label).
///
/// Caller must free the returned string with `chat_module_free_string()`.
/// Returns NULL if the module is not initialised or the internal lock is
/// poisoned.
#[no_mangle]
pub extern "C" fn chat_module_get_installation_name() -> *mut c_char {
    match module().with_state(module::effective_installation_name) {
        Ok(Some(name)) => to_c_string(&name),
        _ => std::ptr::null_mut(),
    }
}

/// Set a local override for the displayed installation name.
///
/// libchat itself does not currently expose a rename API; this override is
/// persisted in `history.json` and returned by
/// `chat_module_get_installation_name()` instead of the libchat-internal
/// value. Pass an empty string to clear the override and fall back to
/// libchat's intrinsic name.
///
/// Returns `CHAT_MODULE_OK` on success, `CHAT_MODULE_ERR_NOT_INIT` if not
/// initialised, `CHAT_MODULE_ERR_BAD_ARG` on a NULL pointer, or
/// `CHAT_MODULE_ERR_INTERNAL` if the lock is poisoned.
#[no_mangle]
pub extern "C" fn chat_module_set_installation_name(name: *const c_char) -> i32 {
    let name = match unsafe_str(name) {
        Some(s) => s,
        None => return CHAT_MODULE_ERR_BAD_ARG,
    };
    match module().with_state_mut(|ms| actions::set_installation_name(ms, name)) {
        Err(_) => CHAT_MODULE_ERR_INTERNAL,
        Ok(None) => CHAT_MODULE_ERR_NOT_INIT,
        Ok(Some(())) => CHAT_MODULE_OK,
    }
}

/// Produce this installation's intro bundle as a UTF-8 string in libchat's
/// native exchange format (`"logos_chatintro_1_<...>"`).
///
/// Caller must free with `chat_module_free_string()`.
/// Returns NULL on failure.
#[no_mangle]
pub extern "C" fn chat_module_create_intro_bundle() -> *mut c_char {
    let outcome = module().with_state_mut(actions::create_intro_bundle);
    match outcome {
        Ok(Some(Ok(s))) => to_c_string_strict(&s).unwrap_or_else(|| {
            eprintln!(
                "chat_module_create_intro_bundle: bundle contained an interior NUL; returning NULL"
            );
            std::ptr::null_mut()
        }),
        Ok(Some(Err(e))) => {
            eprintln!("chat_module_create_intro_bundle: {e}");
            std::ptr::null_mut()
        }
        _ => std::ptr::null_mut(),
    }
}

// ── Conversations ─────────────────────────────────────────────────────────────

/// Initiate a private conversation with a peer using their intro bundle, then
/// deliver all outbound protocol envelopes.
///
/// - `peer_intro_bundle` – the value `chat_module_create_intro_bundle` returns
///   on the peer's side.
/// - `initial_content` – UTF-8 text for the opening message (may be empty `""`).
///
/// Returns the new conversation ID (heap-allocated string) or NULL on failure.
/// Caller must free with `chat_module_free_string()`.
#[no_mangle]
pub extern "C" fn chat_module_create_conversation(
    peer_intro_bundle: *const c_char,
    initial_content: *const c_char,
) -> *mut c_char {
    let bundle = match unsafe_str(peer_intro_bundle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };
    let content = unsafe_str(initial_content).unwrap_or("");

    let outcome = module().with_state_mut(|ms| actions::create_conversation(ms, bundle, content));
    match outcome {
        Ok(Some(Ok(chat_id))) => to_c_string(&chat_id),
        Ok(Some(Err(e))) => {
            eprintln!("chat_module_create_conversation: {e}");
            std::ptr::null_mut()
        }
        _ => std::ptr::null_mut(),
    }
}

/// Return a JSON array of conversation objects:
///   `[{"convo_id":"...","nickname":"...","message_count":N,"last_ts":N}, ...]`
///
/// Caller must free with `chat_module_free_string()`.
/// Returns `"[]"` when there are no conversations (success), NULL on error or
/// if the module is not initialised.
#[no_mangle]
pub extern "C" fn chat_module_list_conversations_json() -> *mut c_char {
    match module().with_state(|ms| actions::list_conversations(ms).to_string()) {
        Ok(Some(json)) => to_c_string(&json),
        _ => std::ptr::null_mut(),
    }
}

/// Return a JSON array of messages for the given conversation:
///   `[{"from_self":bool,"content":"...","timestamp_ms":N}, ...]`
///
/// Caller must free with `chat_module_free_string()`.
/// Returns `"[]"` for an unknown `convo_id` (success: no messages found),
/// NULL on error or if the module is not initialised.
#[no_mangle]
pub extern "C" fn chat_module_get_messages_json(convo_id: *const c_char) -> *mut c_char {
    let convo_id = match unsafe_str(convo_id) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };

    match module().with_state(|ms| actions::get_messages(ms, convo_id).to_string()) {
        Ok(Some(json)) => to_c_string(&json),
        _ => std::ptr::null_mut(),
    }
}

/// Encrypt `content` and dispatch to the peer. Also records the message in
/// the local history.
///
/// Returns `CHAT_MODULE_OK` on success, `CHAT_MODULE_ERR_*` on failure.
#[no_mangle]
pub extern "C" fn chat_module_send_message(convo_id: *const c_char, content: *const c_char) -> i32 {
    let convo_id = match unsafe_str(convo_id) {
        Some(s) => s,
        None => return CHAT_MODULE_ERR_BAD_ARG,
    };
    let content = match unsafe_str(content) {
        Some(s) => s,
        None => return CHAT_MODULE_ERR_BAD_ARG,
    };

    let outcome = module().with_state_mut(|ms| actions::send_message(ms, convo_id, content));
    match outcome {
        Err(_) => CHAT_MODULE_ERR_INTERNAL,
        Ok(None) => CHAT_MODULE_ERR_NOT_INIT,
        Ok(Some(Ok(()))) => CHAT_MODULE_OK,
        Ok(Some(Err(e))) => {
            eprintln!("chat_module_send_message: {e}");
            e.to_code()
        }
    }
}

/// Set (or clear) a human-readable nickname for a conversation.
/// Persisted in `history.json`. An empty `nickname` clears the override.
///
/// Returns `CHAT_MODULE_OK` on success, `CHAT_MODULE_ERR_NOT_INIT`,
/// `CHAT_MODULE_ERR_BAD_ARG`, `CHAT_MODULE_ERR_NOT_FOUND` if `convo_id`
/// is unknown, or `CHAT_MODULE_ERR_INTERNAL` if the lock is poisoned.
#[no_mangle]
pub extern "C" fn chat_module_set_conversation_nickname(
    convo_id: *const c_char,
    nickname: *const c_char,
) -> i32 {
    let convo_id = match unsafe_str(convo_id) {
        Some(s) => s,
        None => return CHAT_MODULE_ERR_BAD_ARG,
    };
    let nickname = match unsafe_str(nickname) {
        Some(s) => s,
        None => return CHAT_MODULE_ERR_BAD_ARG,
    };

    let outcome =
        module().with_state_mut(|ms| actions::set_conversation_nickname(ms, convo_id, nickname));
    match outcome {
        Err(_) => CHAT_MODULE_ERR_INTERNAL,
        Ok(None) => CHAT_MODULE_ERR_NOT_INIT,
        Ok(Some(Ok(()))) => CHAT_MODULE_OK,
        Ok(Some(Err(e))) => {
            eprintln!("chat_module_set_conversation_nickname: {e}");
            e.to_code()
        }
    }
}

/// Remove a conversation from the local history (libchat's crypto state
/// is intentionally retained, but inbound payloads for the deleted convo
/// are dropped going forward — see the `conversationDeleted` event).
///
/// Returns `CHAT_MODULE_OK` on success, `CHAT_MODULE_ERR_NOT_INIT`,
/// `CHAT_MODULE_ERR_BAD_ARG`, `CHAT_MODULE_ERR_NOT_FOUND` if `convo_id`
/// is unknown, or `CHAT_MODULE_ERR_INTERNAL` if the lock is poisoned.
#[no_mangle]
pub extern "C" fn chat_module_delete_conversation(convo_id: *const c_char) -> i32 {
    let convo_id = match unsafe_str(convo_id) {
        Some(s) => s,
        None => return CHAT_MODULE_ERR_BAD_ARG,
    };

    let outcome = module().with_state_mut(|ms| actions::delete_conversation(ms, convo_id));
    match outcome {
        Err(_) => CHAT_MODULE_ERR_INTERNAL,
        Ok(None) => CHAT_MODULE_ERR_NOT_INIT,
        Ok(Some(Ok(()))) => CHAT_MODULE_OK,
        Ok(Some(Err(e))) => {
            eprintln!("chat_module_delete_conversation: {e}");
            e.to_code()
        }
    }
}

// ── Event polling ─────────────────────────────────────────────────────────────

/// Return all pending module events as a JSON array and clear the queue.
/// Each element is an object with at least a `"type"` field:
///
///   `{"type":"messageReceived","convo_id":"...","from_self":false,
///     "content":"...","timestamp_ms":N,"is_new_convo":bool}`
///
///   `{"type":"messageSent","convo_id":"...","content":"...",
///     "timestamp_ms":N}` — emitted on a successful
///     `chat_module_send_message`. Mirrors `messageReceived` for outbound.
///
///   `{"type":"conversationCreated","convo_id":"...","is_outgoing":bool,
///     "peer_label":"..."}`
///
///   `{"type":"conversationUpdated","convo_id":"..."}`
///
///   `{"type":"conversationDeleted","convo_id":"..."}` — local deletion;
///     consumers should drop the convo from their UI. libchat retains
///     crypto state, so a peer can still encrypt and send to this
///     convo — those payloads are silently dropped by the inbound
///     worker.
///
///   `{"type":"deliveryStateChanged","state":"initialising|online|error|stopped",
///     "detail":"..."}`
///
///   `{"type":"eventsDropped","count":N}` — synthetic head-of-batch event
///     emitted when the internal queue overflowed since the last drain.
///     The consumer should re-call `chat_module_list_conversations_json`
///     and `chat_module_get_messages_json` for each conversation it tracks
///     to rebuild any state it was maintaining incrementally from earlier
///     events.
///
/// Returns `"[]"` when the queue is empty (success). Returns NULL only if
/// the module is not initialised or the internal lock is poisoned.
///
/// Caller must free with `chat_module_free_string()`.
#[no_mangle]
pub extern "C" fn chat_module_drain_events_json() -> *mut c_char {
    match module().with_state_mut(|ms| ms.event_queue.drain_to_json()) {
        Ok(Some(json)) => to_c_string(&json),
        _ => std::ptr::null_mut(),
    }
}

// ── Status ────────────────────────────────────────────────────────────────────

/// Return a JSON object with runtime status:
///   `{"identity":"...","convo_count":N,
///     "delivery_state":"initialising|online|error|stopped",
///     "detail":"..."}`
///
/// `"initialising"` is the bootstrap state between a successful
/// `chat_module_init` and the first `connectionStateChanged` event;
/// `"stopped"` only appears for the not-initialised synthetic case.
///
/// For the not-initialised case, returns the synthetic
/// `{"identity":null,"convo_count":0,"delivery_state":"stopped","detail":""}`
/// — a legitimate diagnostic state, not an error.
///
/// Returns NULL only if the module's internal lock is poisoned (should never
/// happen in practice).
///
/// Caller must free with `chat_module_free_string()`.
#[no_mangle]
pub extern "C" fn chat_module_status_json() -> *mut c_char {
    match module().with_state_optional(|ms| actions::status(ms).to_string()) {
        Ok(json) => to_c_string(&json),
        Err(_) => std::ptr::null_mut(),
    }
}

// ── Memory ────────────────────────────────────────────────────────────────────

/// Free a heap-allocated string returned by any function above.
///
/// The parameter is declared `const char*` so the Logos c-ffi codegen can
/// pass `QString::constData()` without a const-cast warning. The function
/// still takes ownership of the underlying heap allocation; do not call it
/// with a non-heap pointer. Safe to call with NULL.
#[no_mangle]
pub extern "C" fn chat_module_free_string(ptr: *const c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(ptr as *mut c_char));
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn unsafe_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

fn to_c_string(s: &str) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => {
            // Lossy fallback; use `to_c_string_strict` where NULL must
            // signal an error instead of substituting "".
            eprintln!("chat_module: to_c_string dropping interior NUL bytes from output");
            CString::new("").unwrap().into_raw()
        }
    }
}

/// Variant of `to_c_string` that returns `None` on interior NUL instead
/// of silently substituting `""`.
fn to_c_string_strict(s: &str) -> Option<*mut c_char> {
    CString::new(s).ok().map(CString::into_raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_str_handles_null() {
        assert_eq!(unsafe_str(std::ptr::null()), None);
    }

    #[test]
    fn to_c_string_round_trips() {
        let p = to_c_string("hello");
        assert!(!p.is_null());
        let recovered = unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned();
        assert_eq!(recovered, "hello");
        chat_module_free_string(p);
    }

    #[test]
    fn free_null_is_safe() {
        // Must not panic / segv.
        chat_module_free_string(std::ptr::null());
    }

    #[test]
    fn to_c_string_strict_rejects_interior_nul() {
        assert!(to_c_string_strict("bundle\0bad").is_none());
        let p = to_c_string_strict("ok").expect("clean input should round-trip");
        assert!(!p.is_null());
        chat_module_free_string(p);
    }
}
