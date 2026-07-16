-- Migration 017 — Month-1 over-time-learning foundations.
--
-- Three coupled changes that together upgrade the polish-prompt vocabulary
-- selector from "single example, static weight, dense-only" to
-- "centroid-of-N-examples, time-decayed reinforcement, hybrid BM25 + dense":
--
--   1. vocab_embedding_examples — one row per *observed sighting* of a
--      vocab term, storing the embedding + the snippet it came from. We
--      keep up to N=10 examples per (user, term) as a FIFO ring; older
--      sightings are evicted. The vocab_embeddings.embedding column
--      becomes the *centroid* (normalised mean) of the live ring rather
--      than a single first-observed snapshot.
--
--      Why: single-example representations are the largest source of
--      retrieval noise — research from Snell et al. (Prototypical
--      Networks, NeurIPS 2017) shows centroid-of-N is dramatically more
--      robust at 5-50 examples per concept, exactly our scale.
--      Bonus: cluster variance becomes a free drift signal (split a term
--      when its example cloud is bimodal, e.g. "Mercury planet vs band").
--
--   2. vocab_fts — FTS5 virtual table over (term, example_context) for
--      BM25 keyword search. Vocabulary is exact-match-heavy (acronyms,
--      brand names, code identifiers) — purely dense retrieval misses
--      these. The original implementation fused dense + BM25 ranks via RRF.
--      SQLite ships FTS5; no new dependency.
--
--      Why: documented 15–30% recall improvement on exact-match-critical
--      corpora (Weaviate hybrid-search, OpenSearch RRF blogs). Cheapest
--      big win available.
--
--   3. Time-decay reinforcement is implemented entirely in code (no schema
--      change) — we already have weight/use_count/last_used columns;
--      they're just never updated. Migration is silent on this; see
--      the vocabulary retrieval code plus bump_last_used in the same PR.

-- ── 1. Per-sighting examples (FIFO ring per term) ────────────────────────────
CREATE TABLE IF NOT EXISTS vocab_embedding_examples (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    term          TEXT NOT NULL,
    embedding     BLOB NOT NULL,         -- 256 × f32 LE
    example_text  TEXT NOT NULL,         -- the snippet this sighting came from
    recorded_at   INTEGER NOT NULL       -- ms epoch
);
CREATE INDEX IF NOT EXISTS idx_vocab_examples_user_term
    ON vocab_embedding_examples (user_id, term, recorded_at DESC);

-- ── 2. FTS5 virtual table over vocabulary for BM25 ───────────────────────────
-- Contentless: we manage inserts/updates/deletes from Rust because external-
-- content FTS5 with triggers gets fragile under upsert (ON CONFLICT) flows.
CREATE VIRTUAL TABLE IF NOT EXISTS vocab_fts USING fts5(
    user_id UNINDEXED,
    term,
    example_context,
    tokenize = 'unicode61 remove_diacritics 2'
);
