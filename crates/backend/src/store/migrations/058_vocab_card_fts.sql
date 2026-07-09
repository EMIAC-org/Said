-- Migration 058 — meaning-first vocabulary card FTS.
--
-- The old vocab_fts indexed only (term, example_context). The new retriever
-- needs one compact card document per term containing term, type, meaning,
-- aliases, and support examples. Rust backfill repopulates the virtual table
-- after migrations finish.

DROP TABLE IF EXISTS vocab_fts;

CREATE VIRTUAL TABLE IF NOT EXISTS vocab_fts USING fts5(
    user_id UNINDEXED,
    term UNINDEXED,
    card_text,
    tokenize = 'unicode61 remove_diacritics 2'
);
