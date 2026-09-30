-- Set when a commit removed this installation from the group; the chat stays.
ALTER TABLE chats ADD COLUMN removed INTEGER NOT NULL DEFAULT 0 CHECK (removed IN (0, 1));
