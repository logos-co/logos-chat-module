//! On-disk persistence for chat module state (`history.json`).
//!
//! ## File
//!
//! Lives at `<instance_path>/history.json`, where `instance_path` is the
//! same value passed to `init`. Format is the pretty-printed
//! JSON serialisation of [`AppState`].
//!
//! ## Write triggers
//!
//! [`save_state`] is invoked after every mutation that should survive a
//! restart:
//! - C ABI calls that mutate state (create_conversation, send_message,
//!   set_installation_name, set_conversation_nickname, delete_conversation).
//! - Inbound `messageReceived` handling that lands a decrypted message in
//!   the history.
//! - The final write at `chat_module_shutdown`.
//!
//! ## Atomicity
//!
//! Writes go to `history.json.tmp` first, then POSIX `rename` over the
//! target. `rename` is atomic on the same filesystem, so a crash mid-write
//! leaves the prior `history.json` intact rather than truncated.
//!
//! ## Recovery
//!
//! [`load_state`] returns `AppState::default()` on a missing, unreadable,
//! or unparseable file and logs to stderr. An unparseable file is renamed
//! to `history.json.bad.<ts>` so the next save doesn't overwrite it.
//!
//! ## Privacy
//!
//! `history.json` is plaintext on disk. Conversation messages, nicknames,
//! and the installation-name override are all stored unencrypted. Only the
//! libchat identity material in `identity.db` (sibling file) is protected
//! by sqlcipher. Any future change to what's persisted here should weigh
//! that caveat.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A single rendered message in a conversation's local history view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DisplayMessage {
    /// `true` if this installation produced the message; `false` for inbound.
    pub from_self: bool,
    /// UTF-8 message body as the user typed (or as decrypted).
    pub content: String,
    /// Milliseconds since the Unix epoch when the message was recorded
    /// locally — not authoritative across peers.
    pub timestamp_ms: u64,
}

/// Per-conversation state held alongside libchat's cryptographic state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ChatSession {
    /// libchat conversation ID (`ConversationIdOwned` rendered to string).
    pub chat_id: String,
    /// User-set display label. `None` falls back to `module::short_label`.
    pub nickname: Option<String>,
    /// Append-only render log of messages exchanged in this conversation.
    pub messages: Vec<DisplayMessage>,
}

/// Top-level persisted document. `#[serde(default)]` keeps additive
/// schema changes safe — missing fields default rather than failing
/// the parse.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppState {
    pub chats: HashMap<String, ChatSession>,
    /// User-overridden installation name. `None` falls back to
    /// `ChatClient::installation_name()`. Superseded once Accounts land.
    pub installation_name: Option<String>,
    /// Locally-deleted convo IDs. Inbound messages for these are
    /// dropped — libchat retains the crypto state regardless.
    pub deleted: HashSet<String>,
}

pub(crate) fn load_state(path: &Path) -> AppState {
    if !path.exists() {
        return AppState::default();
    }

    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "chat_module: load_state: cannot read {}: {e}; starting with default state",
                path.display()
            );
            return AppState::default();
        }
    };

    match serde_json::from_str::<AppState>(&contents) {
        Ok(s) => s,
        Err(e) => {
            // Move the unparseable file aside so the next save doesn't
            // overwrite it.
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let bad = path.with_extension(format!("json.bad.{suffix}"));
            if let Err(rename_err) = fs::rename(path, &bad) {
                eprintln!(
                    "chat_module: load_state: parse failure on {}: {e}; \
                     ALSO failed to move corrupt file aside ({rename_err}); \
                     starting with default state — next save will overwrite",
                    path.display()
                );
            } else {
                eprintln!(
                    "chat_module: load_state: parse failure on {}: {e}; \
                     corrupt file moved to {}; starting with default state",
                    path.display(),
                    bad.display()
                );
            }
            AppState::default()
        }
    }
}

/// Persist `state` to `path`. Returns the underlying I/O (or serialisation)
/// error so callers can surface a failed write rather than silently report
/// success — a mutation whose `save_state` fails would otherwise vanish on the
/// next `load_state`.
pub(crate) fn save_state(state: &AppState, path: &Path) -> io::Result<()> {
    let json = serde_json::to_string_pretty(state).map_err(io::Error::other)?;
    // tmp-file + rename gives POSIX-atomic replacement on the same
    // filesystem; a crash mid-write leaves the prior file intact.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).and_then(|_| fs::rename(&tmp, path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fresh_tmp(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "rust-chat-module-test-{}-{}-{}",
            name,
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn load_state_returns_default_when_missing() {
        let dir = fresh_tmp("missing");
        let p = dir.join("history.json");
        let s = load_state(&p);
        assert!(s.chats.is_empty());
        assert_eq!(s.installation_name, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_state_tolerates_missing_fields() {
        let dir = fresh_tmp("missing_field");
        let p = dir.join("history.json");
        let raw = r#"{"chats":{"abc":{"chat_id":"abc","nickname":null,"messages":[]}}}"#;
        fs::write(&p, raw).unwrap();
        let s = load_state(&p);
        assert_eq!(s.chats.len(), 1);
        assert!(s.chats.contains_key("abc"));
        assert_eq!(s.installation_name, None);
        assert!(s.deleted.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_state_moves_corrupt_file_aside() {
        let dir = fresh_tmp("corrupt");
        let p = dir.join("history.json");
        fs::write(&p, "not valid json {{{").unwrap();
        let s = load_state(&p);
        assert!(s.chats.is_empty());
        // Original file is gone — it was renamed, not silently deleted.
        assert!(!p.exists());
        let renamed = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().contains(".bad."))
            .expect("expected a .bad.<ts> file");
        let preserved = fs::read_to_string(renamed.path()).unwrap();
        assert!(preserved.contains("not valid json"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_state_reports_write_failure() {
        // Parent directory does not exist, so the tmp write fails — save_state
        // must surface it, not swallow it.
        let p = std::env::temp_dir()
            .join("rust-chat-module-test-no-such-dir-xyz")
            .join("history.json");
        let _ = fs::remove_dir_all(p.parent().unwrap());
        assert!(save_state(&AppState::default(), &p).is_err());
    }

    #[test]
    fn deleted_set_survives_round_trip() {
        let dir = fresh_tmp("deleted");
        let p = dir.join("history.json");
        let mut s = AppState::default();
        s.deleted.insert("abc".into());
        s.deleted.insert("xyz".into());
        save_state(&s, &p).unwrap();
        let loaded = load_state(&p);
        assert!(loaded.deleted.contains("abc"));
        assert!(loaded.deleted.contains("xyz"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_state_round_trips() {
        let dir = fresh_tmp("rt");
        let p = dir.join("history.json");
        let mut s = AppState {
            installation_name: Some("alice".into()),
            ..AppState::default()
        };
        s.chats.insert(
            "convo".into(),
            ChatSession {
                chat_id: "convo".into(),
                nickname: Some("bob".into()),
                messages: vec![DisplayMessage {
                    from_self: true,
                    content: "hi".into(),
                    timestamp_ms: 42,
                }],
            },
        );
        save_state(&s, &p).unwrap();

        let loaded = load_state(&p);
        assert_eq!(loaded.installation_name.as_deref(), Some("alice"));
        assert_eq!(loaded.chats.len(), 1);
        let convo = &loaded.chats["convo"];
        assert_eq!(convo.nickname.as_deref(), Some("bob"));
        assert_eq!(convo.messages.len(), 1);
        assert_eq!(convo.messages[0].content, "hi");
        let _ = fs::remove_dir_all(&dir);
    }
}
