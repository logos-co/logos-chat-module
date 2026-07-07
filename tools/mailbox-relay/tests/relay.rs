//! End-to-end tests over real HTTP: spin the app on an ephemeral port and drive
//! it with reqwest. Covers the publish/poll roundtrip, long-poll wakeups, cursor
//! semantics, and auth.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use mailbox_relay::store::Store;
use mailbox_relay::{build_app, AppState, Inner};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

async fn spawn(token: Option<String>) -> String {
    let store = Store::open_in_memory().unwrap();
    let state: AppState = Arc::new(Inner::new(store, token));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, build_app(state)).await.unwrap();
    });
    format!("http://{addr}")
}

fn b64(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(s)
}

#[tokio::test]
async fn publish_then_poll_roundtrip() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();

    let r = client
        .post(format!("{base}/v1/messages?topic=t"))
        .json(&json!({ "data": b64("hello") }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201);
    assert_eq!(r.json::<Value>().await.unwrap()["seq"], 1);

    let got: Value = client
        .post(format!("{base}/v1/poll"))
        .json(&json!({ "topics": { "t": 0 }, "wait_secs": 1 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let msgs = got["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["data"], b64("hello"));
    assert_eq!(msgs[0]["seq"], 1);
}

#[tokio::test]
async fn poll_cursor_is_exclusive() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();
    for m in ["a", "b"] {
        client
            .post(format!("{base}/v1/messages?topic=t"))
            .json(&json!({ "data": b64(m) }))
            .send()
            .await
            .unwrap();
    }
    // cursor at seq 1 => only the second message.
    let got: Value = client
        .post(format!("{base}/v1/poll"))
        .json(&json!({ "topics": { "t": 1 }, "wait_secs": 1 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let msgs = got["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["data"], b64("b"));
    assert_eq!(msgs[0]["seq"], 2);
}

#[tokio::test]
async fn long_poll_wakes_on_publish() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();
    let publisher = client.clone();
    let pub_url = format!("{base}/v1/messages?topic=t");
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        publisher
            .post(pub_url)
            .json(&json!({ "data": b64("ping") }))
            .send()
            .await
            .unwrap();
    });

    let start = Instant::now();
    let got: Value = client
        .post(format!("{base}/v1/poll"))
        .json(&json!({ "topics": { "t": 0 }, "wait_secs": 20 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Woke on the publish, not after the full 20s wait.
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "long-poll did not wake promptly"
    );
    assert_eq!(got["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn poll_times_out_empty() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();
    let got: Value = client
        .post(format!("{base}/v1/poll"))
        .json(&json!({ "topics": { "t": 0 }, "wait_secs": 1 }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["messages"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn auth_is_enforced() {
    let base = spawn(Some("secret".into())).await;
    let client = reqwest::Client::new();

    let no_auth = client
        .get(format!("{base}/v1/topics"))
        .send()
        .await
        .unwrap();
    assert_eq!(no_auth.status(), 401);

    let bad = client
        .get(format!("{base}/v1/topics"))
        .basic_auth("x", Some("wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 401);

    let ok = client
        .get(format!("{base}/v1/topics"))
        .basic_auth("x", Some("secret"))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // healthz stays open even with auth configured.
    assert_eq!(
        client
            .get(format!("{base}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn topics_and_delete() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/v1/messages?topic=t"))
        .json(&json!({ "data": b64("x") }))
        .send()
        .await
        .unwrap();

    let topics: Value = client
        .get(format!("{base}/v1/topics"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(topics.as_array().unwrap().len(), 1);
    assert_eq!(topics[0]["topic"], "t");
    assert_eq!(topics[0]["count"], 1);

    let del: Value = client
        .delete(format!("{base}/v1/messages?topic=t"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(del["deleted"], 1);

    let topics: Value = client
        .get(format!("{base}/v1/topics"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(topics.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn rejects_bad_base64() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/v1/messages?topic=t"))
        .json(&json!({ "data": "not valid base64!!!" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}

#[tokio::test]
async fn list_clamps_negative_limit() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();
    for m in ["a", "b", "c"] {
        client
            .post(format!("{base}/v1/messages?topic=t"))
            .json(&json!({ "data": b64(m) }))
            .send()
            .await
            .unwrap();
    }
    // limit=-1 must NOT be treated as unbounded (sqlite semantics); it clamps to 1.
    let got: Value = client
        .get(format!("{base}/v1/messages?topic=t&limit=-1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(got["messages"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn poll_rejects_too_many_topics() {
    let base = spawn(None).await;
    let client = reqwest::Client::new();
    let topics: serde_json::Map<String, Value> =
        (0..300).map(|i| (format!("t{i}"), json!(0))).collect();
    let r = client
        .post(format!("{base}/v1/poll"))
        .json(&json!({ "topics": topics, "wait_secs": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
}
