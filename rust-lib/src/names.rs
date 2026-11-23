//! Display names, read from each account's log (`profile.displayname`).
//!
//! `get_display_name` answers from [`Names`] and never waits on the registry.
//! An address it holds no fresh name for goes to the worker, which reads the
//! account's log and raises `display_name_changed` when the name differs.

use std::collections::HashMap;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use logos_account::{AccountAddr, AccountProvider, AccountResolver, ProfileRead};

use crate::module::{with_display, with_display_mut};

/// How long a name is served before the next ask reads the log again: the TTL
/// libchat's own client gives the endorsements it reads from the same logs.
const FRESH_FOR: Duration = Duration::from_secs(30 * 60);

/// The names read so far, and the worker's queue.
#[derive(Default)]
pub(crate) struct Names {
    held: HashMap<String, Held>,
    /// `None` before init, so nothing is queued.
    reads: Option<Sender<AccountAddr>>,
}

#[derive(Default)]
struct Held {
    name: String,
    /// `None` until a read succeeds.
    read_at: Option<Instant>,
    /// A read is queued or running.
    reading: bool,
}

impl Names {
    pub(crate) fn new(reads: Sender<AccountAddr>) -> Self {
        Self {
            held: HashMap::new(),
            reads: Some(reads),
        }
    }

    /// Whether init made these names, so reads are queued and recorded.
    fn is_live(&self) -> bool {
        self.reads.is_some()
    }

    /// The name held for `address`, empty when none is. Queues a read when the
    /// held one is missing or stale.
    pub(crate) fn get(&mut self, address: &str) -> String {
        let Some(reads) = &self.reads else {
            return String::new();
        };
        let Ok(addr) = address.parse::<AccountAddr>() else {
            return String::new();
        };
        let held = self.held.entry(address.to_owned()).or_default();
        let stale = held.read_at.is_none_or(|at| at.elapsed() >= FRESH_FOR);
        if stale && !held.reading && reads.send(addr).is_ok() {
            held.reading = true;
        }
        held.name.clone()
    }

    /// Records a read of `address`'s log, `None` for one that failed. `true`
    /// when the name it found differs from the one held.
    fn record(&mut self, address: &str, read: Option<&str>) -> bool {
        if !self.is_live() {
            return false;
        }
        let held = self.held.entry(address.to_owned()).or_default();
        held.reading = false;
        let Some(name) = read else {
            return false;
        };
        held.read_at = Some(Instant::now());
        if held.name == name {
            return false;
        }
        held.name = name.to_owned();
        true
    }
}

/// Starts the worker reading the logs [`Names`] queues from `provider`. It
/// stops once the queue's sender is dropped.
pub(crate) fn spawn<P>(provider: P) -> (Sender<AccountAddr>, JoinHandle<()>)
where
    P: AccountProvider + Send + 'static,
{
    let (reads_tx, reads_rx) = crossbeam_channel::unbounded();
    let handle = thread::Builder::new()
        .name("rust-chat-names".into())
        .spawn(move || run(AccountResolver::new(provider), reads_rx))
        .expect("failed to spawn names thread");
    (reads_tx, handle)
}

fn run<P: AccountProvider>(mut resolver: AccountResolver<P>, reads: Receiver<AccountAddr>) {
    for addr in reads {
        // Shutdown resets the names, and what is still queued goes unread.
        if !with_display(|d| d.names.is_live()) {
            break;
        }
        let address = addr.to_string();
        let read = match resolver.resolve(&addr) {
            Ok(record) => Some(record.and_then(|r| r.display_name()).unwrap_or_default()),
            Err(e) => {
                tracing::warn!("reading the display name of {address}: {e}");
                None
            }
        };
        if with_display_mut(|d| d.names.record(&address, read)) {
            crate::emit_display_name_changed(&address, read.unwrap_or_default());
        }
    }
}

#[cfg(test)]
mod tests {
    use logos_account::test_support::TestAccountProvider;
    use logos_account::{Account, AccountPublisher, ProfileWrite};
    use logos_generic_chat::PendingInstallation;

    use super::*;

    /// A valid address no account published under.
    fn address() -> String {
        hex::encode(PendingInstallation::generate().endorsement_request())
    }

    fn names() -> (Names, Receiver<AccountAddr>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        (Names::new(tx), rx)
    }

    #[test]
    fn nothing_is_read_before_init() {
        let saro = address();
        let mut names = Names::default();

        assert_eq!(names.get(&saro), "");
        assert!(!names.record(&saro, Some("Saro")));
        assert_eq!(names.get(&saro), "");
    }

    #[test]
    fn an_unread_address_is_read_once() {
        let saro = address();
        let (mut names, reads) = names();

        names.get(&saro);
        names.get(&saro);

        let read: Vec<_> = reads.try_iter().map(|addr| addr.to_string()).collect();
        assert_eq!(read, [saro]);
    }

    #[test]
    fn a_read_name_is_held_and_reported_once() {
        let saro = address();
        let (mut names, reads) = names();
        names.get(&saro);

        assert!(names.record(&saro, Some("Saro")));
        assert!(!names.record(&saro, Some("Saro")));
        assert_eq!(names.get(&saro), "Saro");
        assert_eq!(reads.try_iter().count(), 1);
    }

    #[test]
    fn a_failed_read_keeps_the_name() {
        let saro = address();
        let (mut names, _reads) = names();
        names.get(&saro);
        names.record(&saro, Some("Saro"));

        assert!(!names.record(&saro, None));
        assert_eq!(names.get(&saro), "Saro");
    }

    #[test]
    fn a_failed_first_read_is_tried_again() {
        let saro = address();
        let (mut names, reads) = names();
        names.get(&saro);
        names.record(&saro, None);

        names.get(&saro);

        assert_eq!(reads.try_iter().count(), 2);
    }

    #[test]
    fn the_worker_reads_the_name_an_account_set() {
        let mut account = Account::new(TestAccountProvider::default());
        let log = account.update().set_display_name("Saro").publish().unwrap();
        let mut provider = TestAccountProvider::default();
        provider.publish(&account.addr(), &log).unwrap();
        let saro = account.addr().to_string();
        let (reads, worker) = spawn(provider);
        with_display_mut(|d| d.names = Names::new(reads));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut name = crate::actions::display_name(&saro);
        while name.is_empty() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            name = crate::actions::display_name(&saro);
        }
        with_display_mut(|d| d.names = Names::default());
        worker.join().unwrap();

        assert_eq!(name, "Saro");
    }

    #[test]
    fn an_address_that_is_not_one_is_never_read() {
        let (mut names, reads) = names();

        assert_eq!(names.get("saro"), "");
        assert_eq!(reads.try_iter().count(), 0);
    }
}
