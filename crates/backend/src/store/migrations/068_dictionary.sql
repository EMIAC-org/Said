-- One word list replaces the learning tables.
--
-- An entry says: where the transcript has `heard`, write `written`. With no
-- `heard` it is a spelling to keep. Polish receives the entries that occur in
-- a transcript; nothing else reads them.
--
-- Carried over: every vocabulary term, and the misheard forms the user
-- confirmed on a review card. The synthetic spelling variants that were
-- generated after each confirm are left behind.
--
-- The old tables stay in place, unused, so an older AirNote build opening
-- this database still starts; a later migration can drop them.
CREATE TABLE IF NOT EXISTS dictionary (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id    TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    written    TEXT NOT NULL,
    heard      TEXT,
    source     TEXT NOT NULL,  -- 'learned' | 'added'
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_dictionary_entry
    ON dictionary (user_id, lower(written), lower(coalesce(heard, '')));
