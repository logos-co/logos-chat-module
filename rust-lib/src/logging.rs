//! `tracing` subscriber installed on first init.
//!
//! libchat logs exclusively through `tracing`, and a `tracing` event emitted in
//! a process with no subscriber installed is dropped. Install one writing to
//! stderr, where the module's own diagnostics already go, so the chat stack's
//! own account of a failure is available next to them.

use std::sync::Once;

use tracing_subscriber::EnvFilter;

static INSTALL: Once = Once::new();

/// Routes `tracing` events to stderr at the level `RUST_LOG` selects, `warn`
/// when it is unset or unparseable.
pub(crate) fn install_once() {
    INSTALL.call_once(|| {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
        // The output lands in pipes and rotating log files, where ANSI escapes
        // are noise. `try_init` leaves an already-installed subscriber alone.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init();
    });
}
