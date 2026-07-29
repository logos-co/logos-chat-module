//! `tracing` subscriber installed on first init.
//!
//! libchat logs exclusively through `tracing`, and a `tracing` event emitted in
//! a process with no subscriber installed is dropped. Install one writing to
//! stderr, where the module's own diagnostics already go, so the chat stack's
//! own account of a failure is available next to them.

use std::fmt;
use std::sync::Once;

use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;
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
        let selected = filter.to_string();
        // Field formatting still consults the ANSI setting, and the output lands
        // in pipes and log files where escapes are noise. `try_init` leaves an
        // already-installed subscriber alone.
        let installed = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .event_format(HostLine)
            .try_init();
        // Said in the module's own voice rather than through `tracing`, because
        // what it reports is whether `tracing` reaches anything at all. The
        // failure is silent otherwise: the chat core's events go on being raised
        // and dropped, and a reader has no way to tell that from a quiet stack.
        // No `Debug:`/`Trace:` prefix on the success line: the host classifies a
        // module's stderr by exactly those keywords and its own logger sits at
        // info, so a line that names itself debug is dropped before it reaches
        // the log. An unprefixed line lands at info.
        match installed {
            Ok(()) => eprintln!("chat_module: logging {selected}"),
            Err(error) => eprintln!(
                "Warning: chat_module: another subscriber is already installed, \
                 the chat core's events will not be logged: {error}"
            ),
        }
    });
}

/// `<SEVERITY>: <target>: <message>`, one line per event.
///
/// Carries no timestamp: every line reaches a reader through a host that stamps
/// what it re-emits, and a second time on the same line is noise.
struct HostLine;

impl<S, N> FormatEvent<S, N> for HostLine
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        write!(writer, "{}: {}: ", severity(meta.level()), meta.target())?;
        ctx.field_format().format_fields(writer.by_ref(), event)?;
        writeln!(writer)
    }
}

/// The severity word a host ranks the line by.
///
/// Hosts classify a module's stderr by a leading severity token, and the token
/// for a warning is `WARNING`, which is not what `tracing` calls that level.
fn severity(level: &Level) -> &'static str {
    if *level == Level::WARN {
        "WARNING"
    } else {
        level.as_str()
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing_core::Dispatch;
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    /// Collects everything a subscriber writes, so a test can read the line back.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("sink poisoned").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn line_for(emit: impl FnOnce()) -> String {
        let sink = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(sink.clone())
            .with_ansi(false)
            .event_format(HostLine)
            .finish();
        tracing_core::dispatcher::with_default(&Dispatch::new(subscriber), emit);

        let written = sink.0.lock().expect("sink poisoned").clone();
        String::from_utf8(written).expect("the format writes text")
    }

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

    /// A host reads severity off a token, and only `WARNING` names that level;
    /// `tracing`'s own spelling would be ranked as an ordinary line.
    #[test]
    fn every_level_names_itself_the_way_a_host_reads_it() {
        assert_eq!(severity(&Level::ERROR), "ERROR");
        assert_eq!(severity(&Level::WARN), "WARNING");
        assert_eq!(severity(&Level::INFO), "INFO");
        assert_eq!(severity(&Level::DEBUG), "DEBUG");
        assert_eq!(severity(&Level::TRACE), "TRACE");
    }

    /// The line is what a reader downstream parses, so its shape is a contract:
    /// severity, then the emitting target, then the message and its fields.
    #[test]
    fn a_line_leads_with_its_severity_then_its_target() {
        let line = line_for(|| tracing::warn!(convo = %"3f2a", "wakeup failed"));

        assert_eq!(
            line,
            "WARNING: chat_module::logging::tests: wakeup failed convo=3f2a\n"
        );
    }
}
