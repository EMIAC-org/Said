use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use voice_polish_core::deepgram::{BiasPackage, ReplacementRule, resolve_stt_mode};

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

fn alias_support_score(rule: &SttReplacement) -> f64 {
    let mut score = 0.0;
    match rule.export_tier {
        ExportTier::ExportReplaceReady => score += 2.0,
        ExportTier::ExportKeytermSupport => score += 1.0,
        ExportTier::Blocked => score -= 2.0,
        ExportTier::LocalOnly => {}
    }
    score += (rule.use_count.min(8) as f64) * 0.2;
    score += rule.weight.min(5.0) * 0.2;
    score - (rule.contradiction_count as f64 * 0.75)
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
        Some("acronym" | "proper_noun" | "brand" | "code_identifier" | "phrase") => Some(3),
        _ => {
            if phonetics::jargon_score(&term.term) >= 0.55 && phonetics::jargon_score(alias) >= 0.4
            {
                Some(3)
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
    if canonical_keyterm_score(term) < 3.0 {
        return None;
    }

    let phonetic = phonetics::similarity(alias, canonical);
    let min_phonetic = match term.term_type.as_deref() {
        Some("acronym") => 0.42,
        Some("phrase") => 0.52,
        Some("code_identifier") => 0.48,
        Some("brand" | "proper_noun") => 0.58,
        _ => 0.65,
    };
    if phonetic < min_phonetic {
        return None;
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
        && canonical_keyterm_score(canonical) >= 3.0
        && rule.use_count >= 2
        && rule.weight >= 2.0
        && rule.contradiction_count < 2
    {
        return ExportTier::ExportKeytermSupport;
    }
    ExportTier::LocalOnly
}

fn effective_export_tier(canonical: &vocabulary::VocabTerm, rule: &SttReplacement) -> ExportTier {
    if rule.review_status == ReviewStatus::Blocked || rule.export_tier == ExportTier::Blocked {
        return ExportTier::Blocked;
    }
    let deterministic = deterministic_export_tier(canonical, rule);
    match rule.review_status {
        ReviewStatus::Approved | ReviewStatus::Skipped => rule.export_tier,
        ReviewStatus::Pending => deterministic,
        ReviewStatus::Blocked => ExportTier::Blocked,
    }
}

pub fn build_bias_package(
    pool: &DbPool,
    user_id: &str,
    transcription_language: &str,
    output_language: &str,
) -> BiasPackage {
    let stt_mode = resolve_stt_mode(transcription_language);
    let vocab_terms = vocabulary::top_terms_for_language(pool, user_id, output_language, 200);
    let alias_rules = stt_replacements::load_for_language(pool, user_id, output_language);
    let vocab_by_term: HashMap<String, vocabulary::VocabTerm> = vocab_terms
        .iter()
        .cloned()
        .map(|term| (term.term.to_ascii_lowercase(), term))
        .collect();
    let mut alias_support_by_canonical: HashMap<String, f64> = HashMap::new();
    for rule in &alias_rules {
        let Some(canonical) = vocab_by_term.get(&rule.correct_form.to_ascii_lowercase()) else {
            continue;
        };
        let effective_tier = effective_export_tier(canonical, rule);
        if effective_tier == ExportTier::Blocked {
            continue;
        }
        let mut effective_rule = rule.clone();
        effective_rule.export_tier = effective_tier;
        *alias_support_by_canonical
            .entry(rule.correct_form.to_ascii_lowercase())
            .or_insert(0.0) += alias_support_score(&effective_rule);
    }

    let mut seen_terms = HashSet::new();
    let mut keyterms = Vec::new();
    let mut keyterm_candidates: Vec<&vocabulary::VocabTerm> = vocab_terms
        .iter()
        .filter(|t| is_precise_keyterm(t))
        .collect();
    keyterm_candidates.sort_by(|a, b| {
        let sb = canonical_keyterm_score(b)
            + alias_support_by_canonical
                .get(&b.term.to_ascii_lowercase())
                .copied()
                .unwrap_or(0.0);
        let sa = canonical_keyterm_score(a)
            + alias_support_by_canonical
                .get(&a.term.to_ascii_lowercase())
                .copied()
                .unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(Ordering::Equal)
    });
    for term in keyterm_candidates {
        let lowered = term.term.to_ascii_lowercase();
        if seen_terms.insert(lowered) {
            keyterms.push(term.term.clone());
        }
        if keyterms.len() >= voice_polish_core::deepgram::MAX_KEYTERMS {
            break;
        }
    }

    let mut seen_replacements = HashSet::new();
    let mut replacement_candidates: Vec<(f64, ReplacementRule)> = Vec::new();
    for rule in alias_rules {
        let find = rule.transcript_form.trim();
        let replace = rule.correct_form.trim();
        if find.is_empty() || replace.is_empty() {
            continue;
        }
        if find.eq_ignore_ascii_case(replace) {
            continue;
        }
        if rule.review_status == ReviewStatus::Blocked {
            continue;
        }

        let Some(canonical) = vocab_by_term.get(&replace.to_ascii_lowercase()) else {
            continue;
        };
        let effective_tier = effective_export_tier(canonical, &rule);
        if effective_tier != ExportTier::ExportReplaceReady {
            continue;
        }
        let Some(score) = replacement_trust_score(canonical, &rule) else {
            continue;
        };

        let dedupe_key = format!(
            "{}=>{}",
            find.to_ascii_lowercase(),
            replace.to_ascii_lowercase()
        );
        if !seen_replacements.insert(dedupe_key) {
            continue;
        }
        replacement_candidates.push((
            score,
            ReplacementRule {
                find: find.to_ascii_lowercase(),
                replace: Some(replace.to_string()),
            },
        ));
    }

    replacement_candidates.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal));
    let replacements = replacement_candidates
        .into_iter()
        .take(voice_polish_core::deepgram::MAX_REPLACEMENTS)
        .map(|(_, rule)| rule)
        .collect();

    BiasPackage {
        stt_mode,
        keyterms,
        replacements,
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
    fn builds_multi_bias_and_filters_low_trust_replacements() {
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
        vocabulary::upsert_for_language_with_context(
            &pool,
            "u1",
            "return",
            1.0,
            "auto",
            "hinglish",
            Some("return ka automation"),
        );
        stt_replacements::upsert_aliases_for_language(
            &pool, "u1", "emi", "emi", "EMIAC", 1.0, "hinglish",
        );
        stt_replacements::upsert_aliases_for_language(
            &pool, "u1", "emi", "emi", "EMIAC", 1.0, "hinglish",
        );
        stt_replacements::upsert_aliases_for_language(
            &pool, "u1", "emi", "emi", "EMIAC", 1.0, "hinglish",
        );
        stt_replacements::upsert_aliases_for_language(
            &pool, "u1", "return", "return", "return", 1.0, "hinglish",
        );
        stt_replacements::upsert_aliases_for_language(
            &pool, "u1", "return", "return", "return", 1.0, "hinglish",
        );
        stt_replacements::upsert_aliases_for_language(
            &pool, "u1", "return", "return", "return", 1.0, "hinglish",
        );

        let bias = build_bias_package(&pool, "u1", "auto", "hinglish");
        assert_eq!(bias.stt_mode, "multi");
        assert!(bias.keyterms.contains(&"EMIAC".to_string()));
        assert!(
            bias.replacements
                .iter()
                .any(|r| r.find == "emi" && r.replace.as_deref() == Some("EMIAC"))
        );
        assert!(
            !bias
                .replacements
                .iter()
                .any(|r| r.find == "return" && r.replace.as_deref() == Some("return"))
        );
    }

    #[test]
    fn common_word_aliases_never_export_as_replacements() {
        let pool = mem_pool();
        vocabulary::upsert_for_language_with_context(
            &pool,
            "u1",
            "ProjectAtlas",
            2.0,
            "manual",
            "hinglish",
            Some("ProjectAtlas roadmap dekhna"),
        );
        for _ in 0..5 {
            stt_replacements::upsert_aliases_for_language(
                &pool,
                "u1",
                "return",
                "return",
                "ProjectAtlas",
                1.0,
                "hinglish",
            );
        }

        let bias = build_bias_package(&pool, "u1", "auto", "hinglish");
        assert!(
            !bias
                .replacements
                .iter()
                .any(|r| r.find == "return" && r.replace.as_deref() == Some("ProjectAtlas"))
        );
        assert!(bias.keyterms.contains(&"ProjectAtlas".to_string()));
    }
}
