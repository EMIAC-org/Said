-- Migration 063 - meaning-first vocabulary card FTS.
--
-- Migration 060 was already used for provider settings on the dev branch.
-- This rebuild indexes the complete vocabulary card: term, type, meaning,
-- observed aliases, and support examples. Rust backfill repopulates the
-- virtual table after this migration.

DROP TABLE IF EXISTS vocab_fts;

CREATE VIRTUAL TABLE IF NOT EXISTS vocab_fts USING fts5(
    user_id UNINDEXED,
    term UNINDEXED,
    card_text,
    tokenize = 'unicode61 remove_diacritics 2'
);
