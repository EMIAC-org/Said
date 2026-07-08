-- Migration 013 — k-event promotion + per-language vocab buckets.
--
-- Two coupled changes that together cut the most damaging false-positive
-- learnings (single-event promotion + cross-language leak):
--
-- 1. `pending_promotions` — stages every learnable STT_ERROR / POLISH_ERROR
--    seen by the classifier.  We require ≥ 2 confirming sightings (with
--    matching phonetic keys) before promoting to the live `vocabulary` /
--    `stt_replacements` tables.  This is what WisperFlow's troubleshooting
--    docs implicitly admit they're missing — they tell users to manually
--    trim the dictionary because single-event promotion bloats it.
--
-- 2. `vocabulary.language` and `stt_replacements.language` columns —
--    nullable so existing rows are treated as language-agnostic until the
--    next time they're upserted.  Going forward, every promotion records
--    the user's `output_language` at the time of the edit so we can filter
--    the keyterms slate by language at recording time (no more Devanagari
--    leaking into English-mode speech hints).

-- ── Pending promotions queue ─────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS pending_promotions (
    user_id          TEXT NOT NULL REFERENCES local_user(id) ON DELETE CASCADE,
    correct_form     TEXT NOT NULL,           -- the proposed correct spelling
    transcript_form  TEXT NOT NULL,           -- what STT actually wrote (lowercased)
    phonetic_key     TEXT NOT NULL,           -- phonetic key of correct_form
    output_language  TEXT NOT NULL,           -- language at time of sighting
    sighting_count   INTEGER NOT NULL DEFAULT 1,
    first_seen       INTEGER NOT NULL,
    last_seen        INTEGER NOT NULL,
    UNIQUE(user_id, correct_form, output_language)
);
CREATE INDEX IF NOT EXISTS idx_pending_promo_user
    ON pending_promotions (user_id, last_seen DESC);
CREATE INDEX IF NOT EXISTS idx_pending_promo_phon
    ON pending_promotions (user_id, phonetic_key);

-- ── Per-language buckets ─────────────────────────────────────────────────
ALTER TABLE vocabulary       ADD COLUMN language TEXT;
ALTER TABLE stt_replacements ADD COLUMN language TEXT;
CREATE INDEX IF NOT EXISTS idx_vocab_user_lang
    ON vocabulary (user_id, language, weight DESC);
CREATE INDEX IF NOT EXISTS idx_stt_repl_user_lang
    ON stt_replacements (user_id, language);
