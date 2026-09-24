CREATE TABLE chats (
    chat_id     TEXT PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (kind IN ('direct', 'group')),
    nickname    TEXT,
    name        TEXT,
    description TEXT,
    peer        TEXT
);

-- Chats deleted here: whatever arrives for them afterwards is dropped.
CREATE TABLE deleted_chats (
    chat_id TEXT PRIMARY KEY
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
