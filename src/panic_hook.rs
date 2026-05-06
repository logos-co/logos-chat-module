//! Panic hook installed on first init.
//!
//! `panic = "abort"` (Cargo.toml) is mandatory because `safer-ffi` (transitive
//! via libchat) requires it — and unwinding across the C ABI would be UB
//! anyway. The downside is the process aborts before `catch_unwind` can
//! return control, so wrapping every FFI entry in `catch_unwind` is dead
//! code. What *does* run before abort is the panic hook: install one that
//! prints location + payload to stderr so libchat crashes inside the
//! `logos_host_qt` subprocess are locatable instead of an opaque SIGABRT.

use std::panic;
use std::sync::Once;

static INSTALL: Once = Once::new();

pub(crate) fn install_once() {
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let payload = info.payload();
            let msg = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            match info.location() {
                Some(loc) => eprintln!(
                    "chat_module: panic at {}:{}:{}: {msg}",
                    loc.file(),
                    loc.line(),
                    loc.column()
                ),
                None => eprintln!("chat_module: panic at <unknown location>: {msg}"),
            }
            previous(info);
        }));
    });
}
