//! Panic hook installed on first init.
//!
//! `panic = "abort"` (Cargo.toml) is mandatory because `safer-ffi` (transitive
//! via libchat) requires it — and unwinding across the C ABI would be UB
//! anyway. The downside is the process aborts before `catch_unwind` can
//! return control, so wrapping every FFI entry in `catch_unwind` is dead
//! code. What *does* run before abort is the panic hook: install one that
//! prints location + payload to stderr so libchat crashes inside the
//! `logos_host_qt` subprocess are locatable instead of an opaque SIGABRT.
//!
//! The same goes to this run's log file, with a backtrace. A crash is the
//! failure a reader most needs the file for, and it is the one thing that cannot
//! arrive as a `tracing` event: the process is already on its way down.

use std::backtrace::Backtrace;
use std::panic;
use std::sync::Once;

use crate::logging;

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
            let origin = match info.location() {
                Some(loc) => format!("{}:{}:{}", loc.file(), loc.line(), loc.column()),
                None => "<unknown location>".to_string(),
            };
            eprintln!("chat_module: panic at {origin}: {msg}");
            // Forced, not `capture`: this is the one line that has to carry a
            // trace, and leaving it to `RUST_BACKTRACE` means the run that
            // crashed is the run that did not record why.
            logging::write_line(&format!(
                "CRITICAL: chat_module: panic at {origin}: {msg}\n{}",
                Backtrace::force_capture()
            ));
            previous(info);
        }));
    });
}
