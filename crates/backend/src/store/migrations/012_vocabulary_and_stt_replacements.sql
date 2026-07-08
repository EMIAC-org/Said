-- Migration 012: Layered learning architecture
--
-- Replaces the single-table `word_corrections` model with three stores that
-- map onto where each kind of correction must actually be applied:
--
--   1. `vocabulary`        — STT-layer hint terms (local speech / polish
--                            initial_prompt). Catches STT misrecognitions of
--                            jargon, names, brands, code identifiers.
--
--   2. `stt_replacements`  — Post-STT literal+phonetic substitutions. Used when
--                            STT consistently mis-hears one term as another and
--                            biasing alone is insufficient.
--
--   3. `edit_events.edit_class` — every captured edit is now classified into
--                            one of four classes: STT_ERROR, POLISH_ERROR,
--                            USER_REPHRASE, USER_REWRITE.  Drives downstream
--                            promotion logic.
--
-- `word_corrections` keeps its rows but gains a `weight` column (REAL, supports
-- decay and demotion) and a `tier` column (provisional → biased → enforced).

-- ── Vocabulary: STT-layer bias terms ─────────────────────────────────────────
CREATE TABLE IF NOT EXISTS vocabulary (
    user_id     TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    term        TEXT NOT NULL,             -- the correctly-spelled term to bias toward
    weight      REAL NOT NULL DEFAULT 1.0, -- strength; decays over time
    use_count   INTEGER NOT NULL DEFAULT 1,
    last_used   INTEGER NOT NULL,
    source      TEXT NOT NULL DEFAULT 'auto', -- 'auto' | 'manual' | 'starred'
    UNIQUE(user_id, term)
);
CREATE INDEX IF NOT EXISTS idx_vocab_user_weight ON vocabulary (user_id, weight DESC);

-- ── STT replacements: post-STT literal + phonetic substitution ───────────────
CREATE TABLE IF NOT EXISTS stt_replacements (
    user_id        TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    transcript_form TEXT NOT NULL,          -- what STT keeps emitting (lowercased)
    correct_form   TEXT NOT NULL,           -- what user actually said
    phonetic_key   TEXT NOT NULL,           -- soundex/metaphone of transcript_form
    weight         REAL NOT NULL DEFAULT 1.0,
    use_count      INTEGER NOT NULL DEFAULT 1,
    last_used      INTEGER NOT NULL,
    UNIQUE(user_id, transcript_form, correct_form)
);
CREATE INDEX IF NOT EXISTS idx_stt_repl_user ON stt_replacements (user_id);
CREATE INDEX IF NOT EXISTS idx_stt_repl_phon ON stt_replacements (user_id, phonetic_key);

-- ── Add classification + weight to existing tables (additive only) ───────────
ALTER TABLE edit_events ADD COLUMN edit_class TEXT;
-- nullable for backfill compatibility: 'STT_ERROR' | 'POLISH_ERROR' | 'USER_REPHRASE' | 'USER_REWRITE'

ALTER TABLE word_corrections ADD COLUMN weight REAL NOT NULL DEFAULT 1.0;
ALTER TABLE word_corrections ADD COLUMN tier   TEXT NOT NULL DEFAULT 'enforced';
-- tier: 'provisional' | 'enforced' | 'pinned'
