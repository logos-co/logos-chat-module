//! `tracing` subscriber installed on first init.
//!
//! libchat logs exclusively through `tracing`, and a `tracing` event emitted in
//! a process with no subscriber installed is dropped. Install one writing to
//! stderr, where the module's own diagnostics already go, so the chat stack's
//! own account of a failure is available next to them.

use std::sync::Once;

use tracing_subscriber::EnvFilter;

static INSTALL: Once = Once::new();

/// Everything at `warn`, and the chat core's own targets at `info`.
///
/// A flat `info` is the wrong shape here: the dependency graph carries crates
/// far chattier than the chat core at that level (de-mls alone has an order of
/// magnitude more `info` sites than libchat, most of them per-consensus-round),
/// and they would bury the handful of lifecycle events this exists to surface.
///
/// `chat_module` is one of the targets because most of a run's story is this
/// module's own: libchat and the generic client together raise eleven events,
/// nearly all on failure paths, so a healthy run through them alone says
/// nothing.
const DEFAULT_FILTER: &str = "warn,chat_module=info,libchat=info,logos_generic_chat=info";

/// Routes `tracing` events to stderr at the level `RUST_LOG` selects,
/// [`DEFAULT_FILTER`] when it is unset or unparseable.
pub(crate) fn install_once() {
    INSTALL.call_once(|| {
        let filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
        // The output lands in pipes and rotating log files, where ANSI escapes
        // are noise. `try_init` leaves an already-installed subscriber alone.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `EnvFilter::new` drops a directive it cannot parse instead of failing, so
    /// a typo in a target name would silently leave that target at the global
    /// level.
    #[test]
    fn every_default_directive_parses() {
        let parsed = EnvFilter::new(DEFAULT_FILTER).to_string();
        let mut kept: Vec<_> = parsed.split(',').map(str::trim).collect();
        kept.sort_unstable();

        assert_eq!(
            kept,
            [
                "chat_module=info",
                "libchat=info",
                "logos_generic_chat=info",
                "warn"
            ]
        );
    }
}
