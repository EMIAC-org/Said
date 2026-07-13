-- Migration 060 - meaning-first vocabulary card FTS.
--
-- Migration 058 is owned by edit-review sessions. This rebuild indexes the
-- complete vocabulary card: term, type, meaning, observed aliases, and
-- support examples. Rust backfill repopulates the virtual table after this.

DROP TABLE IF EXISTS vocab_fts;

CREATE VIRTUAL TABLE IF NOT EXISTS vocab_fts USING fts5(
    user_id UNINDEXED,
    term UNINDEXED,
    card_text,
    tokenize = 'unicode61 remove_diacritics 2'
);
