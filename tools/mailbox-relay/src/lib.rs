//! HTTP mailbox relay: a centralized, inspectable transport for logos-chat
//! testing. Each delivery topic is an append-only log; publishers POST envelopes
//! and subscribers long-poll for anything past their per-topic cursor. Payloads
//! are opaque (MLS ciphertext); the relay only ever sees metadata.
//!
//! See README.md for the wire API and the curl debugging cookbook.

pub mod store;

use anyhow::Result;
use axum::{
    extract::{Query, Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::{Store, StoredMessage};
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// Longest a `/v1/poll` request may block before returning empty.
const MAX_WAIT_SECS: u64 = 30;
/// Re-check the store at least this often while long-polling, so a lost
/// notification costs at most this much added latency rather than a whole wait.
const POLL_RECHECK: Duration = Duration::from_secs(2);
/// Per-topic ceiling on messages returned by one poll/list.
const FETCH_LIMIT: i64 = 200;
/// Ceiling on topics in one poll request, bounding the per-request query fan-out.
const MAX_POLL_TOPICS: usize = 256;
const RETENTION_INTERVAL: Duration = Duration::from_secs(3600);

pub struct Inner {
    pub store: Store,
    /// Woken on every publish so long-pollers re-check promptly.
    pub notify: Notify,
    /// `None` disables auth (RELAY_NO_AUTH=1); otherwise the Basic-auth password.
    pub token: Option<String>,
    pub start: Instant,
}

impl Inner {
    pub fn new(store: Store, token: Option<String>) -> Self {
        Self {
            store,
            notify: Notify::new(),
            token,
            start: Instant::now(),
        }
    }
}

pub type AppState = Arc<Inner>;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub struct Config {
    pub bind: String,
    pub db: String,
    pub token: Option<String>,
    pub retention_days: i64,
}

impl Config {
    pub fn from_env() -> Result<Config> {
        let no_auth = std::env::var("RELAY_NO_AUTH")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        Ok(Config {
            bind: std::env::var("RELAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".into()),
            db: std::env::var("RELAY_DB").unwrap_or_else(|_| "./relay.db".into()),
            token: resolve_token(no_auth, std::env::var("RELAY_TOKEN").ok())?,
            retention_days: std::env::var("RELAY_RETENTION_DAYS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(7),
        })
    }
}

/// Resolve the Basic-auth token from config. An empty `RELAY_TOKEN` must fail
/// exactly like a missing one — otherwise the relay would start with auth "on"
/// but accept any Basic credential with an empty password.
fn resolve_token(no_auth: bool, raw: Option<String>) -> Result<Option<String>> {
    if no_auth {
        return Ok(None);
    }
    match raw.filter(|t| !t.is_empty()) {
        Some(t) => Ok(Some(t)),
        None => Err(anyhow::anyhow!(
            "RELAY_TOKEN must be set and non-empty (or set RELAY_NO_AUTH=1 for an open relay)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_token;

    #[test]
    fn token_resolution() {
        assert!(resolve_token(true, None).unwrap().is_none()); // no-auth: token irrelevant
        assert_eq!(
            resolve_token(false, Some("s3cret".into()))
                .unwrap()
                .as_deref(),
            Some("s3cret")
        );
        assert!(resolve_token(false, None).is_err()); // missing
        assert!(resolve_token(false, Some(String::new())).is_err()); // empty == missing
    }
}

// ---- error type -----------------------------------------------------------

struct AppError(StatusCode, String);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(e: tokio::task::JoinError) -> Self {
        AppError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("task join failed: {e}"),
        )
    }
}

// ---- wire types -----------------------------------------------------------

#[derive(Deserialize)]
struct TopicQuery {
    topic: String,
}

#[derive(Deserialize)]
struct ListQuery {
    topic: String,
    #[serde(default)]
    after: i64,
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    100
}

#[derive(Deserialize)]
struct PublishBody {
    /// base64url (no padding) of the envelope bytes.
    data: String,
}

#[derive(Deserialize)]
struct PollBody {
    /// topic -> cursor (return messages with seq strictly greater).
    topics: HashMap<String, i64>,
    #[serde(default)]
    wait_secs: Option<u64>,
}

#[derive(Serialize)]
struct MsgOut {
    topic: String,
    seq: i64,
    ts_ms: i64,
    data: String,
}

impl From<StoredMessage> for MsgOut {
    fn from(m: StoredMessage) -> Self {
        MsgOut {
            topic: m.topic,
            seq: m.seq,
            ts_ms: m.ts_ms,
            data: URL_SAFE_NO_PAD.encode(&m.data),
        }
    }
}

#[derive(Serialize)]
struct PollResponse {
    messages: Vec<MsgOut>,
}

// ---- handlers -------------------------------------------------------------

async fn publish(
    State(st): State<AppState>,
    Query(q): Query<TopicQuery>,
    Json(body): Json<PublishBody>,
) -> Result<Response, AppError> {
    if q.topic.is_empty() {
        return Err(AppError(
            StatusCode::BAD_REQUEST,
            "topic must not be empty".into(),
        ));
    }
    let data = URL_SAFE_NO_PAD.decode(body.data.as_bytes()).map_err(|e| {
        AppError(
            StatusCode::BAD_REQUEST,
            format!("data is not valid base64url: {e}"),
        )
    })?;
    let ts = now_ms();
    let st2 = st.clone();
    let topic = q.topic.clone();
    let seq = tokio::task::spawn_blocking(move || st2.store.append(&topic, &data, ts)).await??;
    st.notify.notify_waiters();
    tracing::debug!(topic = %q.topic, seq, "publish");
    Ok((
        StatusCode::CREATED,
        Json(json!({ "seq": seq, "ts_ms": ts })),
    )
        .into_response())
}

async fn poll(
    State(st): State<AppState>,
    Json(body): Json<PollBody>,
) -> Result<Json<PollResponse>, AppError> {
    if body.topics.len() > MAX_POLL_TOPICS {
        return Err(AppError(
            StatusCode::BAD_REQUEST,
            format!("too many topics in one poll (max {MAX_POLL_TOPICS})"),
        ));
    }
    let wait = Duration::from_secs(body.wait_secs.unwrap_or(25).min(MAX_WAIT_SECS));
    let deadline = Instant::now() + wait;
    loop {
        // Register interest *before* the fetch so a publish landing between the
        // fetch and the await is not lost; the periodic recheck is a backstop.
        let notified = st.notify.notified();
        let messages = fetch_topics(&st, &body.topics).await?;
        if !messages.is_empty() {
            return Ok(Json(PollResponse { messages }));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(Json(PollResponse { messages: vec![] }));
        }
        tokio::select! {
            _ = notified => {}
            _ = tokio::time::sleep(remaining.min(POLL_RECHECK)) => {}
        }
    }
}

async fn fetch_topics(
    st: &AppState,
    topics: &HashMap<String, i64>,
) -> Result<Vec<MsgOut>, AppError> {
    let st2 = st.clone();
    let topics = topics.clone();
    let rows = tokio::task::spawn_blocking(move || -> Result<Vec<StoredMessage>> {
        let mut out = Vec::new();
        for (topic, after) in &topics {
            out.extend(st2.store.fetch_after(topic, *after, FETCH_LIMIT)?);
        }
        Ok(out)
    })
    .await??;
    let mut msgs: Vec<MsgOut> = rows.into_iter().map(MsgOut::from).collect();
    msgs.sort_by_key(|m| (m.ts_ms, m.seq));
    Ok(msgs)
}

async fn list(
    State(st): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<PollResponse>, AppError> {
    // Clamp the caller-supplied limit to the same per-topic ceiling poll uses; a
    // negative SQL LIMIT means "unbounded" in sqlite, which would dump the topic.
    let limit = q.limit.clamp(1, FETCH_LIMIT);
    let st2 = st.clone();
    let rows = tokio::task::spawn_blocking(move || st2.store.fetch_after(&q.topic, q.after, limit))
        .await??;
    Ok(Json(PollResponse {
        messages: rows.into_iter().map(MsgOut::from).collect(),
    }))
}

async fn topics(State(st): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let st2 = st.clone();
    let rows = tokio::task::spawn_blocking(move || st2.store.topics()).await??;
    let out: Vec<_> = rows
        .into_iter()
        .map(|t| {
            json!({
                "topic": t.topic,
                "count": t.count,
                "last_seq": t.last_seq,
                "last_ts_ms": t.last_ts_ms,
                "bytes": t.bytes,
            })
        })
        .collect();
    Ok(Json(json!(out)))
}

async fn delete_topic(
    State(st): State<AppState>,
    Query(q): Query<TopicQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let st2 = st.clone();
    let deleted = tokio::task::spawn_blocking(move || st2.store.delete_topic(&q.topic)).await??;
    Ok(Json(json!({ "deleted": deleted })))
}

async fn healthz(State(st): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({ "ok": true, "uptime_s": st.start.elapsed().as_secs() }))
}

// ---- auth -----------------------------------------------------------------

async fn auth_mw(
    State(st): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let Some(expected) = st.token.as_deref() else {
        return Ok(next.run(req).await);
    };
    let ok = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .map(|h| check_basic(h, expected))
        .unwrap_or(false);
    if ok {
        Ok(next.run(req).await)
    } else {
        Err(AppError(StatusCode::UNAUTHORIZED, "unauthorized".into()))
    }
}

/// Validate an `Authorization: Basic ...` header, treating the password as the
/// shared token (username ignored). Plain comparison is fine for a test relay.
fn check_basic(header: &str, token: &str) -> bool {
    let Some(b64) = header.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = STANDARD.decode(b64.trim()) else {
        return false;
    };
    let Ok(creds) = String::from_utf8(decoded) else {
        return false;
    };
    matches!(creds.split_once(':'), Some((_user, pass)) if pass == token)
}

// ---- assembly -------------------------------------------------------------

pub fn build_app(state: AppState) -> Router {
    let protected = Router::new()
        .route("/v1/messages", post(publish).get(list).delete(delete_topic))
        .route("/v1/poll", post(poll))
        .route("/v1/topics", get(topics))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth_mw));

    Router::new()
        .route("/healthz", get(healthz))
        .merge(protected)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

fn spawn_retention(state: AppState, days: i64) {
    if days <= 0 {
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(RETENTION_INTERVAL);
        loop {
            ticker.tick().await;
            let cutoff = now_ms() - days * 86_400_000;
            let st = state.clone();
            match tokio::task::spawn_blocking(move || st.store.delete_before(cutoff)).await {
                Ok(Ok(n)) if n > 0 => tracing::info!(deleted = n, "retention sweep"),
                Ok(Err(e)) => tracing::warn!(error = %e, "retention sweep failed"),
                _ => {}
            }
        }
    });
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

pub async fn run_from_env() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,tower_http=info")),
        )
        .init();

    let cfg = Config::from_env()?;
    let store = Store::open(&cfg.db)?;
    let state: AppState = Arc::new(Inner::new(store, cfg.token));
    spawn_retention(state.clone(), cfg.retention_days);

    let listener = TcpListener::bind(&cfg.bind).await?;
    tracing::info!(
        bind = %cfg.bind,
        db = %cfg.db,
        auth = state.token.is_some(),
        retention_days = cfg.retention_days,
        "mailbox-relay listening"
    );
    axum::serve(listener, build_app(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}
