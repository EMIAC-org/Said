//! Meaning-first vocabulary retrieval.
//!
//! The retriever never rewrites text. It returns a small set of evidence-rich
//! cards for the polish LLM to consider. Hot-path work is local only: SQLite,
//! cached embeddings, phonetics, and token overlap.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    embedder::gemini::blob_to_floats,
    llm::{
        phonetics,
        prompt::{VocabEntry, VocabResolution},
    },
    store::{
        DbPool, company_vocab, now_ms,
        stt_replacements::{self, ExportTier, ReviewStatus, SttReplacement},
        vocab_fts, vocabulary,
        vocabulary::VocabTerm,
    },
};

const DEFAULT_CARD_LIMIT: usize = 8;
const MAX_CARD_LIMIT: usize = 12;
const ASR_BIAS_LIMIT: usize = 20;
const PHONETIC_WITH_CONTEXT_MIN: f64 = 0.45;
const PHONETIC_STRONG_MIN: f64 = 0.90;
const EMBEDDING_SOFT_MIN: f32 = 0.62;
const EMBEDDING_STRONG_MIN: f32 = 0.78;

#[derive(Debug, Clone)]
pub struct VocabRetrievalRequest {
    pub user_id: String,
    pub transcript: String,
    pub output_language: String,
    pub target_app: Option<String>,
    pub bucket: Option<String>,
    pub screen_context: Option<String>,
    pub transcript_embedding: Option<Vec<f32>>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetrievedVocabCard {
    pub term: String,
    pub term_type: Option<String>,
    pub meaning: Option<String>,
    pub aliases: Vec<(String, i64)>,
    pub examples: Vec<String>,
    pub source: String,
    pub score: f32,
    pub evidence: Vec<VocabEvidence>,
    pub do_not_use_when: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VocabEvidence {
    pub kind: VocabEvidenceKind,
    pub span: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabEvidenceKind {
    ExactTerm,
    KnownAlias,
    Phonetic,
    Keyword,
    Meaning,
    AppBucket,
    Recent,
    Starred,
}

#[derive(Debug, Clone, Serialize)]
pub struct AsrBiasPack {
    pub keyterms: Vec<String>,
    pub prompt: String,
    pub generated_at_ms: i64,
}

#[derive(Debug, Clone)]
struct Candidate {
    term: VocabTerm,
    aliases: Vec<(String, i64)>,
    examples: Vec<String>,
    evidence: Vec<VocabEvidence>,
    score: f32,
    strong: bool,
    do_not_use_when: Option<String>,
}

impl Candidate {
    fn new(term: VocabTerm, aliases: Vec<(String, i64)>, examples: Vec<String>) -> Self {
        Self {
            term,
            aliases,
            examples,
            evidence: Vec::new(),
            score: 0.0,
            strong: false,
            do_not_use_when: None,
        }
    }

    fn add(&mut self, kind: VocabEvidenceKind, span: Option<String>, score: f32, strong: bool) {
        self.evidence.push(VocabEvidence { kind, span, score });
        self.score += score;
        self.strong |= strong;
    }

    fn into_card(self) -> RetrievedVocabCard {
        RetrievedVocabCard {
            term: self.term.term,
            term_type: self.term.term_type,
            meaning: self.term.meaning,
            aliases: self.aliases,
            examples: self.examples,
            source: self.term.source,
            score: self.score,
            evidence: self.evidence,
            do_not_use_when: self.do_not_use_when,
        }
    }
}

pub fn retrieve_after_transcription(
    pool: &DbPool,
    request: VocabRetrievalRequest,
) -> Vec<RetrievedVocabCard> {
    let transcript = request.transcript.trim();
    if transcript.is_empty() {
        return vec![];
    }

    let limit = request.limit.clamp(1, MAX_CARD_LIMIT);
    let terms = load_terms(pool, &request.user_id, &request.output_language);
    if terms.is_empty() {
        return vec![];
    }

    let alias_map = load_alias_map(pool, &request.user_id, &request.output_language);
    let examples_map = load_examples_map(pool, &request.user_id);
    let embedding_map = load_embedding_map(pool, &request.user_id);
    let transcript_tokens = tokenize(transcript);
    let transcript_token_set: HashSet<String> = transcript_tokens.iter().cloned().collect();
    let spans = ngrams(&transcript_tokens, 4);
    let fts_hits: HashSet<String> = vocab_fts::search(pool, &request.user_id, transcript, 50)
        .into_iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let screen_tokens = request
        .screen_context
        .as_deref()
        .map(tokenize)
        .unwrap_or_default();
    let app_tokens = request
        .target_app
        .as_deref()
        .into_iter()
        .chain(request.bucket.as_deref())
        .flat_map(tokenize)
        .collect::<Vec<_>>();

    let mut candidates = Vec::new();
    for term in terms {
        let key = term.term.to_ascii_lowercase();
        let aliases = alias_map.get(&key).cloned().unwrap_or_default();
        let examples = examples_map.get(&key).cloned().unwrap_or_default();
        let mut candidate = Candidate::new(term.clone(), aliases, examples);
        let support_text = support_text(&term, &candidate.aliases, &candidate.examples);
        let support_tokens = tokenize_support(&support_text);
        let support_overlap = overlap(&transcript_token_set, &support_tokens);
        let semantic_support = has_semantic_support(&support_overlap);
        let context_conflict = context_conflict_hint(&transcript_tokens, &support_tokens);
        if context_conflict.is_some() {
            candidate.do_not_use_when = context_conflict;
        }

        if contains_phrase(&transcript_tokens, &tokenize(&term.term)) {
            candidate.add(
                VocabEvidenceKind::ExactTerm,
                Some(term.term.clone()),
                95.0,
                true,
            );
        }

        add_alias_evidence(&mut candidate, &transcript_tokens, &spans);

        if !candidate.strong {
            let no_context_conflict = candidate.do_not_use_when.is_none();
            add_phonetic_evidence(
                &mut candidate,
                &spans,
                semantic_support,
                no_context_conflict,
            );
        }

        if fts_hits.contains(&key) && semantic_support {
            candidate.add(
                VocabEvidenceKind::Keyword,
                Some(
                    support_overlap
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                60.0,
                true,
            );
        }

        if semantic_support {
            candidate.add(
                VocabEvidenceKind::Meaning,
                Some(
                    support_overlap
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                if support_overlap.len() >= 2 {
                    70.0
                } else {
                    42.0
                },
                support_overlap.len() >= 2,
            );
        }

        if let (Some(query), Some(card_embedding)) = (
            request.transcript_embedding.as_deref(),
            embedding_map.get(&key).map(Vec::as_slice),
        ) {
            let sim = cosine(query, card_embedding);
            if sim >= EMBEDDING_SOFT_MIN {
                candidate.add(
                    VocabEvidenceKind::Meaning,
                    Some(format!("embedding:{sim:.2}")),
                    sim * 45.0,
                    sim >= EMBEDDING_STRONG_MIN && semantic_support,
                );
            }
        }

        add_boosts(
            &mut candidate,
            &screen_tokens,
            &app_tokens,
            &support_tokens,
            now_ms(),
        );

        if candidate.strong {
            candidates.push(candidate);
        }
    }

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(limit);
    candidates.into_iter().map(Candidate::into_card).collect()
}

pub fn cards_to_vocab_entries(cards: Vec<RetrievedVocabCard>) -> Vec<VocabEntry> {
    cards
        .into_iter()
        .map(|card| {
            let context = card.examples.first().cloned();
            VocabEntry {
                term: card.term,
                context,
                resolution: VocabResolution::Resolved,
                term_type: card.term_type,
                meaning: card.meaning,
                stt_aliases: card.aliases,
                evidence: card
                    .evidence
                    .iter()
                    .map(prompt_evidence_label)
                    .collect::<Vec<_>>(),
                do_not_use_when: card.do_not_use_when,
            }
        })
        .collect()
}

pub fn build_asr_bias_pack(pool: &DbPool, user_id: &str, language: Option<&str>) -> AsrBiasPack {
    let lang = language.unwrap_or("").trim();
    let mut terms = Vec::new();
    let mut seen = HashSet::new();
    for term in company_vocab::starred_or_priority_terms(pool, user_id, ASR_BIAS_LIMIT) {
        push_bias_term(&mut terms, &mut seen, term);
    }

    let vocab = if lang.is_empty() {
        vocabulary::top_terms(pool, user_id, 80)
    } else {
        vocabulary::top_terms_for_language(pool, user_id, lang, 80)
    };
    for term in vocab {
        let is_curated = term.source == "starred" || term.weight >= 2.0 || term.use_count >= 3;
        let has_shape_signal = !matches!(term.term_type.as_deref(), Some("other") | None)
            || phonetics::jargon_score(&term.term) >= 0.55;
        if is_curated && has_shape_signal {
            push_bias_term(&mut terms, &mut seen, term.term);
        }
        if terms.len() >= ASR_BIAS_LIMIT {
            break;
        }
    }

    let prompt = build_bias_prompt(&terms);
    AsrBiasPack {
        keyterms: terms,
        prompt,
        generated_at_ms: now_ms(),
    }
}

fn push_bias_term(terms: &mut Vec<String>, seen: &mut HashSet<String>, term: String) {
    let cleaned = term.trim();
    if cleaned.is_empty() || cleaned.chars().count() > 48 || terms.len() >= ASR_BIAS_LIMIT {
        return;
    }
    let key = cleaned.to_ascii_lowercase();
    if seen.insert(key) {
        terms.push(cleaned.to_string());
    }
}

fn load_terms(pool: &DbPool, user_id: &str, language: &str) -> Vec<VocabTerm> {
    let mut terms = if language.trim().is_empty() {
        vocabulary::top_terms(pool, user_id, 1000)
    } else {
        vocabulary::top_terms_for_language(pool, user_id, language, 1000)
    };
    for company in company_vocab::load_terms(pool, user_id, 100) {
        if !terms
            .iter()
            .any(|term| term.term.eq_ignore_ascii_case(&company.term))
        {
            terms.push(company);
        }
    }
    terms
}

fn load_alias_map(
    pool: &DbPool,
    user_id: &str,
    language: &str,
) -> HashMap<String, Vec<(String, i64)>> {
    let mut rules = stt_replacements::load_for_language(pool, user_id, language);
    rules.extend(company_vocab::load_aliases(pool, user_id));

    let mut map: HashMap<String, Vec<(String, i64)>> = HashMap::new();
    for rule in rules {
        if !alias_is_safe_for_card(&rule) {
            continue;
        }
        map.entry(rule.correct_form.to_ascii_lowercase())
            .or_default()
            .push((rule.transcript_form, rule.use_count));
    }
    for aliases in map.values_mut() {
        aliases.sort_by(|a, b| b.1.cmp(&a.1));
        aliases.truncate(8);
    }
    map
}

fn alias_is_safe_for_card(rule: &SttReplacement) -> bool {
    rule.review_status == ReviewStatus::Approved
        && rule.export_tier != ExportTier::Blocked
        && (rule.review_reason.as_deref() == Some("company_bucket")
            || stt_replacements::is_plausible_alias(&rule.transcript_form, &rule.correct_form))
}

fn load_examples_map(pool: &DbPool, user_id: &str) -> HashMap<String, Vec<String>> {
    let Ok(conn) = pool.get() else {
        return HashMap::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT term, example_text
           FROM vocab_embedding_examples
          WHERE user_id = ?1
          ORDER BY recorded_at DESC",
    ) else {
        return HashMap::new();
    };
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok(rows) = stmt.query_map([user_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) {
        for row in rows.flatten() {
            let entry = map.entry(row.0.to_ascii_lowercase()).or_default();
            if entry.len() < 4 && !entry.iter().any(|s| s == &row.1) {
                entry.push(row.1);
            }
        }
    }
    map
}

fn load_embedding_map(pool: &DbPool, user_id: &str) -> HashMap<String, Vec<f32>> {
    let Ok(conn) = pool.get() else {
        return HashMap::new();
    };
    let Ok(mut stmt) =
        conn.prepare("SELECT term, embedding FROM vocab_embeddings WHERE user_id = ?1")
    else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    if let Ok(rows) = stmt.query_map([user_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    }) {
        for (term, blob) in rows.flatten() {
            if let Some(vector) = blob_to_floats(&blob) {
                map.insert(term.to_ascii_lowercase(), vector);
            }
        }
    }
    map
}

fn add_alias_evidence(candidate: &mut Candidate, transcript_tokens: &[String], spans: &[String]) {
    for (alias, count) in candidate.aliases.clone() {
        let alias_tokens = tokenize(&alias);
        if alias_tokens.is_empty() {
            continue;
        }
        if contains_phrase(transcript_tokens, &alias_tokens) {
            candidate.add(
                VocabEvidenceKind::KnownAlias,
                Some(alias),
                82.0 + (count.max(0) as f32).ln_1p(),
                true,
            );
            return;
        }
        let alias_key = phonetics::phonetic_key(&alias);
        if alias_key.is_empty() {
            continue;
        }
        if let Some((span, sim)) = best_phonetic_span(spans, &alias, &alias_key, 0.74) {
            candidate.add(
                VocabEvidenceKind::KnownAlias,
                Some(span),
                (sim as f32 * 72.0) + (count.max(0) as f32).ln_1p(),
                true,
            );
            return;
        }
    }
}

fn add_phonetic_evidence(
    candidate: &mut Candidate,
    spans: &[String],
    semantic_support: bool,
    no_context_conflict: bool,
) {
    let term = candidate.term.term.clone();
    if compact_alnum(&term).chars().count() < 4 {
        return;
    }
    let term_key = phonetics::phonetic_key(&term);
    if term_key.is_empty() {
        return;
    }
    let Some((span, sim)) = best_phonetic_span(spans, &term, &term_key, PHONETIC_WITH_CONTEXT_MIN)
    else {
        return;
    };
    let strong = sim >= PHONETIC_STRONG_MIN || (semantic_support && no_context_conflict);
    if strong {
        candidate.add(
            VocabEvidenceKind::Phonetic,
            Some(span),
            sim as f32 * 68.0,
            true,
        );
    }
}

fn best_phonetic_span(
    spans: &[String],
    target: &str,
    target_key: &str,
    min_sim: f64,
) -> Option<(String, f64)> {
    let target_first = compact_alnum(target).chars().next()?;
    let mut best: Option<(String, f64)> = None;
    for span in spans {
        let compact = compact_alnum(span);
        if compact.chars().count() < 3 {
            continue;
        }
        if compact.chars().next()? != target_first {
            continue;
        }
        let span_key = phonetics::phonetic_key(span);
        if span_key.is_empty() {
            continue;
        }
        let sim = phonetics::similarity(&span_key, target_key);
        if sim < min_sim {
            continue;
        }
        if best
            .as_ref()
            .map(|(_, best_sim)| sim > *best_sim)
            .unwrap_or(true)
        {
            best = Some((span.clone(), sim));
        }
    }
    best
}

fn add_boosts(
    candidate: &mut Candidate,
    screen_tokens: &[String],
    app_tokens: &[String],
    support_tokens: &HashSet<String>,
    now: i64,
) {
    if candidate.term.source == "starred" {
        candidate.add(VocabEvidenceKind::Starred, None, 8.0, false);
    }
    let days_since = (now - candidate.term.last_used).max(0) as f32 / 86_400_000.0;
    if candidate.term.use_count >= 3 || days_since <= 14.0 {
        let recency = (14.0 - days_since).max(0.0) / 14.0;
        candidate.add(
            VocabEvidenceKind::Recent,
            None,
            4.0 + recency + (candidate.term.use_count.max(0) as f32).ln_1p(),
            false,
        );
    }
    if screen_tokens
        .iter()
        .any(|token| support_tokens.contains(token))
        || app_tokens
            .iter()
            .any(|token| support_tokens.contains(token))
    {
        candidate.add(VocabEvidenceKind::AppBucket, None, 5.0, false);
    }
}

fn support_text(term: &VocabTerm, aliases: &[(String, i64)], examples: &[String]) -> String {
    let mut parts = Vec::new();
    if let Some(meaning) = term.meaning.as_deref().filter(|s| !s.trim().is_empty()) {
        parts.push(meaning.to_string());
    }
    if let Some(context) = term
        .example_context
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        parts.push(context.to_string());
    }
    parts.extend(examples.iter().cloned());
    parts.extend(aliases.iter().map(|(alias, _)| alias.clone()));
    parts.join(" ")
}

fn has_semantic_support(overlap: &[String]) -> bool {
    overlap.len() >= 2 || overlap.iter().any(|token| token.chars().count() >= 7)
}

fn overlap(query_tokens: &HashSet<String>, support_tokens: &HashSet<String>) -> Vec<String> {
    let mut found = support_tokens
        .intersection(query_tokens)
        .filter(|token| !is_stopword(token))
        .cloned()
        .collect::<Vec<_>>();
    found.sort();
    found
}

fn context_conflict_hint(
    transcript_tokens: &[String],
    support_tokens: &HashSet<String>,
) -> Option<String> {
    let beauty = [
        "makeup",
        "make",
        "cosmetic",
        "cosmetics",
        "beauty",
        "shade",
        "lipstick",
        "foundation",
        "concealer",
    ];
    let transcript_has_beauty = transcript_tokens
        .iter()
        .any(|token| beauty.contains(&token.as_str()));
    let support_has_beauty = support_tokens
        .iter()
        .any(|token| beauty.contains(&token.as_str()));
    if transcript_has_beauty && !support_has_beauty {
        Some("cosmetics, makeup products, beauty context".to_string())
    } else {
        None
    }
}

fn prompt_evidence_label(evidence: &VocabEvidence) -> String {
    let name = match evidence.kind {
        VocabEvidenceKind::ExactTerm => "exact",
        VocabEvidenceKind::KnownAlias => "known_alias",
        VocabEvidenceKind::Phonetic => "phonetic",
        VocabEvidenceKind::Keyword => "keyword",
        VocabEvidenceKind::Meaning => "meaning",
        VocabEvidenceKind::AppBucket => "app_bucket",
        VocabEvidenceKind::Recent => "recent",
        VocabEvidenceKind::Starred => "starred",
    };
    match evidence.span.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(span) => format!("{name}({span})"),
        None => name.to_string(),
    }
}

fn build_bias_prompt(terms: &[String]) -> String {
    if terms.is_empty() {
        return String::new();
    }
    let mut kept = Vec::new();
    let mut used = "Possible uncommon spellings: ".chars().count();
    for term in terms {
        let extra = term.chars().count() + if kept.is_empty() { 0 } else { 2 };
        if used + extra > 220 {
            break;
        }
        used += extra;
        kept.push(term.as_str());
    }
    if kept.is_empty() {
        String::new()
    } else {
        format!("Possible uncommon spellings: {}.", kept.join(", "))
    }
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.chars().count() >= 2)
        .collect()
}

fn tokenize_support(text: &str) -> HashSet<String> {
    tokenize(text)
        .into_iter()
        .filter(|token| token.chars().count() >= 3 && !is_stopword(token))
        .collect()
}

fn ngrams(tokens: &[String], max_len: usize) -> Vec<String> {
    let mut spans = Vec::new();
    for start in 0..tokens.len() {
        for len in 1..=max_len {
            if start + len > tokens.len() {
                break;
            }
            spans.push(tokens[start..start + len].join(" "));
        }
    }
    spans
}

fn contains_phrase(tokens: &[String], phrase: &[String]) -> bool {
    if phrase.is_empty() {
        return false;
    }
    if phrase.len() == 1 {
        return tokens.iter().any(|token| token == &phrase[0]);
    }
    tokens.windows(phrase.len()).any(|window| window == phrase)
}

fn compact_alnum(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut an = 0.0;
    let mut bn = 0.0;
    for i in 0..len {
        dot += a[i] * b[i];
        an += a[i] * a[i];
        bn += b[i] * b[i];
    }
    if an == 0.0 || bn == 0.0 {
        0.0
    } else {
        dot / (an.sqrt() * bn.sqrt())
    }
}

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "are"
            | "aur"
            | "hai"
            | "hain"
            | "he"
            | "hi"
            | "ka"
            | "kar"
            | "ke"
            | "ki"
            | "ko"
            | "main"
            | "mein"
            | "me"
            | "the"
            | "to"
            | "with"
            | "you"
            | "your"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2_sqlite::SqliteConnectionManager;
    use rusqlite::params;

    fn mem_pool() -> DbPool {
        crate::legacy_learning::enable_debug_legacy_writes_for_tests();
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        pool.get()
            .unwrap()
            .execute_batch(
                "CREATE TABLE local_user (id TEXT PRIMARY KEY);
                 INSERT INTO local_user(id) VALUES ('u1');
                 CREATE TABLE vocabulary (
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     weight REAL NOT NULL DEFAULT 1.0,
                     use_count INTEGER NOT NULL DEFAULT 1,
                     last_used INTEGER NOT NULL,
                     source TEXT NOT NULL DEFAULT 'auto',
                     language TEXT,
                     example_context TEXT,
                     term_type TEXT,
                     meaning TEXT,
                     UNIQUE(user_id, term)
                 );
                 CREATE TABLE stt_replacements (
                     user_id TEXT NOT NULL,
                     transcript_form TEXT NOT NULL,
                     correct_form TEXT NOT NULL,
                     phonetic_key TEXT NOT NULL DEFAULT '',
                     weight REAL NOT NULL DEFAULT 1.0,
                     use_count INTEGER NOT NULL DEFAULT 1,
                     last_used INTEGER NOT NULL DEFAULT 0,
                     language TEXT,
                     export_tier TEXT NOT NULL DEFAULT 'export_replace_ready',
                     contradiction_count INTEGER NOT NULL DEFAULT 0,
                     review_status TEXT NOT NULL DEFAULT 'approved',
                     review_reason TEXT,
                     last_reviewed_at INTEGER
                 );
                 CREATE TABLE vocab_embedding_examples (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     embedding BLOB NOT NULL,
                     example_text TEXT NOT NULL,
                     recorded_at INTEGER NOT NULL
                 );
                 CREATE TABLE vocab_embeddings (
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     embedding BLOB NOT NULL,
                     updated_at INTEGER NOT NULL,
                     UNIQUE(user_id, term)
                 );
                 CREATE TABLE company_vocabulary (
                     user_id TEXT NOT NULL,
                     term TEXT NOT NULL,
                     term_norm TEXT NOT NULL,
                     term_type TEXT,
                     language TEXT,
                     weight REAL NOT NULL DEFAULT 1.0,
                     priority INTEGER NOT NULL DEFAULT 0,
                     status TEXT NOT NULL DEFAULT 'approved',
                     updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE company_stt_replacements (
                     user_id TEXT NOT NULL,
                     transcript_form TEXT NOT NULL,
                     transcript_norm TEXT NOT NULL,
                     correct_form TEXT NOT NULL,
                     correct_norm TEXT NOT NULL,
                     language TEXT,
                     weight REAL NOT NULL DEFAULT 1.0,
                     safety_status TEXT NOT NULL DEFAULT 'approved',
                     status TEXT NOT NULL DEFAULT 'approved',
                     updated_at INTEGER NOT NULL
                 );
                 CREATE VIRTUAL TABLE vocab_fts USING fts5(
                     user_id UNINDEXED,
                     term UNINDEXED,
                     card_text,
                     tokenize = 'unicode61 remove_diacritics 2'
                 );",
            )
            .unwrap();
        pool
    }

    fn seed_term(pool: &DbPool, term: &str, meaning: &str, context: &str, source: &str) {
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO vocabulary
                    (user_id, term, weight, use_count, last_used, source, language, example_context, term_type, meaning)
                 VALUES ('u1', ?1, 3.0, 5, ?2, ?3, 'hinglish', ?4, 'brand', ?5)",
                params![term, now_ms(), source, context, meaning],
            )
            .unwrap();
        vocab_fts::upsert(pool, "u1", term, Some(context));
    }

    fn req(transcript: &str) -> VocabRetrievalRequest {
        VocabRetrievalRequest {
            user_id: "u1".into(),
            transcript: transcript.into(),
            output_language: "hinglish".into(),
            target_app: None,
            bucket: None,
            screen_context: None,
            transcript_embedding: None,
            limit: 8,
        }
    }

    #[test]
    fn phonetic_plus_meaning_retrieves_macobs() {
        let pool = mem_pool();
        seed_term(
            &pool,
            "MACOBS",
            "Internal onboarding product workflow for account setup and rollout.",
            "MACOBS onboarding flow for new users",
            "auto",
        );

        let cards = retrieve_after_transcription(&pool, req("main cops onboarding flow batao"));
        assert_eq!(cards.first().map(|c| c.term.as_str()), Some("MACOBS"));
        assert!(
            cards[0]
                .evidence
                .iter()
                .any(|e| e.kind == VocabEvidenceKind::Phonetic)
        );
    }

    #[test]
    fn makeup_context_does_not_retrieve_macobs() {
        let pool = mem_pool();
        seed_term(
            &pool,
            "MACOBS",
            "Internal onboarding product workflow for account setup and rollout.",
            "MACOBS onboarding flow for new users",
            "auto",
        );

        let cards = retrieve_after_transcription(&pool, req("makeup shade party ke liye"));
        assert!(cards.is_empty(), "beauty context must not pull MACOBS");
    }

    #[test]
    fn exact_alias_retrieves_without_embedding() {
        let pool = mem_pool();
        seed_term(
            &pool,
            "MACOBS",
            "Internal onboarding workflow.",
            "MACOBS onboarding",
            "auto",
        );
        pool.get()
            .unwrap()
            .execute(
                "INSERT INTO stt_replacements
                    (user_id, transcript_form, correct_form, phonetic_key, weight, use_count, last_used, export_tier, review_status)
                 VALUES ('u1', 'main cops', 'MACOBS', 'mnkps', 2.0, 4, ?1, 'export_replace_ready', 'approved')",
                params![now_ms()],
            )
            .unwrap();

        let cards = retrieve_after_transcription(&pool, req("main cops status"));
        assert_eq!(cards.first().map(|c| c.term.as_str()), Some("MACOBS"));
        assert!(
            cards[0]
                .evidence
                .iter()
                .any(|e| e.kind == VocabEvidenceKind::KnownAlias)
        );
    }

    #[test]
    fn starred_recent_boost_cannot_admit_alone() {
        let pool = mem_pool();
        seed_term(
            &pool,
            "Vipassana",
            "Meditation retreat term.",
            "Vipassana schedule",
            "starred",
        );

        let cards = retrieve_after_transcription(&pool, req("send the invoice tomorrow"));
        assert!(cards.is_empty());
    }

    #[test]
    fn asr_bias_is_bounded_and_curated() {
        let pool = mem_pool();
        seed_term(
            &pool,
            "MACOBS",
            "Internal workflow.",
            "MACOBS onboarding",
            "starred",
        );
        for idx in 0..40 {
            seed_term(
                &pool,
                &format!("Generic{idx}"),
                "Generic term.",
                "unused",
                "auto",
            );
        }

        let pack = build_asr_bias_pack(&pool, "u1", Some("hinglish"));
        assert!(pack.keyterms.contains(&"MACOBS".to_string()));
        assert!(pack.keyterms.len() <= ASR_BIAS_LIMIT);
        assert!(pack.prompt.chars().count() <= 230);
    }
}
