//! `tracing` subscriber installed on first init.
//!
//! libchat logs exclusively through `tracing`, and a `tracing` event emitted in
//! a process with no subscriber installed is dropped. Install one writing to
//! two places: stderr, where the module's own diagnostics already go, so the
//! chat stack's own account of a failure is available next to them; and a file
//! in the instance directory the host assigned, which is what a consumer can
//! hand over after the run is over.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once, OnceLock};

use chrono::Local;
use tracing_core::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields, MakeWriter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

static INSTALL: Once = Once::new();

/// The file [`install_once`] opened, for [`log_path`] to report. Empty until
/// then, and empty for good when there was nowhere to write.
static LOG_PATH: Mutex<String> = Mutex::new(String::new());

/// The same file the subscriber writes, for [`write_line`] to reach without it.
static RUN_LOG: OnceLock<RunLog> = OnceLock::new();

/// The level the chat core's own targets log at when the client named none.
const DEFAULT_LEVEL: &str = "info";

/// The name every log in an instance directory follows: `<stem>_<stamp>.log` is
/// the file being written, `<stem>_<stamp>.NNN.log` a rotation of it. A consumer
/// groups a directory's files into runs by reading this shape, and reads the
/// stem off the announced path, so a second writer keeping its own log in the
/// same directory is grouped separately without either knowing about the other.
const STEM: &str = "chat_module";
const STAMP_FORMAT: &str = "%Y%m%d_%H%M%S";
/// The time a file line leads with. ISO-8601 to the millisecond, which is what
/// reading this log against another writer's takes.
const LINE_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f";
/// Length of a [`STAMP_FORMAT`] stamp: eight date digits, a separator, six time
/// digits.
const STAMP_LEN: usize = 15;
/// Lines before the file being written is moved aside and a fresh one opened.
const ROTATE_AFTER_LINES: u64 = 10_000;
/// Runs kept in the instance directory. That directory is persistence, so
/// nothing else sweeps it and the module prunes its own.
const KEEP_RUNS: usize = 10;

/// Everything at `warn`, and the chat core's own targets at `level`.
///
/// A flat level is the wrong shape here: the dependency graph carries crates far
/// chattier than the chat core (de-mls alone has an order of magnitude more
/// `info` sites than libchat, most of them per-consensus-round), and they would
/// bury the handful of lifecycle events this exists to surface.
///
/// `chat_module` is one of the targets because most of a run's story is this
/// module's own: libchat and the generic client together raise eleven events,
/// nearly all on failure paths, so a healthy run through them alone says
/// nothing.
fn filter_for(level: &str) -> String {
    format!("warn,chat_module={level},libchat={level},logos_generic_chat={level}")
}

/// The level a client may ask the chat core to log at. Anything else — absent,
/// empty, misspelled — is [`DEFAULT_LEVEL`].
fn level_or_default(requested: &str) -> &str {
    match requested {
        "error" | "warn" | "info" | "debug" | "trace" => requested,
        _ => DEFAULT_LEVEL,
    }
}

/// `RUST_LOG` outranks the client's choice, and neither an empty nor an
/// unparseable value is a choice: `EnvFilter` reads `""` as "enable nothing",
/// which would silence the stack a client asked to hear.
fn filter_from(env: Option<&str>, level: &str) -> EnvFilter {
    env.filter(|value| !value.trim().is_empty())
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(|| EnvFilter::new(filter_for(level)))
}

/// The file this module is writing, empty when none was opened.
pub(crate) fn log_path() -> String {
    LOG_PATH.lock().expect("log path poisoned").clone()
}

/// Appends one already-formatted line to this run's log, bypassing `tracing`.
///
/// For what cannot go through the subscriber: the panic hook runs while the
/// process is on its way to `abort`, and the thread that panicked may be the one
/// holding the file. A busy file is skipped rather than waited on, because a
/// deadlock here costs the whole record and one line is what is at stake.
pub(crate) fn write_line(line: &str) {
    if let Some(log) = RUN_LOG.get() {
        log.write_stamped(line);
    }
}

/// Routes `tracing` events to stderr and to this run's log file, at the level
/// `RUST_LOG` selects, else the one the client asked for, else
/// [`DEFAULT_LEVEL`].
///
/// The level is frozen here: `Once` plus `try_init` mean a second `init` changes
/// nothing, which matches `init`'s own documented behaviour. Letting a client
/// raise the level while running needs a `tracing_subscriber::reload` layer over
/// the filter, and nothing asks for that yet.
pub(crate) fn install_once(level: &str) {
    INSTALL.call_once(|| {
        let filter = filter_from(
            std::env::var("RUST_LOG").ok().as_deref(),
            level_or_default(level),
        );
        let selected = filter.to_string();

        let (run_log, opened) = match RunLog::open() {
            Ok(log) => {
                let path = log.path();
                (log, path)
            }
            Err(reason) => {
                eprintln!(
                    "Warning: chat_module: no log file ({reason}); the chat \
                     core's events reach stderr only"
                );
                (RunLog::inert(), String::new())
            }
        };
        *LOG_PATH.lock().expect("log path poisoned") = opened.clone();
        let _ = RUN_LOG.set(run_log.clone());
        let destination = if opened.is_empty() {
            "stderr".to_string()
        } else {
            format!("stderr and {opened}")
        };

        // Two destinations rather than one writer taking both, because the lines
        // differ: what goes to stderr is re-emitted by a host that stamps it,
        // and what goes to the file is stamped here or not at all.
        //
        // Field formatting still consults the ANSI setting, and the output lands
        // in pipes and log files where escapes are noise. `try_init` leaves an
        // already-installed subscriber alone.
        let to_stderr = tracing_subscriber::fmt::layer()
            .with_writer(io::stderr)
            .with_ansi(false)
            .event_format(HostLine);
        let to_file = tracing_subscriber::fmt::layer()
            .with_writer(run_log)
            .with_ansi(false)
            .event_format(StampedLine);
        let installed = tracing_subscriber::registry()
            .with(filter)
            .with(to_stderr)
            .with(to_file)
            .try_init();
        // Said in the module's own voice rather than through `tracing`, because
        // what it reports is whether `tracing` reaches anything at all. The
        // failure is silent otherwise: the chat core's events go on being raised
        // and dropped, and a reader has no way to tell that from a quiet stack.
        // No `Debug:`/`Trace:` prefix on the success line: the host classifies a
        // module's stderr by exactly those keywords and its own logger sits at
        // info, so a line that names itself debug is dropped before it reaches
        // the log. An unprefixed line lands at info. That classifier reads the
        // stderr half only — the file is written directly and passes nothing,
        // which is what makes `debug` and `trace` worth asking for.
        match installed {
            Ok(()) => eprintln!("chat_module: logging {selected} to {destination}"),
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

/// [`HostLine`] with the time in front, for the file.
///
/// The file is written directly and nothing else stamps it, and a stamp is what
/// reading this log against another writer's in the same directory takes. The
/// stderr half stays unstamped: a host stamps what it re-emits, and two times on
/// one line is noise.
struct StampedLine;

impl<S, N> FormatEvent<S, N> for StampedLine
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
        write!(writer, "{} ", Local::now().format(LINE_TIME_FORMAT))?;
        HostLine.format_event(ctx, writer, event)
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

// ── The run's log file ───────────────────────────────────────────────────────

/// The file this run writes its log to, shared with the subscriber, which writes
/// from whichever thread raised the event. Holds nothing when there was nowhere
/// to write, so the subscriber takes one writer either way.
#[derive(Clone)]
struct RunLog(Arc<Mutex<Option<OpenLog>>>);

/// The file a run is writing, and what rotating it takes.
struct OpenLog {
    file: File,
    /// The announced path, and always the file being written: a rotation moves
    /// the full one aside and reopens under this name.
    path: PathBuf,
    stamp: String,
    lines: u64,
    rotations: u32,
}

impl RunLog {
    /// Opens this run's file in the instance directory the host assigned, and
    /// prunes all but the newest [`KEEP_RUNS`] runs.
    fn open() -> Result<Self, String> {
        // A host that never set a persistence base path still stamps a context,
        // with the path left empty, so emptiness is the "host not configured"
        // signal — the reading `actions::initialize` makes of the same field.
        let directory = crate::context()
            .map(|ctx| ctx.instance_persistence_path)
            .filter(|path| !path.is_empty())
            .ok_or("the host assigned no instance directory")?;
        let directory = PathBuf::from(directory);
        // This runs ahead of `actions::initialize`, which is what otherwise
        // creates the directory, so on a first run it is not there yet.
        fs::create_dir_all(&directory)
            .map_err(|e| format!("cannot create {}: {e}", directory.display()))?;

        let stamp = Local::now().format(STAMP_FORMAT).to_string();
        let path = directory.join(format!("{STEM}_{stamp}.log"));
        // Appending: two runs starting in the same second share a stamp, and the
        // earlier one's lines are worth more than a tidy file.
        let file = File::options()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;

        // After opening, so the run about to be written is one of the ones kept.
        prune_runs(&directory);

        Ok(Self(Arc::new(Mutex::new(Some(OpenLog {
            file,
            path,
            stamp,
            lines: 0,
            rotations: 0,
        })))))
    }

    /// A log with nowhere to write: every line reaches stderr and stops there.
    fn inert() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    fn path(&self) -> String {
        self.0
            .lock()
            .expect("run log poisoned")
            .as_ref()
            .map(|open| open.path.display().to_string())
            .unwrap_or_default()
    }

    /// Appends one already-formatted line, stamped like every other. Skips a
    /// busy file instead of waiting on it, which is what makes this safe to call
    /// from a thread that may itself be the one holding it.
    fn write_stamped(&self, line: &str) {
        let Ok(mut open) = self.0.try_lock() else {
            return;
        };
        if let Some(open) = open.as_mut() {
            let stamped = format!("{} {line}\n", Local::now().format(LINE_TIME_FORMAT));
            let _ = open.append(stamped.as_bytes());
        }
    }
}

impl OpenLog {
    /// Appends one event's bytes, rotating once the file is full. The count is of
    /// the lines this process wrote, so a run that reopens an existing stamp
    /// carries on from whatever was already in the file.
    fn append(&mut self, buf: &[u8]) -> io::Result<()> {
        self.file.write_all(buf)?;
        self.lines += buf.iter().filter(|byte| **byte == b'\n').count() as u64;
        if self.lines >= ROTATE_AFTER_LINES {
            self.rotate()?;
        }
        Ok(())
    }

    /// Moves the full file aside under the next rotation ordinal and opens a
    /// fresh one back under the announced name.
    fn rotate(&mut self) -> io::Result<()> {
        // Spend the budget before trying: a rotation that cannot happen has to
        // be retried when the file is next full, not on every line after this
        // one.
        self.lines = 0;
        self.rotations += 1;
        let aside = self
            .path
            .with_file_name(format!("{STEM}_{}.{:03}.log", self.stamp, self.rotations));
        fs::rename(&self.path, aside)?;
        self.file = File::create(&self.path)?;
        Ok(())
    }
}

impl io::Write for RunLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.0.lock().expect("run log poisoned").as_mut() {
            Some(open) => open.append(buf).map(|()| buf.len()),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.0.lock().expect("run log poisoned").as_mut() {
            Some(open) => open.file.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for RunLog {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Deletes every run in `directory` but the newest [`KEEP_RUNS`], rotations
/// included.
fn prune_runs(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };

    let mut runs: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(stamp) = name.to_str().and_then(stamp_of) else {
            continue;
        };
        runs.entry(stamp.to_string())
            .or_default()
            .push(entry.path());
    }

    // A stamp sorts lexicographically the way it sorts chronologically, so the
    // oldest runs are the front of the map.
    let excess = runs.len().saturating_sub(KEEP_RUNS);
    for (_, files) in runs.into_iter().take(excess) {
        for file in files {
            let _ = fs::remove_file(file);
        }
    }
}

/// The stamp a run file's name carries, or `None` when the name is not one of
/// this module's — which is what leaves another writer's log in the same
/// directory alone.
fn stamp_of(file_name: &str) -> Option<&str> {
    let rest = file_name.strip_prefix(STEM)?.strip_prefix('_')?;
    let stamp = rest.get(..STAMP_LEN)?;
    let (date, time) = stamp.split_once('_')?;
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    if date.len() != 8 || time.len() != 6 || !digits(date) || !digits(time) {
        return None;
    }
    is_run_suffix(&rest[STAMP_LEN..]).then_some(stamp)
}

/// Whether what follows a stamp names the file being written (`.log`) or one of
/// its rotations (`.NNN.log`).
fn is_run_suffix(suffix: &str) -> bool {
    if suffix == ".log" {
        return true;
    }
    let bytes = suffix.as_bytes();
    bytes.len() == ".000.log".len()
        && bytes[0] == b'.'
        && suffix.ends_with(".log")
        && bytes[1..4].iter().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tempfile::TempDir;
    use tracing_core::Dispatch;

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

    fn stamped_line_for(emit: impl FnOnce()) -> String {
        let sink = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(sink.clone())
            .with_ansi(false)
            .event_format(StampedLine)
            .finish();
        tracing_core::dispatcher::with_default(&Dispatch::new(subscriber), emit);

        let written = sink.0.lock().expect("sink poisoned").clone();
        String::from_utf8(written).expect("the format writes text")
    }

    /// A log open in `directory` under a fixed stamp, so a test can name the
    /// files it expects rather than reading the clock.
    fn log_at(directory: &Path, stamp: &str) -> RunLog {
        let path = directory.join(format!("{STEM}_{stamp}.log"));
        let file = File::options()
            .create(true)
            .append(true)
            .open(&path)
            .expect("the temporary directory is writable");
        RunLog(Arc::new(Mutex::new(Some(OpenLog {
            file,
            path,
            stamp: stamp.to_string(),
            lines: 0,
            rotations: 0,
        }))))
    }

    fn write_lines(log: &mut RunLog, count: u64) {
        for _ in 0..count {
            io::Write::write_all(log, b"INFO: libchat: something happened\n")
                .expect("the temporary directory is writable");
        }
    }

    /// `EnvFilter` reports the directives it kept in an order of its own, so a
    /// filter is compared as a set rather than as the string it was built from.
    fn directives(filter: EnvFilter) -> Vec<String> {
        let mut kept: Vec<String> = filter
            .to_string()
            .split(',')
            .map(|directive| directive.trim().to_string())
            .collect();
        kept.sort();
        kept
    }

    fn file_names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .expect("the temporary directory is readable")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// `EnvFilter::new` drops a directive it cannot parse instead of failing, so
    /// a typo in a target name would silently leave that target at the global
    /// level.
    #[test]
    fn every_default_directive_parses() {
        assert_eq!(
            directives(EnvFilter::new(filter_for(DEFAULT_LEVEL))),
            [
                "chat_module=info",
                "libchat=info",
                "logos_generic_chat=info",
                "warn"
            ]
        );
    }

    /// The whole ladder is available, and only the chat core's targets move: a
    /// client asking for `trace` must not uncap the crates around it.
    #[test]
    fn every_level_applies_to_the_chat_core_and_nothing_else() {
        for level in ["error", "warn", "info", "debug", "trace"] {
            assert_eq!(
                directives(filter_from(None, level_or_default(level))),
                [
                    format!("chat_module={level}"),
                    format!("libchat={level}"),
                    format!("logos_generic_chat={level}"),
                    "warn".to_string()
                ],
                "{level} must reach the chat core's targets alone"
            );
        }
    }

    /// A level nobody recognises is not a reason to log nothing, or everything.
    #[test]
    fn an_unusable_level_is_the_default_one() {
        for requested in ["", "verbose", "INFO", "9"] {
            assert_eq!(level_or_default(requested), DEFAULT_LEVEL);
        }
    }

    /// The environment outranks the client, because a developer with `RUST_LOG`
    /// set is overriding the app on purpose.
    #[test]
    fn rust_log_wins_over_the_clients_level() {
        assert_eq!(
            directives(filter_from(Some("libchat=trace"), "error")),
            ["libchat=trace"]
        );
    }

    /// An empty `RUST_LOG` is what an unset one looks like to a shell that
    /// exported it anyway, and `EnvFilter` reads it as "enable nothing" — which
    /// would silence the stack rather than leave it as the client asked.
    #[test]
    fn an_empty_or_unusable_rust_log_is_no_choice_at_all() {
        for env in ["", "   ", "libchat=nonsense"] {
            assert_eq!(
                directives(filter_from(Some(env), "debug")),
                directives(EnvFilter::new(filter_for("debug"))),
                "{env:?} must leave the client's level standing"
            );
        }
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

    /// The file is the only record of its own lines, so each one carries the
    /// time; what follows is the line stderr gets.
    #[test]
    fn a_file_line_leads_with_the_time() {
        let line = stamped_line_for(|| tracing::warn!("wakeup failed"));
        let (stamp, rest) = line.split_once(' ').expect("a time, then the line");

        assert!(
            chrono::NaiveDateTime::parse_from_str(stamp, LINE_TIME_FORMAT).is_ok(),
            "{stamp} does not read as a time"
        );
        assert_eq!(
            rest,
            "WARNING: chat_module::logging::tests: wakeup failed\n"
        );
    }

    /// The panic hook's route in. A crash is the failure the file exists for and
    /// the one thing that cannot arrive as a `tracing` event, so it goes to the
    /// file directly, and stamped like everything around it.
    #[test]
    fn a_line_written_outside_tracing_is_stamped_like_the_rest() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let log = log_at(directory.path(), "20260730_120000");

        log.write_stamped("CRITICAL: chat_module: panic at group_v1.rs:340:14: boom");

        let written = fs::read_to_string(directory.path().join("chat_module_20260730_120000.log"))
            .expect("the line was written");
        let (stamp, rest) = written.split_once(' ').expect("a time, then the line");
        assert!(
            chrono::NaiveDateTime::parse_from_str(stamp, LINE_TIME_FORMAT).is_ok(),
            "{stamp} does not read as a time"
        );
        assert_eq!(
            rest,
            "CRITICAL: chat_module: panic at group_v1.rs:340:14: boom\n"
        );
    }

    /// The panicking thread may be the one holding the file. Waiting for it
    /// would trade the one line for the whole record, so the line is what gives.
    #[test]
    fn a_busy_file_costs_the_line_and_not_the_process() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let log = log_at(directory.path(), "20260730_120000");
        let held = log.0.lock().expect("run log poisoned");

        log.write_stamped("CRITICAL: chat_module: panic while holding the file");

        drop(held);
        assert_eq!(
            fs::read_to_string(directory.path().join("chat_module_20260730_120000.log"))
                .expect("the file is readable"),
            ""
        );
    }

    /// A log with nowhere to write is still a writer, so the subscriber needs no
    /// second shape for the case where the host assigned no directory.
    #[test]
    fn a_log_with_nowhere_to_write_swallows_its_lines() {
        let mut inert = RunLog::inert();

        write_lines(&mut inert, 3);

        assert!(inert.path().is_empty());
    }

    /// The announced path is what a consumer lists a directory from, so it has to
    /// keep naming the file being written once the first one fills.
    #[test]
    fn a_full_file_is_moved_aside_and_the_announced_path_reopened() {
        let directory = TempDir::new().expect("a temporary directory");
        let mut log = log_at(directory.path(), "20260730_142811");
        let announced = log.path();

        write_lines(&mut log, ROTATE_AFTER_LINES);

        assert_eq!(log.path(), announced, "the announced path does not move");
        assert_eq!(
            file_names(directory.path()),
            [
                "chat_module_20260730_142811.001.log",
                "chat_module_20260730_142811.log"
            ]
        );
        assert_eq!(
            fs::read_to_string(&announced)
                .expect("the reopened file is readable")
                .len(),
            0,
            "the reopened file starts empty"
        );
    }

    /// Rotations of one run are that run, so pruning counts stamps and takes a
    /// run's whole set of files with it.
    #[test]
    fn pruning_keeps_ten_runs_and_counts_a_runs_rotations_as_one() {
        let directory = TempDir::new().expect("a temporary directory");
        for day in 1..=12 {
            let stamp = format!("202607{day:02}_120000");
            for name in [
                format!("{STEM}_{stamp}.log"),
                format!("{STEM}_{stamp}.001.log"),
            ] {
                File::create(directory.path().join(name)).expect("writable");
            }
        }
        // Another writer's log in the same directory, and a file that only looks
        // like a run.
        File::create(directory.path().join("chat_ui_20260701_120000.log")).expect("writable");
        File::create(directory.path().join("chat_module_notastamp.log")).expect("writable");

        prune_runs(directory.path());

        let kept = file_names(directory.path());
        assert_eq!(kept.len(), 10 * 2 + 2, "ten runs, and neither stranger");
        assert!(
            kept.contains(&"chat_ui_20260701_120000.log".to_string()),
            "another writer's log is not this module's to delete"
        );
        assert!(
            kept.contains(&"chat_module_notastamp.log".to_string()),
            "a file carrying no stamp belongs to no run"
        );

        // `file_names` sorts, so a run's files are adjacent and the oldest kept
        // stamp comes first.
        let mut stamps: Vec<&str> = kept.iter().filter_map(|name| stamp_of(name)).collect();
        stamps.dedup();
        assert_eq!(stamps.len(), KEEP_RUNS);
        assert_eq!(
            stamps.first().copied(),
            Some("20260703_120000"),
            "the two oldest runs go, both of their files each"
        );
    }

    /// Grouping a directory into runs reads the stem, so what a name has to say
    /// to be one of ours is worth pinning.
    #[test]
    fn only_this_modules_run_files_carry_a_stamp() {
        assert_eq!(
            stamp_of("chat_module_20260730_142811.log"),
            Some("20260730_142811")
        );
        assert_eq!(
            stamp_of("chat_module_20260730_142811.007.log"),
            Some("20260730_142811")
        );
        for stranger in [
            "chat_ui_20260730_142811.log",
            "chat_module_20260730_142811.log.bak",
            "chat_module_2026073_142811.log",
            "chat_module_20260730_14281x.log",
            "chat_module_20260730_142811.7.log",
            "chat_module.log",
        ] {
            assert_eq!(stamp_of(stranger), None, "{stranger} is not one of ours");
        }
    }
}
