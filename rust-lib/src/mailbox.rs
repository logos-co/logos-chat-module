//! HTTP mailbox transport: an alternative to the delivery_module path that
//! reaches peers through a centralized relay (see `tools/mailbox-relay`).
//! Selected at init by a non-empty `transport_url`; the delivery_module path is
//! left untouched when it's empty.
//!
//! `publish` enqueues each outbound envelope onto a channel the sender thread
//! drains and POSTs (with retry); `subscribe` registers a topic the poller
//! long-polls; `inbound` hands the client the same `Receiver<Vec<u8>>` the poller
//! feeds. So everything above the transport (MLS, InboxV2, GroupV2) behaves
//! identically to the delivery_module path — only the wire underneath changes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use crossbeam_channel::{Receiver, Sender};
use logos_chat::{AddressedEnvelope, DeliveryService, Transport};
use serde::Deserialize;

use crate::delivery::content_topic_for;
use crate::module::{with_display_mut, DeliveryStateKind};

/// How long the poller asks the relay to hold a request open. A message already
/// past a cursor comes back immediately (the relay wakes the long-poll on
/// publish), so this only bounds idle behaviour: reconnect cadence, how long a
/// freshly-subscribed topic waits before it's first polled, and shutdown latency
/// (join waits for the in-flight poll to return). Kept short so all three stay
/// snappy for interactive use.
const POLL_WAIT_SECS: u64 = 5;
/// Client-side request timeout; must exceed `POLL_WAIT_SECS` so a legitimately
/// held-open poll isn't cut short.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const BACKOFF_MIN: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(8);
/// Wall-clock budget for retrying one outbound envelope: once this much time has
/// elapsed since the first attempt, drop it, log it, and move on — so a dead
/// relay can't wedge the sender thread (and with it, shutdown) forever. Measured
/// end to end, so it counts blocked-POST time (up to `HTTP_TIMEOUT` each), not
/// just backoff; a relay that accepts-but-hangs is given up on after roughly one
/// timed-out POST. Still far more durable than the delivery_module path's
/// fire-and-forget, which drops on the first failure.
const SEND_MAX_ELAPSED: Duration = Duration::from_secs(15);

/// An outbound envelope queued for the sender thread: recipient topic + bytes.
#[derive(Debug)]
struct Outbound {
    topic: String,
    data: Vec<u8>,
}

/// The mailbox transport handle held by the client. Mirrors [`LogosDelivery`]'s
/// shape: an inbound receiver taken once by the client, plus the senders feeding
/// the two worker threads.
#[derive(Debug)]
pub(crate) struct MailboxDelivery {
    inbound_rx: Option<Receiver<Vec<u8>>>,
    subscribe_tx: Sender<String>,
    outbound_tx: Sender<Outbound>,
}

impl MailboxDelivery {
    fn new(
        inbound_rx: Receiver<Vec<u8>>,
        subscribe_tx: Sender<String>,
        outbound_tx: Sender<Outbound>,
    ) -> Self {
        Self {
            inbound_rx: Some(inbound_rx),
            subscribe_tx,
            outbound_tx,
        }
    }
}

impl DeliveryService for MailboxDelivery {
    type Error = String;

    fn publish(&mut self, envelope: AddressedEnvelope) -> Result<(), String> {
        // Fire-and-forget onto the outbound queue: the sender thread does the
        // blocking POST with retry, so the dispatch thread never waits on the network.
        let topic = content_topic_for(&envelope.delivery_address);
        self.outbound_tx
            .send(Outbound {
                topic,
                data: envelope.data,
            })
            .map_err(|e| e.to_string())
    }

    fn subscribe(&mut self, delivery_address: &str) -> Result<(), String> {
        self.subscribe_tx
            .send(content_topic_for(delivery_address))
            .map_err(|e| e.to_string())
    }
}

impl Transport for MailboxDelivery {
    fn inbound(&mut self) -> Receiver<Vec<u8>> {
        self.inbound_rx
            .take()
            .expect("MailboxDelivery::inbound called more than once")
    }
}

/// The relay handle and channel ends needed to spawn the mailbox workers, kept
/// separate from the transport so `initialize` can defer [`spawn`](Self::spawn)
/// until after the fallible client build succeeds — a build failure then drops
/// this without ever starting a thread.
pub(crate) struct MailboxWorkers {
    relay: Relay,
    subscribe_rx: Receiver<String>,
    inbound_tx: Sender<Vec<u8>>,
    outbound_rx: Receiver<Outbound>,
}

/// Build the relay client from `transport_url` and wire the channels, returning
/// the transport and its not-yet-spawned workers. Fails fast on an unparseable
/// URL or an HTTP client that can't be built.
pub(crate) fn prepare(transport_url: &str) -> Result<(MailboxDelivery, MailboxWorkers), String> {
    let relay = Relay::build(transport_url)?;

    let (inbound_tx, inbound_rx) = crossbeam_channel::unbounded();
    let (subscribe_tx, subscribe_rx) = crossbeam_channel::unbounded();
    let (outbound_tx, outbound_rx) = crossbeam_channel::unbounded();

    let transport = MailboxDelivery::new(inbound_rx, subscribe_tx, outbound_tx);
    let workers = MailboxWorkers {
        relay,
        subscribe_rx,
        inbound_tx,
        outbound_rx,
    };
    Ok((transport, workers))
}

impl MailboxWorkers {
    /// Spawn the poller and sender, returning the shared stop flag and both
    /// handles. Called once, after the client build has succeeded.
    pub(crate) fn spawn(self) -> (Arc<AtomicBool>, JoinHandle<()>, JoinHandle<()>) {
        let stop = Arc::new(AtomicBool::new(false));
        let poll_thread = {
            let (stop, relay) = (stop.clone(), self.relay.clone());
            let (subscribe_rx, inbound_tx) = (self.subscribe_rx, self.inbound_tx);
            thread::Builder::new()
                .name("rust-chat-mailbox-poll".into())
                .spawn(move || run_poller(stop, relay, subscribe_rx, inbound_tx))
                .expect("failed to spawn mailbox poller thread")
        };
        let send_thread = {
            let stop = stop.clone();
            let (relay, outbound_rx) = (self.relay, self.outbound_rx);
            thread::Builder::new()
                .name("rust-chat-mailbox-send".into())
                .spawn(move || run_sender(stop, relay, outbound_rx))
                .expect("failed to spawn mailbox sender thread")
        };
        (stop, poll_thread, send_thread)
    }
}

/// A configured relay endpoint: the blocking HTTP client (reqwest + rustls, the
/// same stack `HttpRegistry` uses), the two endpoint URLs, and the Basic-auth
/// credentials lifted out of the transport URL's userinfo.
#[derive(Clone)]
struct Relay {
    http: reqwest::blocking::Client,
    messages_url: String,
    poll_url: String,
    auth: Option<(String, String)>,
}

impl Relay {
    fn build(transport_url: &str) -> Result<Relay, String> {
        // Do NOT interpolate the raw transport_url here — it may carry the relay's
        // auth token in userinfo, and this error propagates to logs and the UI.
        // url::ParseError's Display is a static per-variant description that does
        // not echo the input.
        let mut url = reqwest::Url::parse(transport_url)
            .map_err(|e| format!("invalid transport_url: {e}"))?;
        let user = url.username().to_string();
        let pass = url.password().map(str::to_string);
        let auth = if user.is_empty() && pass.is_none() {
            None
        } else {
            Some((user, pass.unwrap_or_default()))
        };
        // Strip the userinfo so it isn't repeated in the request-line URL.
        let _ = url.set_username("");
        let _ = url.set_password(None);
        let base = url.as_str().trim_end_matches('/').to_string();

        let http = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| format!("http client build failed: {e}"))?;

        Ok(Relay {
            http,
            messages_url: format!("{base}/v1/messages"),
            poll_url: format!("{base}/v1/poll"),
            auth,
        })
    }

    fn authed(&self, rb: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        match &self.auth {
            Some((user, pass)) => rb.basic_auth(user, Some(pass)),
            None => rb,
        }
    }

    fn publish(&self, topic: &str, data: &[u8]) -> Result<(), String> {
        let body = serde_json::json!({ "data": URL_SAFE_NO_PAD.encode(data) });
        let resp = self
            .authed(
                self.http
                    .post(&self.messages_url)
                    .query(&[("topic", topic)])
                    .json(&body),
            )
            .send()
            .map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("relay publish returned HTTP {}", resp.status()))
        }
    }

    fn poll(&self, cursors: &HashMap<String, i64>) -> Result<Vec<PollMessage>, String> {
        let body = serde_json::json!({ "topics": cursors, "wait_secs": POLL_WAIT_SECS });
        let resp = self
            .authed(self.http.post(&self.poll_url).json(&body))
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("relay poll returned HTTP {}", resp.status()));
        }
        resp.json::<PollResponse>()
            .map(|r| r.messages)
            .map_err(|e| e.to_string())
    }
}

#[derive(Deserialize)]
struct PollResponse {
    messages: Vec<PollMessage>,
}

#[derive(Deserialize)]
struct PollMessage {
    topic: String,
    seq: i64,
    /// base64url of the envelope bytes.
    data: String,
}

/// Long-poll the relay for every subscribed topic and feed decoded payloads to
/// the client's inbound channel. Advances a per-topic cursor so no message is
/// missed or redelivered. Reflects relay reachability in `delivery_state`.
fn run_poller(
    stop: Arc<AtomicBool>,
    relay: Relay,
    subscribe_rx: Receiver<String>,
    inbound_tx: Sender<Vec<u8>>,
) {
    let mut cursors: HashMap<String, i64> = HashMap::new();
    let mut backoff = BACKOFF_MIN;
    while !stop.load(Ordering::Relaxed) {
        while let Ok(topic) = subscribe_rx.try_recv() {
            cursors.entry(topic).or_insert(0);
        }
        if cursors.is_empty() {
            // No inbound addresses subscribed yet; nap briefly and re-check.
            sleep_interruptible(&stop, BACKOFF_MIN);
            continue;
        }
        match relay.poll(&cursors) {
            Ok(messages) => {
                backoff = BACKOFF_MIN;
                set_delivery_state(DeliveryStateKind::Online, "");
                for msg in messages {
                    match URL_SAFE_NO_PAD.decode(msg.data.as_bytes()) {
                        Ok(bytes) => {
                            // The receiver is the client's worker; a failed send means
                            // the client is being dropped and we're about to stop.
                            let _ = inbound_tx.send(bytes);
                        }
                        Err(e) => {
                            eprintln!("chat_module mailbox: undecodable payload from relay: {e}")
                        }
                    }
                    let cursor = cursors.entry(msg.topic).or_insert(0);
                    *cursor = (*cursor).max(msg.seq);
                }
            }
            Err(e) => {
                if !stop.load(Ordering::Relaxed) {
                    eprintln!("chat_module mailbox: poll failed: {e}");
                    set_delivery_state(DeliveryStateKind::Error, &e);
                }
                sleep_interruptible(&stop, backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// Drain queued outbound envelopes and POST each to the relay, retrying with
/// backoff. Ends when the transport (holding `outbound_tx`) is dropped at
/// shutdown, or breaks a stuck retry when `stop` is set.
fn run_sender(stop: Arc<AtomicBool>, relay: Relay, outbound_rx: Receiver<Outbound>) {
    for item in outbound_rx {
        let start = Instant::now();
        let mut backoff = BACKOFF_MIN;
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            match relay.publish(&item.topic, &item.data) {
                Ok(()) => break,
                Err(e) => {
                    eprintln!("chat_module mailbox: publish to {} failed: {e}", item.topic);
                    if start.elapsed() >= SEND_MAX_ELAPSED {
                        eprintln!(
                            "chat_module mailbox: giving up on a message to {} after {}s",
                            item.topic,
                            start.elapsed().as_secs()
                        );
                        break;
                    }
                    sleep_interruptible(&stop, backoff);
                    backoff = (backoff * 2).min(BACKOFF_MAX);
                }
            }
        }
    }
}

/// Sleep up to `dur`, waking early if `stop` is set, so a backoff never delays
/// shutdown by more than one 100ms step.
fn sleep_interruptible(stop: &AtomicBool, dur: Duration) {
    let step = Duration::from_millis(100);
    let mut slept = Duration::ZERO;
    while slept < dur && !stop.load(Ordering::Relaxed) {
        let nap = step.min(dur - slept);
        thread::sleep(nap);
        slept += nap;
    }
}

/// Reflect relay reachability in `delivery_state`. `actions::set_delivery_state`
/// no-ops unless the state or detail actually changed, so this both refreshes the
/// error reason when it changes and stays quiet on a steady state.
fn set_delivery_state(state: DeliveryStateKind, detail: &str) {
    with_display_mut(|d| crate::actions::set_delivery_state(d, state, detail));
}
