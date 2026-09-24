//! On-disk persistence for chat module state (`chat.db`).
//!
//! ## File
//!
//! Lives at `chat.db` inside the instance persistence path the host assigns
//! (`RustModuleContext::instance_persistence_path`): a SQLCipher database in
//! WAL mode. It holds the module's own tables, versioned by [`MIGRATIONS`]
//! through `PRAGMA user_version`, and the `message_store_*` tables of the
//! `message_store` crate, which keep the messages.
//!
//! ## Writes
//!
//! Each mutation writes its rows before it returns, so shutdown has nothing
//! left to write. A message goes in one transaction with its chat's row when it
//! opens or names the chat, and a deleted chat's row, messages and tombstone
//! change together.
//!
//! ## Privacy
//!
//! The key is derived from the instance persistence path, so the file is
//! obfuscated, not protected.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use message_store::{Direction, Message, MessageStore};
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{params, Connection, OptionalExtension};
use rusqlite_migration::{Migrations, M};

/// The module's own tables. A change to them is a new migration appended here.
const MIGRATION_ARRAY: &[M] = &[M::up(include_str!("migrations/001_app_state.sql"))];
const MIGRATIONS: Migrations = Migrations::from_slice(MIGRATION_ARRAY);

/// How many of a chat's newest messages [`load_state`] reads back.
const LOADED_PER_CHAT: usize = 500;

/// A single rendered message in a conversation's local history view.
#[derive(Debug, Clone)]
pub(crate) struct DisplayMessage {
    /// `true` if this installation produced the message; `false` for inbound.
    pub from_self: bool,
    /// UTF-8 message body as the user typed (or as decrypted).
    pub content: String,
    /// Milliseconds since the Unix epoch when the message was recorded
    /// locally — not authoritative across peers.
    pub timestamp_ms: u64,
    /// Sender's account address; `None` on messages this installation sent.
    pub sender: Option<String>,
}

/// Whether a conversation is pairwise or a group; drives the members-panel
/// affordance in the UI. Stored as the contract's `"direct"`/`"group"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ConversationKind {
    #[default]
    Direct,
    Group,
}

impl ConversationKind {
    /// The contract wire string (`"direct"` / `"group"`).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ConversationKind::Direct => "direct",
            ConversationKind::Group => "group",
        }
    }
}

impl ToSql for ConversationKind {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.as_str()))
    }
}

impl FromSql for ConversationKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value.as_str()? {
            "direct" => Ok(ConversationKind::Direct),
            "group" => Ok(ConversationKind::Group),
            other => Err(FromSqlError::Other(
                format!("unknown conversation kind {other:?}").into(),
            )),
        }
    }
}

/// Per-conversation state held alongside libchat's cryptographic state.
#[derive(Debug, Clone)]
pub(crate) struct ChatSession {
    /// libchat conversation ID.
    pub chat_id: String,
    /// User-set display label. `None` falls back to `module::short_label`.
    pub nickname: Option<String>,
    /// Pairwise vs group.
    pub kind: ConversationKind,
    /// Group's shared name, `None` for a direct conversation or unnamed group.
    pub name: Option<String>,
    /// Group's shared description, `None` when unset.
    pub description: Option<String>,
    /// A direct conversation's other side: the address it was opened with, or
    /// the account of its first inbound message. `None` for a group.
    pub peer: Option<String>,
    /// Append-only render log of messages exchanged in this conversation. Kept
    /// in the message store, which fills it at init.
    pub messages: Vec<DisplayMessage>,
    /// Messages the message store holds for this conversation before
    /// `messages`, which init did not load.
    pub older_messages: usize,
    /// From a previous session, so the client holds no conversation for it and
    /// it is kept for its history only.
    pub history_only: bool,
}

/// Everything the module persists, as read back at init.
#[derive(Debug, Default)]
pub(crate) struct AppState {
    pub chats: HashMap<String, ChatSession>,
    /// User-overridden installation name. `None` falls back to
    /// `ChatClient::installation_name()`. Superseded once Accounts land.
    pub installation_name: Option<String>,
    /// Locally-deleted convo IDs. Inbound messages for these are
    /// dropped — libchat retains the crypto state regardless.
    pub deleted: HashSet<String>,
}

/// Open `chat.db`, keyed with `key`, with every table at its latest version.
pub(crate) fn open(path: &Path, key: &str) -> rusqlite_migration::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "key", key)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    MIGRATIONS.to_latest(&mut conn)?;
    MessageStore::migrate(&mut conn)?;
    Ok(conn)
}

/// The persisted state, each chat with its newest [`LOADED_PER_CHAT`] messages.
pub(crate) fn load_state(conn: &Connection) -> rusqlite::Result<AppState> {
    let store = MessageStore::new(conn);
    let mut chats = HashMap::new();
    let mut stmt =
        conn.prepare("SELECT chat_id, kind, nickname, name, description, peer FROM chats")?;
    let rows = stmt.query_map([], |row| {
        Ok(ChatSession {
            chat_id: row.get(0)?,
            kind: row.get(1)?,
            nickname: row.get(2)?,
            name: row.get(3)?,
            description: row.get(4)?,
            peer: row.get(5)?,
            messages: Vec::new(),
            older_messages: 0,
            history_only: false,
        })
    })?;
    for session in rows {
        let mut session = session?;
        session.messages = store
            .messages(&session.chat_id, None, LOADED_PER_CHAT)?
            .into_iter()
            .map(|stored| display_message(stored.message))
            .collect();
        session.older_messages = store.count(&session.chat_id)? - session.messages.len();
        chats.insert(session.chat_id.clone(), session);
    }

    let deleted = conn
        .prepare("SELECT chat_id FROM deleted_chats")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<HashSet<String>>>()?;
    let installation_name = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'installation_name'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    Ok(AppState {
        chats,
        installation_name,
        deleted,
    })
}

/// Write a chat's row, inserting it or replacing what it held.
pub(crate) fn save_chat(conn: &Connection, session: &ChatSession) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO chats (chat_id, kind, nickname, name, description, peer)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (chat_id) DO UPDATE SET
             kind = excluded.kind,
             nickname = excluded.nickname,
             name = excluded.name,
             description = excluded.description,
             peer = excluded.peer",
        params![
            session.chat_id,
            session.kind,
            session.nickname,
            session.name,
            session.description,
            session.peer,
        ],
    )?;
    Ok(())
}

/// Record a message, and write its chat's row when `session` is given, in one
/// transaction.
pub(crate) fn record_message(
    conn: &mut Connection,
    session: Option<&ChatSession>,
    message: &Message,
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    if let Some(session) = session {
        save_chat(&tx, session)?;
    }
    MessageStore::new(&tx).record(message)?;
    tx.commit()
}

/// Delete a chat and its messages, and keep its tombstone, in one transaction.
pub(crate) fn delete_chat(conn: &mut Connection, chat_id: &str) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    MessageStore::new(&tx).delete_chat(chat_id)?;
    tx.execute("DELETE FROM chats WHERE chat_id = ?1", [chat_id])?;
    tx.execute(
        "INSERT OR IGNORE INTO deleted_chats (chat_id) VALUES (?1)",
        [chat_id],
    )?;
    tx.commit()
}

/// Write the installation-name override, or clear it with `None`.
pub(crate) fn save_installation_name(
    conn: &Connection,
    name: Option<&str>,
) -> rusqlite::Result<()> {
    match name {
        Some(name) => conn.execute(
            "INSERT INTO settings (key, value) VALUES ('installation_name', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [name],
        ),
        None => conn.execute("DELETE FROM settings WHERE key = 'installation_name'", []),
    }?;
    Ok(())
}

/// A stored message as `get_messages` shows it, its sender named by account.
pub(crate) fn display_message(message: Message) -> DisplayMessage {
    DisplayMessage {
        from_self: message.direction == Direction::Sent,
        content: String::from_utf8_lossy(&message.content).into_owned(),
        timestamp_ms: message.timestamp_ms as u64,
        sender: message.sender_account,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "rust-chat-test";

    fn chat(chat_id: &str, kind: ConversationKind) -> ChatSession {
        ChatSession {
            chat_id: chat_id.into(),
            nickname: None,
            kind,
            name: None,
            description: None,
            peer: None,
            messages: Vec::new(),
            older_messages: 0,
            history_only: false,
        }
    }

    fn message(chat_id: &str, content: &str) -> Message {
        Message {
            chat_id: chat_id.into(),
            convo_id: chat_id.into(),
            direction: Direction::Received,
            sender_account: Some("raya-account".into()),
            sender_installation: Some("raya-device".into()),
            message_id: None,
            timestamp_ms: 42,
            content: content.as_bytes().to_vec(),
        }
    }

    #[test]
    fn migrations_are_valid() {
        MIGRATIONS.validate().unwrap();
    }

    #[test]
    fn chat_db_is_encrypted_under_its_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        drop(open(&path, KEY).unwrap());

        let header = std::fs::read(&path).unwrap();
        assert!(!header.starts_with(b"SQLite format 3"));
        assert!(open(&path, "rust-chat-other").is_err());
        assert!(open(&path, KEY).is_ok());
    }

    #[test]
    fn state_reads_back_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");
        {
            let mut conn = open(&path, KEY).unwrap();
            let group = ChatSession {
                nickname: Some("club".into()),
                name: Some("Book Club".into()),
                description: Some("Weekly reads".into()),
                ..chat("group", ConversationKind::Group)
            };
            save_chat(&conn, &group).unwrap();
            let direct = ChatSession {
                peer: Some("raya-account".into()),
                ..chat("direct", ConversationKind::Direct)
            };
            record_message(&mut conn, Some(&direct), &message("direct", "hi saro")).unwrap();
            save_installation_name(&conn, Some("saro-laptop")).unwrap();
        }

        let state = load_state(&open(&path, KEY).unwrap()).unwrap();

        assert_eq!(state.installation_name.as_deref(), Some("saro-laptop"));
        let group = &state.chats["group"];
        assert_eq!(group.kind, ConversationKind::Group);
        assert_eq!(group.nickname.as_deref(), Some("club"));
        assert_eq!(group.name.as_deref(), Some("Book Club"));
        assert_eq!(group.description.as_deref(), Some("Weekly reads"));
        assert!(group.messages.is_empty());
        let direct = &state.chats["direct"];
        assert_eq!(direct.kind, ConversationKind::Direct);
        assert_eq!(direct.peer.as_deref(), Some("raya-account"));
        let read_back: Vec<_> = direct
            .messages
            .iter()
            .map(|m| (m.from_self, m.content.as_str(), m.sender.as_deref()))
            .collect();
        assert_eq!(read_back, [(false, "hi saro", Some("raya-account"))]);
    }

    #[test]
    fn a_long_chat_loads_its_newest_page_and_counts_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("chat.db"), KEY).unwrap();
        let tx = conn.transaction().unwrap();
        save_chat(&tx, &chat("direct", ConversationKind::Direct)).unwrap();
        for i in 0..=LOADED_PER_CHAT {
            MessageStore::new(&tx)
                .record(&message("direct", &format!("m{i}")))
                .unwrap();
        }
        tx.commit().unwrap();

        let state = load_state(&conn).unwrap();

        let direct = &state.chats["direct"];
        assert_eq!(direct.messages.len(), LOADED_PER_CHAT);
        assert_eq!(direct.messages[0].content, "m1");
        assert_eq!(direct.older_messages, 1);
    }

    #[test]
    fn a_deleted_chat_leaves_only_its_tombstone() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = open(&dir.path().join("chat.db"), KEY).unwrap();
        let direct = chat("direct", ConversationKind::Direct);
        record_message(&mut conn, Some(&direct), &message("direct", "hi saro")).unwrap();

        delete_chat(&mut conn, "direct").unwrap();

        let state = load_state(&conn).unwrap();
        assert!(state.chats.is_empty());
        assert!(state.deleted.contains("direct"));
        assert!(MessageStore::new(&conn)
            .messages("direct", None, 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_cleared_installation_name_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(&dir.path().join("chat.db"), KEY).unwrap();
        save_installation_name(&conn, Some("saro-laptop")).unwrap();

        save_installation_name(&conn, None).unwrap();

        assert_eq!(load_state(&conn).unwrap().installation_name, None);
    }

    #[test]
    fn conversation_kind_uses_contract_strings() {
        assert_eq!(ConversationKind::Direct.as_str(), "direct");
        assert_eq!(ConversationKind::Group.as_str(), "group");
    }
}
