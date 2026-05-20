use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use said_core::deepgram::{BiasPackage, ReplacementRule, resolve_stt_mode};

use crate::{
    llm::phonetics,
    store::{
        DbPool,
        stt_replacements::{self, ExportTier, ReviewStatus, SttReplacement},
        vocabulary,
    },
};

fn is_high_signal_term_type(term_type: Option<&str>) -> bool {
    matches!(
        term_type,
        Some("acronym" | "proper_noun" | "brand" | "code_identifier" | "phrase")
    )
}

fn term_is_commonish(term: &vocabulary::VocabTerm) -> bool {
    let raw = term.term.trim();
    if raw.is_empty() || raw.contains(char::is_whitespace) {
        return false;
    }
    let alpha_only = raw.chars().all(|c| c.is_ascii_alphabetic());
    let lower_only = raw.chars().all(|c| !c.is_ascii_uppercase());
    alpha_only
        && lower_only
        && !is_high_signal_term_type(term.term_type.as_deref())
        && phonetics::jargon_score(raw) < 0.35
}

fn is_precise_keyterm(term: &vocabulary::VocabTerm) -> bool {
    matches!(term.source.as_str(), "manual" | "starred")
        || (is_high_signal_term_type(term.term_type.as_deref()) && !term_is_commonish(term))
        || (term.use_count > 1 && phonetics::jargon_score(&term.term) >= 0.4)
        || (term.weight >= 1.5 && phonetics::jargon_score(&term.term) >= 0.45)
}

fn canonical_keyterm_score(term: &vocabulary::VocabTerm) -> f64 {
    let mut score = 0.0;
    match term.source.as_str() {
        "starred" => score += 4.0,
        "manual" => score += 3.0,
        _ => {}
    }
    if is_high_signal_term_type(term.term_type.as_deref()) {
        score += 2.0;
    }
    if !term_is_commonish(term) {
        score += phonetics::jargon_score(&term.term) * 2.0;
    }
    score += term.weight.min(5.0) * 0.5;
    score += (term.use_count.min(8) as f64) * 0.35;
    score += (term.last_used.max(0) as f64) / 1_000_000_000_000.0;
    score
}

fn alias_is_commonish(alias: &str) -> bool {
    let trimmed = alias.trim();
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return false;
    }
    let alpha_only = trimmed.chars().all(|c| c.is_ascii_alphabetic());
    let lower_only = trimmed.chars().all(|c| !c.is_ascii_uppercase());
    alpha_only && lower_only && phonetics::jargon_score(trimmed) < 0.35 && trimmed.len() <= 10
}

fn replacement_threshold(term: &vocabulary::VocabTerm, alias: &str) -> Option<i64> {
    if alias_is_commonish(alias) {
        let alias_len = alias.trim().chars().count();
        let canonical_kind = term.term_type.as_deref();
        let allow_short_acronym_alias = alias_len <= 4
            && matches!(canonical_kind, Some("acronym" | "code_identifier"))
            && phonetics::similarity(alias, &term.term) >= 0.4;
        if !allow_short_acronym_alias {
            return None;
        }
    }

    match term.term_type.as_deref() {
        Some("acronym" | "proper_noun" | "brand" | "code_identifier" | "phrase") => Some(1),
        _ => {
            if phonetics::jargon_score(&term.term) >= 0.55 && phonetics::jargon_score(alias) >= 0.4
            {
                Some(2)
            } else {
                None
            }
        }
    }
}

fn replacement_trust_score(
    term: &vocabulary::VocabTerm,
    rule: &stt_replacements::SttReplacement,
) -> Option<f64> {
    let alias = rule.transcript_form.trim();
    let canonical = rule.correct_form.trim();
    if alias.is_empty() || canonical.is_empty() {
        return None;
    }
    if alias.eq_ignore_ascii_case(canonical) {
        return None;
    }

    let threshold = replacement_threshold(term, alias)?;
    if rule.use_count < threshold || rule.weight < threshold as f64 {
        return None;
    }
    if canonical_keyterm_score(term) < 2.5 {
        return None;
    }

    let phonetic = phonetics::similarity(alias, canonical);

    // For typed terms (acronym, proper_noun, brand, code_id), the classify
    // pipeline already validated this correction via phonetic triage + LLM +
    // promotion gates.  The phonetic check here is redundant and actively
    // harmful for cross-lingual pairs (e.g. Hindi "MEAH" → "Emiac" scores
    // 0.33 in the English phonetic system).  Skip it for high-signal types.
    let skip_phonetic_gate = is_high_signal_term_type(term.term_type.as_deref());
    if !skip_phonetic_gate {
        let min_phonetic = 0.50;
        if phonetic < min_phonetic {
            return None;
        }
    }

    let mut score = phonetic * 4.0;
    score += phonetics::jargon_score(canonical) * 2.5;
    score += phonetics::jargon_score(alias) * 1.5;
    score += canonical_keyterm_score(term);
    score += (rule.use_count.min(8) as f64) * 0.5;
    score += rule.weight.min(5.0) * 0.35;
    Some(score)
}

pub fn deterministic_export_tier(
    canonical: &vocabulary::VocabTerm,
    rule: &SttReplacement,
) -> ExportTier {
    if rule.contradiction_count >= 4 {
        return ExportTier::Blocked;
    }
    if alias_is_commonish(&rule.transcript_form)
        && !matches!(
            canonical.term_type.as_deref(),
            Some("acronym" | "code_identifier")
        )
    {
        return ExportTier::LocalOnly;
    }
    if replacement_trust_score(canonical, rule).is_some() {
        return ExportTier::ExportReplaceReady;
    }
    if is_precise_keyterm(canonical)
        && canonical_keyterm_score(canonical) >= 2.5
        && rule.use_count >= 1
        && rule.weight >= 1.0
        && rule.contradiction_count < 2
    {
        return ExportTier::ExportKeytermSupport;
    }
    ExportTier::LocalOnly
}

pub fn build_bias_package(
    pool: &DbPool,
    user_id: &str,
    transcription_language: &str,
    output_language: &str,
) -> BiasPackage {
    // v4: Selective biasing — only high-weight, high-confidence terms.
    // The learning pipeline guards what enters the DB (classifier rejects
    // common Hindi words like "maine", "mac"). What survives into the DB
    // with high weight is safe to bias Deepgram with.
    //
    // Only top terms (weight ≥ 2.0, use_count ≥ 3) get exported as keyterms.
    // This prevents low-confidence one-off corrections from biasing STT.
    let stt_mode = resolve_stt_mode(transcription_language);

    let vocab = vocabulary::top_terms(pool, user_id, 50);
    let keyterms: Vec<String> = vocab
        .iter()
        .filter(|v| v.weight >= 2.0 && v.use_count >= 3)
        .map(|v| v.term.clone())
        .take(20)
        .collect();

    if !keyterms.is_empty() {
        tracing::info!(
            "[stt-bias] exporting {} keyterm(s): {:?}",
            keyterms.len(),
            keyterms,
        );
    }

    BiasPackage {
        stt_mode,
        keyterms,
        replacements: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{DbPool, stt_replacements, vocabulary};
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE vocabulary (
                 user_id TEXT NOT NULL,
                 term TEXT NOT NULL,
                 weight REAL NOT NULL,
                 use_count INTEGER NOT NULL,
                 last_used INTEGER NOT NULL,
                 source TEXT NOT NULL,
                 language TEXT,
                 example_context TEXT,
                 term_type TEXT,
                 meaning TEXT,
                 meaning_updated_at INTEGER,
                 examples_since_meaning INTEGER NOT NULL DEFAULT 0,
                 UNIQUE(user_id, term)
             );
             CREATE TABLE stt_replacements (
                 user_id TEXT NOT NULL,
                 transcript_form TEXT NOT NULL,
                 correct_form TEXT NOT NULL,
                 phonetic_key TEXT NOT NULL,
                 weight REAL NOT NULL,
                 use_count INTEGER NOT NULL,
                 last_used INTEGER NOT NULL,
                 language TEXT,
                 export_tier TEXT NOT NULL DEFAULT 'local_only',
                 contradiction_count INTEGER NOT NULL DEFAULT 0,
                 last_contradicted_at INTEGER,
                 review_status TEXT NOT NULL DEFAULT 'pending',
                 review_reason TEXT,
                 last_reviewed_at INTEGER,
                 UNIQUE(user_id, transcript_form, correct_form)
             );",
        )
        .unwrap();
        pool
    }

    #[test]
    fn bias_returns_no_keyterms_and_no_replacements() {
        let pool = mem_pool();
        vocabulary::upsert_for_language_with_context(
            &pool,
            "u1",
            "EMIAC",
            2.0,
            "manual",
            "hinglish",
            Some("EMIAC technology ke baare mein"),
        );

        let bias = build_bias_package(&pool, "u1", "auto", "hinglish");
        assert_eq!(bias.stt_mode, "multi");
        assert!(bias.keyterms.is_empty(), "keyterms must be empty — biasing disabled");
        assert!(bias.replacements.is_empty());
    }
}
