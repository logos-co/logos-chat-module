//! SQLite-backed message log. Each topic is an independent ordered stream keyed
//! by a per-topic monotonic `seq`, so a poller fetches strictly-newer messages
//! with `seq > cursor` and can neither miss nor duplicate one.

use anyhow::Result;
use rusqlite::{params, Connection};
use std::sync::Mutex;

pub struct StoredMessage {
    pub topic: String,
    pub seq: i64,
    pub ts_ms: i64,
    pub data: Vec<u8>,
}

pub struct TopicStat {
    pub topic: String,
    pub count: i64,
    pub last_seq: i64,
    pub last_ts_ms: i64,
    pub bytes: i64,
}

/// One connection behind a mutex. Access is brief and always off the async
/// executor (handlers call these inside `spawn_blocking`), so a single
/// serialized connection is more than enough for a test relay.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS messages (
                 topic  TEXT    NOT NULL,
                 seq    INTEGER NOT NULL,
                 ts_ms  INTEGER NOT NULL,
                 data   BLOB    NOT NULL,
                 PRIMARY KEY (topic, seq)
             );",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Append to `topic`, assigning the next per-topic seq atomically. Returns it.
    pub fn append(&self, topic: &str, data: &[u8], ts_ms: i64) -> Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let seq: i64 = tx.query_row(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE topic = ?1",
            params![topic],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO messages (topic, seq, ts_ms, data) VALUES (?1, ?2, ?3, ?4)",
            params![topic, seq, ts_ms, data],
        )?;
        tx.commit()?;
        Ok(seq)
    }

    /// Messages on `topic` with `seq > after`, oldest first, capped at `limit`.
    pub fn fetch_after(&self, topic: &str, after: i64, limit: i64) -> Result<Vec<StoredMessage>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT topic, seq, ts_ms, data FROM messages
             WHERE topic = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![topic, after, limit], |r| {
            Ok(StoredMessage {
                topic: r.get(0)?,
                seq: r.get(1)?,
                ts_ms: r.get(2)?,
                data: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn topics(&self) -> Result<Vec<TopicStat>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT topic, COUNT(*), MAX(seq), MAX(ts_ms), COALESCE(SUM(LENGTH(data)), 0)
             FROM messages GROUP BY topic ORDER BY topic",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TopicStat {
                topic: r.get(0)?,
                count: r.get(1)?,
                last_seq: r.get(2)?,
                last_ts_ms: r.get(3)?,
                bytes: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_topic(&self, topic: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM messages WHERE topic = ?1", params![topic])?)
    }

    pub fn delete_before(&self, cutoff_ms: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute("DELETE FROM messages WHERE ts_ms < ?1", params![cutoff_ms])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_assigns_monotonic_seq_per_topic() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.append("a", b"1", 10).unwrap(), 1);
        assert_eq!(s.append("a", b"2", 11).unwrap(), 2);
        assert_eq!(s.append("b", b"x", 12).unwrap(), 1); // seq is per-topic
        assert_eq!(s.append("a", b"3", 13).unwrap(), 3);
    }

    #[test]
    fn fetch_after_is_exclusive_and_ordered() {
        let s = Store::open_in_memory().unwrap();
        for i in 1..=5 {
            s.append("a", format!("m{i}").as_bytes(), i).unwrap();
        }
        let got = s.fetch_after("a", 2, 100).unwrap();
        assert_eq!(got.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
        assert_eq!(got[0].data, b"m3");
    }

    #[test]
    fn fetch_after_respects_limit() {
        let s = Store::open_in_memory().unwrap();
        for i in 1..=10 {
            s.append("a", b"x", i).unwrap();
        }
        assert_eq!(s.fetch_after("a", 0, 3).unwrap().len(), 3);
    }

    #[test]
    fn topics_reports_counts_and_bytes() {
        let s = Store::open_in_memory().unwrap();
        s.append("a", b"12345", 1).unwrap();
        s.append("a", b"67", 2).unwrap();
        let t = s.topics().unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].topic, "a");
        assert_eq!(t[0].count, 2);
        assert_eq!(t[0].last_seq, 2);
        assert_eq!(t[0].bytes, 7);
    }

    #[test]
    fn delete_before_and_topic() {
        let s = Store::open_in_memory().unwrap();
        s.append("a", b"x", 100).unwrap();
        s.append("a", b"x", 200).unwrap();
        assert_eq!(s.delete_before(150).unwrap(), 1);
        assert_eq!(s.fetch_after("a", 0, 100).unwrap().len(), 1);
        assert_eq!(s.delete_topic("a").unwrap(), 1);
        assert!(s.topics().unwrap().is_empty());
    }

    #[test]
    fn persists_across_reopen() {
        let path = std::env::temp_dir().join(format!("relay-store-test-{}.db", std::process::id()));
        let p = path.to_str().unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{p}{suffix}"));
        }
        {
            let s = Store::open(p).unwrap();
            s.append("a", b"hi", 1).unwrap();
        }
        {
            let s = Store::open(p).unwrap();
            let got = s.fetch_after("a", 0, 10).unwrap();
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].data, b"hi");
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{p}{suffix}"));
        }
    }
}
