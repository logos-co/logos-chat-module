-- The seed of this module's account key. One row, written on the first start
-- and never replaced.
CREATE TABLE account (
    id   INTEGER PRIMARY KEY CHECK (id = 0),
    seed BLOB NOT NULL CHECK (length(seed) = 32)
);
