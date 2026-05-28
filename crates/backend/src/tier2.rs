//! Tier 2 local learning correction pipeline.
//!
//! Voice runtime order:
//! 1. `collect_evidence` — scan raw/numeric transcript, no mutation.
//! 2. LLM grammar polish.
//! 3. `final_resolve` — apply high-trust protected-term replacements on polished text.
//!
//! Legacy `correct()` / `correct_with_store()` remain for eval/lab paths.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::{
    llm::{phonetics, promotion_gate},
    store::{
        DbPool,
        stt_replacements::{
            AppliedMatch, ApplyResult, MatchKind, ReviewStatus, ScoringTrace, SttReplacement,
        },
        tier2_edit_policy::{self, Tier2EditPolicyRule},
        tier2_model::Tier2ModelMetadata,
        tier2_policy::{self, PolicyKey, Tier2PolicyWeight},
        vocabulary::{self, VocabTerm},
    },
};

const MIN_TOKEN_LEN: usize = 3;
const SCORE_THRESHOLD: f64 = 0.86;
const MARGIN_THRESHOLD: f64 = 0.18;
const DETERMINISTIC_FLOOR: f64 = 0.68;
const TRACE_REJECTED_SCORE_FLOOR: f64 = 0.74;
const MAX_REPLACEMENTS_PER_RECORDING: usize = 3;
const DEFAULT_MAX_LEN: usize = 32;
const CLUSTER_FUZZY_THRESHOLD: f64 = 0.78;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveScorer {
    Deterministic,
    Onnx,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tier2Status {
    pub enabled: bool,
    pub active_scorer: ActiveScorer,
    pub artifact_present: bool,
    pub trained_at: Option<i64>,
    pub alias_count: i64,
    pub vocab_count: i64,
    pub data_fingerprint: Option<String>,
    pub last_error: Option<String>,
    pub policy_pair_count: i64,
    pub positive_reward_count: i64,
    pub negative_reward_count: i64,
    pub pending_onnx_refresh: bool,
    pub onnx_trust_level: String,
    pub last_policy_update_at: Option<i64>,
    pub candidate_rule_count: i64,
    pub active_rule_count: i64,
    pub blocked_rule_count: i64,
    pub explicit_positive_count: i64,
    pub negative_count: i64,
    pub fuzzy_shadow_count: i64,
}

#[derive(Debug, Clone)]
struct Candidate {
    term: String,
    term_lower: String,
    term_type: String,
    source: String,
    weight: f64,
    use_count: i64,
    aliases: Vec<AliasEvidence>,
}

#[derive(Debug, Clone)]
struct AliasEvidence {
    transcript_form: String,
    weight: f64,
    use_count: i64,
}

/// One protected-term signal collected from raw STT before LLM polish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedTermEvidence {
    pub transcript_form: String,
    pub correct_form: String,
    pub kind: MatchKind,
    pub raw_token_index: Option<usize>,
}

/// Evidence collected from raw/numeric transcript without mutating it.
#[derive(Debug, Clone)]
pub struct EvidenceResult {
    pub source_text: String,
    pub evidence: Vec<ProtectedTermEvidence>,
    pub matches: Vec<AppliedMatch>,
    pub traces: Vec<ScoringTrace>,
}

impl EvidenceResult {
    pub fn as_apply_result(&self) -> ApplyResult {
        ApplyResult {
            text: self.source_text.clone(),
            matches: self.matches.clone(),
            traces: self.traces.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct CandidateScore {
    candidate: Candidate,
    score: f64,
    kind: MatchKind,
    deterministic_score: f64,
    onnx_score: Option<f64>,
    policy_score: Option<f64>,
    policy_boost: f64,
}

#[derive(Debug, Clone)]
struct ScoringDecision {
    best: CandidateScore,
    second: Option<CandidateScore>,
    margin: f64,
    applied: bool,
}

pub struct CorrectionEngine<'a> {
    stt_replacements: &'a [SttReplacement],
    vocabulary: &'a [VocabTerm],
    metadata: Option<Tier2ModelMetadata>,
    policy_weights: HashMap<PolicyKey, Tier2PolicyWeight>,
    edit_policy_rules: Vec<Tier2EditPolicyRule>,
}

impl<'a> CorrectionEngine<'a> {
    pub fn new(
        stt_replacements: &'a [SttReplacement],
        vocabulary: &'a [VocabTerm],
        metadata: Option<Tier2ModelMetadata>,
    ) -> Self {
        Self {
            stt_replacements,
            vocabulary,
            metadata,
            policy_weights: HashMap::new(),
            edit_policy_rules: vec![],
        }
    }

    pub fn new_with_policy(
        stt_replacements: &'a [SttReplacement],
        vocabulary: &'a [VocabTerm],
        metadata: Option<Tier2ModelMetadata>,
        policy_weights: HashMap<PolicyKey, Tier2PolicyWeight>,
        edit_policy_rules: Vec<Tier2EditPolicyRule>,
    ) -> Self {
        Self {
            stt_replacements,
            vocabulary,
            metadata,
            policy_weights,
            edit_policy_rules,
        }
    }

    /// Full correction pass: exact safe aliases first, then Tier 2 scoring.
    pub fn correct(&self, transcript: &str) -> ApplyResult {
        let exact =
            crate::store::stt_replacements::apply_exact_safe(transcript, self.stt_replacements);
        for m in &exact.matches {
            info!(
                "[tier2] exact alias: {:?} → {:?} ({:?})",
                m.transcript_form, m.correct_form, m.kind
            );
        }
        let exact_outputs: HashSet<String> = exact
            .matches
            .iter()
            .map(|m| normalize_core(&m.correct_form))
            .filter(|s| !s.is_empty())
            .collect();
        let edit_policy = self.apply_edit_policy_only_skipping(&exact.text, &exact_outputs);
        for m in &edit_policy.matches {
            info!(
                "[tier2] edit-policy: {:?} → {:?} ({:?})",
                m.transcript_form, m.correct_form, m.kind
            );
        }
        for t in &edit_policy.traces {
            if !t.applied {
                debug!(
                    "[tier2] edit-policy skip: {:?} → {:?} score={:.3} (context_fail)",
                    t.token, t.candidate, t.score
                );
            }
        }
        let corrected_outputs: HashSet<String> = exact_outputs
            .into_iter()
            .chain(
                edit_policy
                    .matches
                    .iter()
                    .map(|m| normalize_core(&m.correct_form))
                    .filter(|s| !s.is_empty()),
            )
            .collect();
        let mut result = combine_results(exact, edit_policy);

        let cluster_fuzzy = cluster_fuzzy_pass(
            &result.text,
            &self.edit_policy_rules,
            &corrected_outputs,
            self.vocabulary,
            self.stt_replacements,
        );
        for m in &cluster_fuzzy.matches {
            info!(
                "[tier2] cluster-fuzzy: {:?} → {:?} ({:?})",
                m.transcript_form, m.correct_form, m.kind
            );
        }
        let corrected_outputs: HashSet<String> = corrected_outputs
            .into_iter()
            .chain(
                cluster_fuzzy
                    .matches
                    .iter()
                    .map(|m| normalize_core(&m.correct_form))
                    .filter(|s| !s.is_empty()),
            )
            .collect();
        result = combine_results(result, cluster_fuzzy);

        let fuzzy = self.fuzzy_scoring_pass(&result.text, &corrected_outputs);
        for t in &fuzzy.traces {
            let label = if t.applied { "APPLY" } else { "shadow" };
            let scorer = match t.kind {
                MatchKind::Tier2Onnx => "onnx",
                MatchKind::Tier2Deterministic => "det",
                MatchKind::Tier2Policy => "policy",
                _ => "other",
            };
            info!(
                "[tier2] {label} [{scorer}]: {:?} → {:?} score={:.3} (det={:.3} onnx={} margin={:.3})",
                t.token,
                t.candidate,
                t.score,
                t.deterministic_score,
                t.onnx_score
                    .map(|s| format!("{s:.3}"))
                    .unwrap_or_else(|| "—".into()),
                t.margin,
            );
        }
        result = combine_results(result, fuzzy);
        if !result.matches.is_empty() {
            info!(
                "[tier2] final: {} correction(s) applied, output: {:?}",
                result.matches.len(),
                truncate_utf8_safe(&result.text, 200)
            );
        }
        result
    }

    /// Tier 2 only. The caller must already have applied exact aliases.
    pub fn correct_tier2_only(&self, transcript: &str) -> ApplyResult {
        let mut result = self.apply_edit_policy_only_skipping(transcript, &HashSet::new());
        let corrected: HashSet<String> = result
            .matches
            .iter()
            .map(|m| normalize_core(&m.correct_form))
            .filter(|s| !s.is_empty())
            .collect();
        let cluster_fuzzy = cluster_fuzzy_pass(
            &result.text,
            &self.edit_policy_rules,
            &corrected,
            self.vocabulary,
            self.stt_replacements,
        );
        let corrected: HashSet<String> = corrected
            .into_iter()
            .chain(
                cluster_fuzzy
                    .matches
                    .iter()
                    .map(|m| normalize_core(&m.correct_form))
                    .filter(|s| !s.is_empty()),
            )
            .collect();
        result = combine_results(result, cluster_fuzzy);
        let fuzzy = self.fuzzy_scoring_pass(&result.text, &corrected);
        result = combine_results(result, fuzzy);
        result
    }

    fn apply_edit_policy_only_skipping(
        &self,
        transcript: &str,
        protected_tokens: &HashSet<String>,
    ) -> ApplyResult {
        apply_edit_policy(
            transcript,
            &self.edit_policy_rules,
            protected_tokens,
            self.vocabulary,
        )
    }

    fn fuzzy_scoring_pass(
        &self,
        transcript: &str,
        protected_tokens: &HashSet<String>,
    ) -> ApplyResult {
        let candidates = build_candidates(self.stt_replacements, self.vocabulary);
        if candidates.is_empty() {
            return ApplyResult {
                text: transcript.to_string(),
                matches: vec![],
                traces: vec![],
            };
        }

        let onnx_model =
            self.metadata
                .as_ref()
                .and_then(|metadata| match load_onnx_model(metadata) {
                    Ok(model) => {
                        info!("[tier2] ONNX model loaded — scoring with neural net");
                        Some(model)
                    }
                    Err(err) => {
                        info!("[tier2] ONNX unavailable ({err}) — deterministic scorer only");
                        None
                    }
                });

        if self.metadata.is_none() {
            info!("[tier2] no model metadata in DB — deterministic scorer only");
        }

        if let Some(model) = onnx_model {
            match apply_tier2(
                transcript,
                &candidates,
                Some(&model),
                protected_tokens,
                &self.policy_weights,
                true,
            ) {
                Ok(result) => result,
                Err(err) => {
                    debug!("[tier2] ONNX inference failed, using deterministic scorer: {err}");
                    apply_tier2(
                        transcript,
                        &candidates,
                        None,
                        protected_tokens,
                        &self.policy_weights,
                        true,
                    )
                    .unwrap_or_else(|_| ApplyResult {
                        text: transcript.to_string(),
                        matches: vec![],
                        traces: vec![],
                    })
                }
            }
        } else {
            apply_tier2(
                transcript,
                &candidates,
                None,
                protected_tokens,
                &self.policy_weights,
                true,
            )
            .unwrap_or_else(|_| ApplyResult {
                text: transcript.to_string(),
                matches: vec![],
                traces: vec![],
            })
        }
    }

    /// Scan transcript for protected-term evidence without mutating text.
    pub fn collect_evidence(&self, transcript: &str) -> EvidenceResult {
        info!(
            "[tier2] collect_evidence: {:?}",
            truncate_utf8_safe(transcript, 200)
        );
        let exact =
            crate::store::stt_replacements::apply_exact_safe(transcript, self.stt_replacements);
        let exact_outputs: HashSet<String> = exact
            .matches
            .iter()
            .map(|m| normalize_core(&m.correct_form))
            .filter(|s| !s.is_empty())
            .collect();

        let edit_policy = apply_edit_policy(
            transcript,
            &self.edit_policy_rules,
            &exact_outputs,
            self.vocabulary,
        );

        let corrected_outputs: HashSet<String> = exact_outputs
            .into_iter()
            .chain(
                edit_policy
                    .matches
                    .iter()
                    .map(|m| normalize_core(&m.correct_form))
                    .filter(|s| !s.is_empty()),
            )
            .collect();

        let cluster_fuzzy = cluster_fuzzy_pass(
            transcript,
            &self.edit_policy_rules,
            &corrected_outputs,
            self.vocabulary,
            self.stt_replacements,
        );

        let fuzzy = self.fuzzy_scoring_pass(transcript, &corrected_outputs);

        let mut matches = exact.matches;
        matches.extend(edit_policy.matches);
        matches.extend(cluster_fuzzy.matches);

        let mut traces = edit_policy.traces;
        traces.extend(cluster_fuzzy.traces);
        traces.extend(fuzzy.traces);

        let evidence = matches
            .iter()
            .map(|m| ProtectedTermEvidence {
                transcript_form: m.transcript_form.clone(),
                correct_form: m.correct_form.clone(),
                kind: m.kind,
                raw_token_index: token_index_of(transcript, &m.transcript_form),
            })
            .collect();

        EvidenceResult {
            source_text: transcript.to_string(),
            evidence,
            matches,
            traces,
        }
    }

    /// Apply high-trust protected-term replacements to polished text using raw evidence.
    pub fn final_resolve(
        &self,
        polished: &str,
        evidence: &EvidenceResult,
        raw_source: &str,
    ) -> ApplyResult {
        info!(
            "[tier2] final_resolve: polished={:?} evidence={} traces={}",
            truncate_utf8_safe(polished, 120),
            evidence.matches.len(),
            evidence.traces.len()
        );

        let mut text = polished.to_string();
        let mut matches = Vec::new();
        let mut traces = Vec::new();
        let mut replacement_count = 0usize;
        let mut applied_forms: HashSet<String> = HashSet::new();

        let mut candidates: Vec<AppliedMatch> = evidence.matches.clone();
        candidates.sort_by_key(|m| match_priority(m.kind));

        for candidate in candidates {
            if replacement_count >= MAX_REPLACEMENTS_PER_RECORDING {
                break;
            }
            if !final_apply_kind(candidate.kind) {
                continue;
            }
            let key = normalize_core(&candidate.transcript_form);
            if key.is_empty() || applied_forms.contains(&key) {
                continue;
            }
            if let Some(next) =
                replace_surface_in_text(&text, &candidate.transcript_form, &candidate.correct_form)
            {
                if next != text {
                    info!(
                        "[tier2] final: {:?} → {:?} ({:?})",
                        candidate.transcript_form, candidate.correct_form, candidate.kind
                    );
                    text = next;
                    applied_forms.insert(key);
                    if let Some(trace) = trace_for_match(evidence, &candidate) {
                        traces.push(trace);
                    }
                    matches.push(candidate);
                    replacement_count += 1;
                    continue;
                }
            }
            if let Some(next) = position_aligned_replace(
                &text,
                raw_source,
                &candidate.transcript_form,
                &candidate.correct_form,
            ) {
                if next != text {
                    info!(
                        "[tier2] final aligned: {:?} → {:?} ({:?})",
                        candidate.transcript_form, candidate.correct_form, candidate.kind
                    );
                    text = next;
                    applied_forms.insert(key);
                    if let Some(trace) = trace_for_match(evidence, &candidate) {
                        traces.push(trace);
                    }
                    matches.push(candidate);
                    replacement_count += 1;
                }
            }
        }

        for trace in &evidence.traces {
            if replacement_count >= MAX_REPLACEMENTS_PER_RECORDING {
                break;
            }
            if !trace.applied || trace.policy_score.is_none() {
                continue;
            }
            let key = normalize_core(&trace.token);
            if key.is_empty() || applied_forms.contains(&key) {
                continue;
            }
            let candidate = AppliedMatch {
                transcript_form: trace.token.clone(),
                correct_form: trace.candidate.clone(),
                kind: trace.kind,
            };
            if let Some(next) =
                replace_surface_in_text(&text, &candidate.transcript_form, &candidate.correct_form)
            {
                if next != text {
                    text = next;
                    applied_forms.insert(key);
                    matches.push(candidate);
                    traces.push(trace.clone());
                    replacement_count += 1;
                }
            }
        }

        ApplyResult {
            text,
            matches,
            traces,
        }
    }
}

pub fn collect_evidence_with_store(
    pool: &DbPool,
    user_id: &str,
    transcript: &str,
    stt_replacements: &[SttReplacement],
    vocabulary: &[VocabTerm],
) -> EvidenceResult {
    let metadata = crate::store::tier2_model::get(pool, user_id);
    let policy_weights = crate::store::tier2_policy::load_weights(pool, user_id);
    let edit_policy_rules =
        crate::store::tier2_edit_policy::load_active_replace_rules(pool, user_id);
    CorrectionEngine::new_with_policy(
        stt_replacements,
        vocabulary,
        metadata,
        policy_weights,
        edit_policy_rules,
    )
    .collect_evidence(transcript)
}

pub fn final_resolve_with_store(
    pool: &DbPool,
    user_id: &str,
    polished: &str,
    evidence: &EvidenceResult,
    raw_source: &str,
    stt_replacements: &[SttReplacement],
    vocabulary: &[VocabTerm],
) -> ApplyResult {
    let metadata = crate::store::tier2_model::get(pool, user_id);
    let policy_weights = crate::store::tier2_policy::load_weights(pool, user_id);
    let edit_policy_rules =
        crate::store::tier2_edit_policy::load_active_replace_rules(pool, user_id);
    CorrectionEngine::new_with_policy(
        stt_replacements,
        vocabulary,
        metadata,
        policy_weights,
        edit_policy_rules,
    )
    .final_resolve(polished, evidence, raw_source)
}

pub fn correct_with_store(
    pool: &DbPool,
    user_id: &str,
    transcript: &str,
    stt_replacements: &[SttReplacement],
    vocabulary: &[VocabTerm],
) -> ApplyResult {
    let metadata = crate::store::tier2_model::get(pool, user_id);
    let policy_weights = crate::store::tier2_policy::load_weights(pool, user_id);
    let edit_policy_rules =
        crate::store::tier2_edit_policy::load_active_replace_rules(pool, user_id);
    let onnx_available = metadata
        .as_ref()
        .map(|m| std::path::Path::new(&m.artifact_path).is_file())
        .unwrap_or(false);
    info!(
        "[tier2] engine: vocab={} aliases={} edit_rules={} policy_pairs={} onnx={}",
        vocabulary.len(),
        stt_replacements.len(),
        edit_policy_rules.len(),
        policy_weights.len(),
        if onnx_available { "YES" } else { "no" },
    );
    CorrectionEngine::new_with_policy(
        stt_replacements,
        vocabulary,
        metadata,
        policy_weights,
        edit_policy_rules,
    )
    .correct(transcript)
}

pub fn status(pool: &DbPool, user_id: &str) -> Tier2Status {
    let metadata = crate::store::tier2_model::get(pool, user_id);
    let artifact_present = metadata
        .as_ref()
        .map(|m| Path::new(&m.artifact_path).is_file())
        .unwrap_or_else(|| crate::store::tier2_model::default_artifact_path(user_id).is_file());
    let onnx_loadable = metadata
        .as_ref()
        .filter(|_| artifact_present)
        .and_then(|m| load_onnx_model(m).ok())
        .is_some();
    let policy_status =
        tier2_policy::status(pool, user_id, metadata.as_ref().map(|m| m.trained_at));
    let edit_policy_status = tier2_edit_policy::status(pool, user_id);
    let pending_onnx_refresh = policy_status.rewards_since_onnx >= 20
        || edit_policy_status.explicit_positive_count >= 20
        || (!artifact_present
            && (policy_status.positive_reward_count >= 50
                || edit_policy_status.explicit_positive_count >= 50));
    let onnx_trust_level = if !onnx_loadable {
        "none"
    } else if policy_status.positive_reward_count < 20 {
        "cold"
    } else if policy_status.positive_reward_count < 100 {
        "assist"
    } else {
        "trusted_fuzzy"
    };

    Tier2Status {
        enabled: true,
        active_scorer: if onnx_loadable {
            ActiveScorer::Onnx
        } else {
            ActiveScorer::Deterministic
        },
        artifact_present,
        trained_at: metadata.as_ref().map(|m| m.trained_at),
        alias_count: metadata.as_ref().map(|m| m.alias_count).unwrap_or(0),
        vocab_count: metadata.as_ref().map(|m| m.vocab_count).unwrap_or(0),
        data_fingerprint: metadata.as_ref().and_then(|m| m.data_fingerprint.clone()),
        last_error: metadata.as_ref().and_then(|m| m.last_error.clone()),
        policy_pair_count: policy_status.policy_pair_count,
        positive_reward_count: policy_status.positive_reward_count,
        negative_reward_count: policy_status.negative_reward_count,
        pending_onnx_refresh,
        onnx_trust_level: onnx_trust_level.to_string(),
        last_policy_update_at: policy_status.last_policy_update_at,
        candidate_rule_count: edit_policy_status.candidate_rule_count,
        active_rule_count: edit_policy_status.active_rule_count,
        blocked_rule_count: edit_policy_status.blocked_rule_count,
        explicit_positive_count: edit_policy_status.explicit_positive_count,
        negative_count: edit_policy_status.negative_count,
        fuzzy_shadow_count: edit_policy_status.fuzzy_shadow_count,
    }
}

fn combine_results(mut exact: ApplyResult, tier2: ApplyResult) -> ApplyResult {
    exact.text = tier2.text;
    exact.matches.extend(tier2.matches);
    exact.traces.extend(tier2.traces);
    exact
}

fn build_candidates(
    stt_replacements: &[SttReplacement],
    vocabulary: &[VocabTerm],
) -> Vec<Candidate> {
    let mut by_term: HashMap<String, Candidate> = HashMap::new();

    for term in vocabulary {
        let source = term.source.trim();
        let term_type = term
            .term_type
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| vocabulary::classify_term_type(&term.term).to_string());
        if !is_protected_term(&term.term, &term_type, source) {
            continue;
        }
        if term.term.split_whitespace().count() != 1 {
            continue;
        }
        let term_lower = normalize_core(&term.term);
        if term_lower.len() < MIN_TOKEN_LEN {
            continue;
        }

        by_term
            .entry(term_lower.clone())
            .or_insert_with(|| Candidate {
                term: term.term.clone(),
                term_lower,
                term_type,
                source: source.to_string(),
                weight: term.weight,
                use_count: term.use_count,
                aliases: vec![],
            });
    }

    for rule in stt_replacements {
        if rule.review_status != ReviewStatus::Approved {
            continue;
        }
        if crate::llm::alias_safety::is_common_alias_source(&rule.transcript_form)
            || is_in_dictionary(&rule.transcript_form)
        {
            continue;
        }
        let key = normalize_core(&rule.correct_form);
        let Some(candidate) = by_term.get_mut(&key) else {
            continue;
        };
        let alias = normalize_core(&rule.transcript_form);
        if alias.len() < MIN_TOKEN_LEN || alias == candidate.term_lower {
            continue;
        }
        candidate.aliases.push(AliasEvidence {
            transcript_form: rule.transcript_form.clone(),
            weight: rule.weight,
            use_count: rule.use_count,
        });
    }

    by_term.into_values().collect()
}

fn is_protected_term(term: &str, term_type: &str, source: &str) -> bool {
    let source = source.trim();
    if source == "manual" || source == "starred" {
        return true;
    }
    if is_in_dictionary(term) {
        return false;
    }
    matches!(
        term_type,
        "brand" | "acronym" | "proper_noun" | "code_identifier"
    )
}

fn apply_edit_policy(
    transcript: &str,
    rules: &[Tier2EditPolicyRule],
    protected_tokens: &HashSet<String>,
    vocabulary: &[VocabTerm],
) -> ApplyResult {
    if rules.is_empty() {
        return ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
    }
    let known_terms: HashSet<String> = vocabulary
        .iter()
        .map(|term| normalize_core(&term.term))
        .filter(|term| !term.is_empty())
        .collect();
    let chunks = split_chunks(transcript);
    let cores: Vec<String> = chunks.iter().map(|chunk| normalize_core(chunk)).collect();
    let mut out = String::with_capacity(transcript.len());
    let mut matches = Vec::new();
    let mut traces = Vec::new();
    let mut replacement_count = 0usize;

    // ── Multi-token pre-pass ─────────────────────────────────────────────────
    // "auto note" → normalize_token gives "autonote" which matches a rule
    // variant_norm. We scan bigrams and trigrams BEFORE the per-token loop
    // so multi-word STT outputs like "auto note" get merged into "AutoNote".
    let mut consumed: HashSet<usize> = HashSet::new();
    let active_rules: Vec<&Tier2EditPolicyRule> = rules
        .iter()
        .filter(|r| r.status == "active" && r.edit_type == "replace")
        .collect();

    // Check trigrams first (longest match wins), then bigrams
    for window_size in [3usize, 2] {
        if cores.len() < window_size {
            continue;
        }
        for start in 0..=(cores.len() - window_size) {
            if replacement_count >= MAX_REPLACEMENTS_PER_RECORDING {
                break;
            }
            if (start..start + window_size).any(|i| consumed.contains(&i)) {
                continue;
            }
            // Concatenate adjacent token norms (same as normalize_token does with spaces)
            let merged_norm: String = cores[start..start + window_size]
                .iter()
                .flat_map(|s| s.chars())
                .collect();
            if merged_norm.is_empty() {
                continue;
            }
            // Any single core alone would also match — skip to avoid double-matching
            if window_size == 2 && cores[start] == merged_norm {
                continue;
            }

            let span_norm = cores[start..start + window_size].join(" ");
            if crate::llm::alias_safety::is_common_alias_source_norm(&span_norm)
                || is_in_dictionary(&span_norm)
            {
                continue;
            }

            let best_rule = active_rules
                .iter()
                .filter(|r| {
                    r.variant_norm == merged_norm
                        || (r.variant_norm.len() >= 6
                            && edit_similarity(&r.variant_norm, &merged_norm) >= 0.90)
                })
                .max_by_key(|r| r.positive_count - (r.negative_count * 2));
            let Some(rule) = best_rule else {
                continue;
            };

            let span_label: String = (start..start + window_size)
                .map(|i| word_core(chunks[i]).to_string())
                .collect::<Vec<_>>()
                .join(" ");
            info!(
                "[tier2] multi-token edit-policy: {:?} ({} tokens) → {:?}",
                span_label, window_size, rule.correct_form,
            );

            // Mark intermediate chunks as consumed (they'll be skipped)
            for i in start..start + window_size {
                consumed.insert(i);
            }
            // We build the output later in the main loop using `consumed` +
            // a replacement map.
            // For now, store the replacement info.
            matches.push(AppliedMatch {
                transcript_form: span_label,
                correct_form: rule.correct_form.clone(),
                kind: MatchKind::Tier2EditPolicy,
            });
            replacement_count += 1;

            // Store the assembled replacement text for the first chunk of the span
            // so the main loop can emit it.
            // We'll use a separate map for this.
            // Actually, let's build the output directly here by collecting
            // multi-token replacements into a map: start_idx → replacement string.
            // We'll do this below after collecting all multi-token matches.
        }
    }

    // Build a map of multi-token replacement strings keyed by start index
    // Re-scan the same way to build output strings (this is cheap, rules are few)
    let mut multi_replacements: HashMap<usize, (String, usize)> = HashMap::new();
    // Re-derive from matches we just pushed — rebuild so we have the text
    {
        let mut mr_count = 0usize;
        for window_size in [3usize, 2] {
            if cores.len() < window_size {
                continue;
            }
            for start in 0..=(cores.len() - window_size) {
                if !consumed.contains(&start) {
                    continue;
                }
                // Check if this start is the beginning of a multi-token span
                let all_consumed = (start..start + window_size).all(|i| consumed.contains(&i));
                if !all_consumed {
                    continue;
                }
                // Avoid double-processing: only process if start-1 is NOT consumed
                // (meaning this is the real start of a span, not the middle)
                if start > 0 && consumed.contains(&(start - 1)) {
                    continue;
                }
                let merged_norm: String = cores[start..start + window_size]
                    .iter()
                    .flat_map(|s| s.chars())
                    .collect();
                let span_norm = cores[start..start + window_size].join(" ");
                if crate::llm::alias_safety::is_common_alias_source_norm(&span_norm)
                    || is_in_dictionary(&span_norm)
                {
                    continue;
                }

                let best_rule = active_rules
                    .iter()
                    .filter(|r| {
                        r.variant_norm == merged_norm
                            || (r.variant_norm.len() >= 6
                                && edit_similarity(&r.variant_norm, &merged_norm) >= 0.90)
                    })
                    .max_by_key(|r| r.positive_count - (r.negative_count * 2));
                if let Some(rule) = best_rule {
                    let (lead, _) = split_punct(chunks[start]);
                    let last_chunk = chunks[start + window_size - 1];
                    let (_, trail) = split_punct(last_chunk);
                    let (_, trail2) = split_punct_trailing(trail);
                    let replacement = format!("{lead}{}{trail2}", rule.correct_form);
                    multi_replacements.insert(start, (replacement, window_size));
                    mr_count += 1;
                }
                if mr_count >= MAX_REPLACEMENTS_PER_RECORDING {
                    break;
                }
            }
        }
    }

    // ── Per-token loop ───────────────────────────────────────────────────────
    let mut idx = 0;
    while idx < chunks.len() {
        // Check if this position starts a multi-token replacement
        if let Some((replacement, span_len)) = multi_replacements.get(&idx) {
            out.push_str(replacement);
            idx += span_len;
            continue;
        }
        // Skip consumed positions (middle/end of multi-token spans)
        if consumed.contains(&idx) {
            idx += 1;
            continue;
        }

        let chunk = chunks[idx];
        if replacement_count >= MAX_REPLACEMENTS_PER_RECORDING {
            out.push_str(chunk);
            idx += 1;
            continue;
        }
        let core = word_core(chunk);
        let core_norm = &cores[idx];
        if core_norm.is_empty()
            || protected_tokens.contains(core_norm)
            || known_terms.contains(core_norm)
            || crate::llm::alias_safety::is_common_alias_source(core_norm)
            || is_in_dictionary(core_norm)
        {
            out.push_str(chunk);
            idx += 1;
            continue;
        }

        let Some(rule) = rules
            .iter()
            .filter(|rule| rule.variant_norm == *core_norm)
            .filter(|rule| rule.status == "active" && rule.edit_type == "replace")
            .max_by(|a, b| {
                let a_score = a.positive_count - (a.negative_count * 2);
                let b_score = b.positive_count - (b.negative_count * 2);
                a_score.cmp(&b_score)
            })
        else {
            out.push_str(chunk);
            idx += 1;
            continue;
        };

        let requires_context = edit_policy_requires_context(rule);
        let context_ok = !requires_context || edit_policy_context_matches(&cores, idx, rule);
        let score = edit_policy_score(rule, context_ok);
        traces.push(ScoringTrace {
            token: core.to_string(),
            candidate: rule.correct_form.clone(),
            second_candidate: None,
            kind: MatchKind::Tier2EditPolicy,
            score,
            second_score: 0.0,
            margin: score,
            deterministic_score: 0.0,
            onnx_score: None,
            policy_score: Some(score),
            policy_boost: score,
            applied: context_ok,
        });
        if !context_ok {
            out.push_str(chunk);
            idx += 1;
            continue;
        }

        let (lead, trail) = split_punct(chunk);
        let (_, trail2) = split_punct_trailing(trail);
        out.push_str(lead);
        out.push_str(&rule.correct_form);
        out.push_str(trail2);
        matches.push(AppliedMatch {
            transcript_form: core.to_string(),
            correct_form: rule.correct_form.clone(),
            kind: MatchKind::Tier2EditPolicy,
        });
        replacement_count += 1;
        idx += 1;
    }

    ApplyResult {
        text: out,
        matches,
        traces,
    }
}

fn edit_policy_requires_context(rule: &Tier2EditPolicyRule) -> bool {
    if edit_policy_context_word(&rule.variant_norm) {
        return true;
    }
    if pair_similarity(&rule.variant_norm, &rule.correct_form) >= 0.72 {
        return false;
    }
    rule.variant_norm.chars().all(|c| c.is_ascii_alphabetic())
}

fn edit_policy_context_word(norm: &str) -> bool {
    const CONTEXT_REQUIRED: &[&str] = &[
        "account", "accounts", "base", "capital", "cancer", "cool", "corp", "corps", "course",
        "dock", "git", "go", "google", "graph", "guard", "house", "just", "local", "meaning",
        "nest", "next", "oath", "post", "prayer", "press", "prism", "rbi", "react", "red", "rest",
        "return", "sent", "super", "supply", "swift", "table", "tail", "tell", "think", "tower",
        "type", "verse", "white",
    ];
    CONTEXT_REQUIRED.contains(&norm)
}

fn edit_policy_context_matches(cores: &[String], idx: usize, rule: &Tier2EditPolicyRule) -> bool {
    let left = nearby_left(cores, idx);
    let right = nearby_right(cores, idx);
    let left_match = rule
        .left_context
        .iter()
        .any(|token| context_token_matches(token, &left));
    let right_match = rule
        .right_context
        .iter()
        .any(|token| context_token_matches(token, &right));
    if left_match || right_match {
        return true;
    }
    hinglish_particle_nearby(&left, &right)
}

fn hinglish_particle_nearby(left: &[&str], right: &[&str]) -> bool {
    const PARTICLES: &[&str] = &[
        "ka", "ki", "ke", "ko", "mein", "mai", "hai", "hain", "par", "pe", "se", "ne", "bhi",
        "nahi", "nhi", "aur", "ya", "thi", "tha", "ho", "wala", "wali", "wale", "kya", "kaisa",
        "kaisi", "kaise", "kaha", "yeh", "ye", "voh", "vo", "ek",
    ];
    left.iter()
        .chain(right.iter())
        .any(|token| PARTICLES.contains(token))
}

fn nearby_left(cores: &[String], idx: usize) -> Vec<&str> {
    cores[..idx]
        .iter()
        .rev()
        .filter(|token| !token.is_empty())
        .take(3)
        .map(String::as_str)
        .collect()
}

fn nearby_right(cores: &[String], idx: usize) -> Vec<&str> {
    cores[idx + 1..]
        .iter()
        .filter(|token| !token.is_empty())
        .take(3)
        .map(String::as_str)
        .collect()
}

fn context_token_matches(token: &str, nearby: &[&str]) -> bool {
    let token_norm = tier2_edit_policy::normalize_token(token);
    !token_norm.is_empty()
        && nearby.iter().any(|near| {
            let near_norm = tier2_edit_policy::normalize_token(near);
            near_norm == token_norm || (token_norm.len() >= 4 && near_norm.contains(&token_norm))
        })
}

fn edit_policy_score(rule: &Tier2EditPolicyRule, context_ok: bool) -> f64 {
    let base = 0.91 + (rule.positive_count.max(0) as f64 * 0.025);
    let penalty = rule.negative_count.max(0) as f64 * 0.08;
    let context_penalty = if context_ok { 0.0 } else { 0.25 };
    (base - penalty - context_penalty).clamp(0.0, 0.99)
}

/// Cluster-fuzzy pass: for tokens not yet corrected, check fuzzy similarity
/// against targets that have at least one active edit-policy rule. This catches
/// new STT distortions of known terms (e.g. "cubernetize" for Kubernetes) without
/// the old problem of replacing random words with arbitrary vocab.
fn cluster_fuzzy_pass(
    transcript: &str,
    edit_policy_rules: &[Tier2EditPolicyRule],
    protected_tokens: &HashSet<String>,
    vocabulary: &[VocabTerm],
    stt_replacements: &[SttReplacement],
) -> ApplyResult {
    let active_targets: HashMap<String, &str> = edit_policy_rules
        .iter()
        .filter(|r| r.status == "active" && r.edit_type == "replace")
        .map(|r| (r.correct_form_norm.clone(), r.correct_form.as_str()))
        .collect();
    if active_targets.is_empty() {
        return ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
    }

    let known_vocab: HashSet<String> = vocabulary
        .iter()
        .map(|v| normalize_core(&v.term))
        .filter(|s| !s.is_empty())
        .collect();

    let target_candidates: Vec<Candidate> = {
        let all = build_candidates(stt_replacements, vocabulary);
        all.into_iter()
            .filter(|c| active_targets.contains_key(&c.term_lower))
            .collect()
    };
    if target_candidates.is_empty() {
        return ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        };
    }

    let chunks = split_chunks(transcript);
    let mut out = String::with_capacity(transcript.len());
    let mut matches = Vec::new();
    let mut traces = Vec::new();
    let mut replacement_count = 0usize;

    for chunk in &chunks {
        if replacement_count >= MAX_REPLACEMENTS_PER_RECORDING {
            out.push_str(chunk);
            continue;
        }
        let core = word_core(chunk);
        let core_norm = normalize_core(core);
        if core_norm.is_empty()
            || core_norm.chars().count() < MIN_TOKEN_LEN
            || protected_tokens.contains(&core_norm)
            || known_vocab.contains(&core_norm)
            || crate::llm::alias_safety::is_common_alias_source(&core_norm)
            || is_in_dictionary(&core_norm)
            || edit_policy_context_word(&core_norm)
        {
            out.push_str(chunk);
            continue;
        }
        if core_norm.chars().all(|c| c.is_ascii_digit()) {
            out.push_str(chunk);
            continue;
        }

        let mut best_score = 0.0_f64;
        let mut best_candidate: Option<&Candidate> = None;
        for candidate in &target_candidates {
            if candidate.term_lower == core_norm {
                best_candidate = None;
                break;
            }
            let score = cluster_fuzzy_score(&core_norm, candidate);
            if score > best_score {
                best_score = score;
                best_candidate = Some(candidate);
            }
        }

        let Some(candidate) = best_candidate else {
            out.push_str(chunk);
            continue;
        };

        let applied = best_score >= CLUSTER_FUZZY_THRESHOLD;
        traces.push(ScoringTrace {
            token: core.to_string(),
            candidate: candidate.term.clone(),
            second_candidate: None,
            kind: MatchKind::Tier2ClusterFuzzy,
            score: best_score,
            second_score: 0.0,
            margin: best_score,
            deterministic_score: best_score,
            onnx_score: None,
            policy_score: None,
            policy_boost: 0.0,
            applied,
        });

        if !applied {
            out.push_str(chunk);
            continue;
        }

        let (lead, trail) = split_punct(chunk);
        let (_, trail2) = split_punct_trailing(trail);
        let surface = active_targets
            .get(&candidate.term_lower)
            .copied()
            .unwrap_or(candidate.term.as_str());
        out.push_str(lead);
        out.push_str(surface);
        out.push_str(trail2);
        matches.push(AppliedMatch {
            transcript_form: core.to_string(),
            correct_form: surface.to_string(),
            kind: MatchKind::Tier2ClusterFuzzy,
        });
        replacement_count += 1;
    }

    ApplyResult {
        text: out,
        matches,
        traces,
    }
}

fn cluster_fuzzy_score(token_norm: &str, candidate: &Candidate) -> f64 {
    let mut best = pair_similarity(token_norm, &candidate.term);
    for alias in &candidate.aliases {
        best = best.max(pair_similarity(token_norm, &alias.transcript_form));
    }
    let type_boost = match candidate.term_type.as_str() {
        "acronym" | "code_identifier" => 0.04,
        "brand" | "proper_noun" => 0.02,
        _ => 0.0,
    };
    (best + type_boost).min(1.0)
}

fn apply_tier2(
    transcript: &str,
    candidates: &[Candidate],
    onnx_model: Option<&Arc<OnnxModel>>,
    protected_tokens: &HashSet<String>,
    policy_weights: &HashMap<PolicyKey, Tier2PolicyWeight>,
    allow_apply: bool,
) -> Result<ApplyResult, String> {
    let chunks = split_chunks(transcript);
    if chunks.is_empty() {
        return Ok(ApplyResult {
            text: transcript.to_string(),
            matches: vec![],
            traces: vec![],
        });
    }

    let mut out = String::with_capacity(transcript.len());
    let mut matches = Vec::new();
    let mut traces = Vec::new();
    let mut replacement_count = 0usize;
    let known_candidate_terms: HashSet<&str> = candidates
        .iter()
        .map(|candidate| candidate.term_lower.as_str())
        .collect();

    for chunk in chunks {
        if replacement_count >= MAX_REPLACEMENTS_PER_RECORDING {
            out.push_str(chunk);
            continue;
        }

        let core = word_core(chunk);
        let core_norm = normalize_core(core);
        if core_norm.is_empty() || core_norm.chars().count() < MIN_TOKEN_LEN {
            out.push_str(chunk);
            continue;
        }
        if protected_tokens.contains(&core_norm) {
            debug!("[tier2-score] skip {:?}: already corrected", core);
            out.push_str(chunk);
            continue;
        }
        if known_candidate_terms.contains(core_norm.as_str()) {
            debug!("[tier2-score] skip {:?}: is a vocab term itself", core);
            out.push_str(chunk);
            continue;
        }
        if !is_suspicious_token(core) {
            debug!(
                "[tier2-score] skip {:?}: not suspicious (common/short/digit)",
                core
            );
            out.push_str(chunk);
            continue;
        }

        let Some(decision) = choose_candidate(core, candidates, onnx_model, policy_weights)? else {
            debug!("[tier2-score] skip {:?}: no candidate matched", core);
            out.push_str(chunk);
            continue;
        };
        if should_record_trace(&decision) {
            traces.push(scoring_trace(core, &decision, allow_apply));
        }

        if !allow_apply || !decision.applied {
            out.push_str(chunk);
            continue;
        }

        let best = decision.best;

        let (lead, trail) = split_punct(chunk);
        let (_, trail2) = split_punct_trailing(trail);
        out.push_str(lead);
        out.push_str(&best.candidate.term);
        out.push_str(trail2);
        matches.push(AppliedMatch {
            transcript_form: core.to_string(),
            correct_form: best.candidate.term,
            kind: best.kind,
        });
        replacement_count += 1;
    }

    Ok(ApplyResult {
        text: out,
        matches,
        traces,
    })
}

fn choose_candidate(
    token: &str,
    candidates: &[Candidate],
    onnx_model: Option<&Arc<OnnxModel>>,
    policy_weights: &HashMap<PolicyKey, Tier2PolicyWeight>,
) -> Result<Option<ScoringDecision>, String> {
    let token_norm = normalize_core(token);
    if token_norm.is_empty() {
        return Ok(None);
    }

    let mut scored = Vec::new();
    for candidate in candidates {
        if candidate.term_lower == token_norm {
            continue;
        }
        if !candidate_can_match_token(&token_norm, candidate) {
            continue;
        }
        let deterministic_score = deterministic_score(&token_norm, candidate);
        let onnx_score = if let Some(model) = onnx_model {
            let raw = model.score(&token_norm, candidate, deterministic_score)?;
            let edit_sim = edit_similarity(&token_norm, &candidate.term_lower);
            let phon_sim = phonetics::similarity(&token_norm, &candidate.term_lower);
            if raw > 0.8 && (edit_sim < 0.45 || phon_sim < 0.35) {
                Some(0.0)
            } else {
                Some(raw)
            }
        } else {
            None
        };
        if onnx_score.is_some_and(|score| {
            score < 0.35
                && (crate::llm::alias_safety::is_common_alias_source(token)
                    || is_in_dictionary(&token_norm))
        }) {
            continue;
        }
        let mut score = onnx_score
            .map(|onnx| deterministic_score.max((onnx * 0.65) + (deterministic_score * 0.35)))
            .unwrap_or(deterministic_score);
        let mut kind = if onnx_score.is_some() && score > deterministic_score {
            MatchKind::Tier2Onnx
        } else {
            MatchKind::Tier2Deterministic
        };
        let policy = policy_weights.get(&(token_norm.clone(), candidate.term_lower.clone()));
        let policy_score = policy.and_then(tier2_policy::policy_score);
        let mut policy_boost = 0.0;
        if let Some(policy_score_value) = policy_score {
            if policy_score_value > 0.0 {
                policy_boost = (policy_score_value - deterministic_score).max(0.0);
                score = score.max(policy_score_value);
                kind = MatchKind::Tier2Policy;
            }
        }
        scored.push(CandidateScore {
            candidate: candidate.clone(),
            score,
            kind,
            deterministic_score,
            onnx_score,
            policy_score,
            policy_boost,
        });
    }
    if scored.is_empty() {
        return Ok(None);
    }

    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    let best = scored[0].clone();
    let second = scored.get(1).cloned();
    let second_score = second.as_ref().map(|s| s.score).unwrap_or(0.0);
    let margin = best.score - second_score;
    let has_direct_policy_evidence = best.policy_score.is_some();
    Ok(Some(ScoringDecision {
        best,
        second,
        margin,
        applied: has_direct_policy_evidence
            && margin >= MARGIN_THRESHOLD
            && scored[0].score >= SCORE_THRESHOLD
            && scored[0].deterministic_score >= DETERMINISTIC_FLOOR,
    }))
}

fn scoring_trace(token: &str, decision: &ScoringDecision, allow_apply: bool) -> ScoringTrace {
    ScoringTrace {
        token: token.to_string(),
        candidate: decision.best.candidate.term.clone(),
        second_candidate: decision.second.as_ref().map(|s| s.candidate.term.clone()),
        kind: decision.best.kind,
        score: decision.best.score,
        second_score: decision.second.as_ref().map(|s| s.score).unwrap_or(0.0),
        margin: decision.margin,
        deterministic_score: decision.best.deterministic_score,
        onnx_score: decision.best.onnx_score,
        policy_score: decision.best.policy_score,
        policy_boost: decision.best.policy_boost,
        applied: allow_apply && decision.applied,
    }
}

fn should_record_trace(decision: &ScoringDecision) -> bool {
    decision.applied
        || decision.best.policy_score.is_some()
        || decision.best.score >= TRACE_REJECTED_SCORE_FLOOR
}

fn candidate_can_match_token(token_norm: &str, candidate: &Candidate) -> bool {
    if !candidate.aliases.is_empty() {
        return true;
    }
    let token_len = token_norm.chars().count();
    let candidate_len = candidate.term_lower.chars().count();
    if candidate_len <= 3 && token_len > candidate_len + 1 {
        return false;
    }
    if candidate.term_type == "acronym" && token_len > candidate_len + 2 {
        return false;
    }
    true
}

fn deterministic_score(token_norm: &str, candidate: &Candidate) -> f64 {
    let mut best = pair_similarity(token_norm, &candidate.term);
    for alias in &candidate.aliases {
        best = best.max(pair_similarity(token_norm, &alias.transcript_form));
    }

    let type_boost = match candidate.term_type.as_str() {
        "acronym" | "code_identifier" => 0.07,
        "brand" | "proper_noun" => 0.05,
        _ => 0.0,
    };
    let source_boost = match candidate.source.as_str() {
        "starred" => 0.07,
        "manual" => 0.05,
        _ => 0.0,
    };
    let evidence = ((candidate.use_count.max(0) as f64).ln_1p() * 0.015)
        + ((candidate.weight.max(0.0) - 1.0).max(0.0) * 0.015);
    let alias_evidence = candidate
        .aliases
        .iter()
        .map(|a| (a.use_count.max(0) as f64).ln_1p() * 0.012 + a.weight.max(0.0) * 0.008)
        .sum::<f64>()
        .min(0.08);

    (best + type_boost + source_boost + evidence.min(0.06) + alias_evidence).min(1.0)
}

fn pair_similarity(token_norm: &str, surface: &str) -> f64 {
    let surface_norm = normalize_core(surface);
    if surface_norm.is_empty() {
        return 0.0;
    }
    if token_norm == surface_norm {
        return 1.0;
    }
    let edit = edit_similarity(token_norm, &surface_norm);
    let phon = phonetics::similarity(token_norm, &surface_norm);
    let mut score = edit.max(phon);
    if edit >= 0.70 && token_norm.len().abs_diff(surface_norm.len()) <= 2 {
        score += 0.08;
    }
    if phon >= 0.65 {
        score += 0.06;
    }
    if first_char(token_norm) == first_char(&surface_norm) {
        score += 0.03;
    }
    score.min(1.0)
}

fn is_suspicious_token(core: &str) -> bool {
    let norm = normalize_core(core);
    if norm.chars().count() < MIN_TOKEN_LEN {
        return false;
    }
    if crate::llm::alias_safety::is_common_alias_source(core)
        || crate::llm::alias_safety::is_common_alias_source(&norm)
    {
        return false;
    }
    if is_in_dictionary(&norm) {
        return false;
    }
    if norm.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    norm.chars()
        .any(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn token_index_of(text: &str, form: &str) -> Option<usize> {
    let target = normalize_core(form);
    if target.is_empty() {
        return None;
    }
    let chunks = split_chunks(text);
    let cores: Vec<String> = chunks.iter().map(|chunk| normalize_core(chunk)).collect();
    for (idx, core) in cores.iter().enumerate() {
        if *core == target {
            return Some(idx);
        }
    }
    let form_word_count = form.split_whitespace().count();
    if form_word_count >= 2 {
        for window in 2..=form_word_count.min(4) {
            if cores.len() < window {
                continue;
            }
            for start in 0..=(cores.len() - window) {
                let merged = cores[start..start + window].join(" ");
                let compact: String = cores[start..start + window]
                    .iter()
                    .flat_map(|s| s.chars())
                    .collect();
                if merged == target || compact == target {
                    return Some(start);
                }
            }
        }
    }
    None
}

fn match_priority(kind: MatchKind) -> usize {
    match kind {
        MatchKind::Exact => 0,
        MatchKind::Tier2EditPolicy => 1,
        MatchKind::Tier2ClusterFuzzy => 2,
        MatchKind::Tier2Policy => 3,
        MatchKind::Phonetic => 4,
        MatchKind::Tier2Onnx | MatchKind::Tier2Deterministic => 9,
    }
}

fn final_apply_kind(kind: MatchKind) -> bool {
    matches!(
        kind,
        MatchKind::Exact
            | MatchKind::Tier2EditPolicy
            | MatchKind::Tier2ClusterFuzzy
            | MatchKind::Tier2Policy
    )
}

fn trace_for_match(evidence: &EvidenceResult, candidate: &AppliedMatch) -> Option<ScoringTrace> {
    evidence
        .traces
        .iter()
        .find(|trace| {
            trace.kind == candidate.kind
                && normalize_core(&trace.token) == normalize_core(&candidate.transcript_form)
                && normalize_core(&trace.candidate) == normalize_core(&candidate.correct_form)
        })
        .cloned()
        .or_else(|| {
            if candidate.kind == MatchKind::Exact {
                None
            } else {
                Some(ScoringTrace {
                    token: candidate.transcript_form.clone(),
                    candidate: candidate.correct_form.clone(),
                    second_candidate: None,
                    kind: candidate.kind,
                    score: 1.0,
                    second_score: 0.0,
                    margin: 1.0,
                    deterministic_score: 1.0,
                    onnx_score: None,
                    policy_score: Some(1.0),
                    policy_boost: 1.0,
                    applied: true,
                })
            }
        })
}

fn replace_surface_in_text(text: &str, from: &str, to: &str) -> Option<String> {
    let from = from.trim();
    let to = to.trim();
    if from.is_empty() || to.is_empty() || normalize_core(from) == normalize_core(to) {
        return None;
    }
    let lower_text = text.to_ascii_lowercase();
    let lower_from = from.to_ascii_lowercase();
    let mut search_start = 0usize;
    while let Some(rel) = lower_text[search_start..].find(&lower_from) {
        let start = search_start + rel;
        let end = start + lower_from.len();
        if start > text.len()
            || end > text.len()
            || !text.is_char_boundary(start)
            || !text.is_char_boundary(end)
        {
            search_start = end.min(text.len());
            continue;
        }
        if is_word_boundary(text, start, end) {
            let mut out = String::with_capacity(text.len() + to.len());
            out.push_str(&text[..start]);
            out.push_str(to);
            out.push_str(&text[end..]);
            return Some(out);
        }
        search_start = end.min(text.len());
    }
    None
}

fn position_aligned_replace(text: &str, raw_source: &str, from: &str, to: &str) -> Option<String> {
    let raw_idx = token_index_of(raw_source, from)?;
    let raw_chunks = split_chunks(raw_source);
    let out_chunks = split_chunks(text);
    if raw_chunks.is_empty()
        || out_chunks.is_empty()
        || raw_chunks.len().abs_diff(out_chunks.len()) > 1
        || raw_idx >= out_chunks.len()
    {
        return None;
    }
    let target = normalize_core(from);
    let out_core = normalize_core(out_chunks[raw_idx]);
    if out_core.is_empty()
        || crate::llm::alias_safety::is_common_alias_source(&out_core)
        || is_in_dictionary(&out_core)
    {
        return None;
    }
    if out_core != target && edit_similarity(&out_core, &target) < 0.92 {
        return None;
    }
    let mut out = String::new();
    for (idx, chunk) in out_chunks.iter().enumerate() {
        if idx == raw_idx {
            let (lead, trail) = split_punct(chunk);
            let (_, trail2) = split_punct_trailing(trail);
            out.push_str(lead);
            out.push_str(to);
            out.push_str(trail2);
        } else {
            out.push_str(chunk);
        }
    }
    if out == text { None } else { Some(out) }
}

fn is_word_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    !before.map(is_word_char).unwrap_or(false) && !after.map(is_word_char).unwrap_or(false)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

fn normalize_core(text: &str) -> String {
    let core = word_core(text);
    if crate::llm::script::contains_devanagari(core) {
        let romanized = crate::llm::script::romanize_devanagari(core);
        collapse_long_vowels(&romanized.to_ascii_lowercase())
    } else {
        core.to_ascii_lowercase()
    }
}

/// Collapse doubled vowels from Devanagari romanization into Hinglish short
/// forms so "kaa" matches "ka", "kee" matches "ki", "koo" matches "ku".
fn collapse_long_vowels(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        if i + 1 < bytes.len() && bytes[i + 1] == bytes[i] && b"aeiou".contains(&bytes[i]) {
            match ch {
                'e' => out.push('i'),
                'o' => out.push('u'),
                _ => out.push(ch),
            }
            i += 2;
        } else {
            out.push(ch);
            i += 1;
        }
    }
    out
}

fn first_char(text: &str) -> Option<char> {
    text.chars().next().map(|c| c.to_ascii_lowercase())
}

fn edit_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let max_len = a_chars.len().max(b_chars.len());
    if max_len == 0 {
        return 1.0;
    }
    let d = levenshtein_chars(&a_chars, &b_chars) as f64;
    (1.0 - d / max_len as f64).max(0.0)
}

fn levenshtein_chars(a: &[char], b: &[char]) -> usize {
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0; m + 1];
    for i in 1..=n {
        curr[0] = i;
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (curr[j - 1] + 1).min(prev[j] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[m]
}

fn split_chunks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        out.push(&text[start..i]);
        start = i;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn truncate_utf8_safe(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn word_core(chunk: &str) -> &str {
    let trimmed = chunk.trim();
    let leading = trimmed
        .find(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .unwrap_or(trimmed.len());
    let trailing = trimmed
        .rfind(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .map(|i| i + trimmed[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(trimmed.len());
    if leading >= trailing {
        return "";
    }
    &trimmed[leading..trailing]
}

fn split_punct(chunk: &str) -> (&str, &str) {
    let trimmed_start = chunk
        .find(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .unwrap_or(chunk.len());
    (&chunk[..trimmed_start], &chunk[trimmed_start..])
}

fn split_punct_trailing(chunk: &str) -> (&str, &str) {
    let trailing = chunk
        .rfind(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .map(|i| i + chunk[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(0);
    (&chunk[..trailing], &chunk[trailing..])
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OnnxCacheKey {
    artifact_path: PathBuf,
    vocab_index_path: PathBuf,
    artifact_mtime_ms: u128,
    vocab_index_mtime_ms: u128,
    data_fingerprint: Option<String>,
}

struct OnnxCacheEntry {
    key: OnnxCacheKey,
    model: Arc<OnnxModel>,
}

struct OnnxModel {
    session: Mutex<ort::session::Session>,
    vocab_index: VocabIndex,
}

#[derive(Debug, Clone, Deserialize)]
struct VocabIndex {
    #[serde(default = "default_max_len")]
    max_len: usize,
    #[serde(default)]
    char_to_id: HashMap<String, i64>,
    #[serde(default = "default_feature_names")]
    feature_names: Vec<String>,
    #[serde(default)]
    model_type: Option<String>,
}

impl VocabIndex {
    fn is_tree_model(&self) -> bool {
        self.model_type
            .as_deref()
            .is_some_and(|t| t == "xgboost" || t == "tree")
    }
}

static ONNX_CACHE: Lazy<Mutex<Option<OnnxCacheEntry>>> = Lazy::new(|| Mutex::new(None));
static DICTIONARY: Lazy<HashSet<String>> = Lazy::new(|| {
    let mut words = HashSet::new();
    if let Ok(content) = fs::read_to_string("/usr/share/dict/words") {
        for line in content.lines() {
            let w = line.trim().to_ascii_lowercase();
            if w.len() >= 3 && w.len() <= 20 {
                words.insert(w);
            }
        }
    }
    for w in &[
        "dock", "oath", "white", "just", "verse", "tower", "guard", "cool", "press", "super",
        "base", "graph", "local", "red", "post", "type", "strip", "fire", "cell", "hub", "lab",
        "prism", "meaning", "react", "swift", "rust", "dart", "flask", "spring", "nest", "corps",
        "main", "time", "return", "course", "prayer", "house", "table", "next", "rest", "google",
        "accounts", // Plurals / verb forms that /usr/share/dict/words may lack
        "cubes", "nodes", "tests", "builds", "stacks", "queues", "routes", "hooks", "props",
        "pipes", "shells", "caches", "agents", "proxies", "hosts", "ports",
        // Dev-adjacent real English words the system dictionary includes but
        // we list explicitly so bundled-dictionary builds (Windows) stay safe
        "slack", "stripe", "sentry", "cursor", "notion", "oracle", "consul", "vault", "helm",
        "drone", "spark", "hive", "celery", "rabbit", "harbor", "nomad", "beam", "arrow", "lance",
        "slate", "mint", "asana", "mongo", "flak", "agent", "proxy", "route", "cache", "queue",
        "stack", "heap", "pipe", "shell", "core", "flux", "hook", "arch", "brew",
    ] {
        words.insert(w.to_string());
    }
    // Hindi common words — base forms that promotion_gate may miss
    for w in &[
        "kar", "de", "le", "ja", "aa", "ho", "lo", "do", "so", "ro", "pa", "bol", "sun", "chal",
        "mil", "rakh", "ban", "baith", "uth", "khol", "rok", "din", "raat", "ghar", "log", "banda",
        "cheez", "jagah", "waqt", "baar", "baat", "kaam", "aaj", "kal", "aajkal", "ajkal", "parso",
        "subah", "shaam", "dopahar", "hamesha", "kabhi", "tarah", "saath", "andar", "bahar",
        "upar", "neeche", "peeche", "saamne",
        // Hinglish adjectives/adverbs that Deepgram transcribes and are close to brand names
        "kaafi", "kafi", "sahi", "galat", "accha", "pura", "poora",
    ] {
        words.insert(w.to_string());
    }
    words
});

pub fn is_in_dictionary(token: &str) -> bool {
    let norm = token
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect::<String>();
    crate::llm::alias_safety::is_common_alias_source(&norm)
        || DICTIONARY.contains(&norm)
        || promotion_gate::is_common_word(&norm)
}

fn default_max_len() -> usize {
    DEFAULT_MAX_LEN
}

fn default_feature_names() -> Vec<String> {
    vec![
        "edit_similarity".to_string(),
        "phonetic_similarity".to_string(),
        "length_ratio".to_string(),
        "term_type_acronym".to_string(),
        "term_type_brand".to_string(),
        "term_type_proper_noun".to_string(),
        "term_type_code_identifier".to_string(),
        "deterministic_score".to_string(),
    ]
}

fn load_onnx_model(metadata: &Tier2ModelMetadata) -> Result<Arc<OnnxModel>, String> {
    let artifact_path = PathBuf::from(&metadata.artifact_path);
    let vocab_index_path = PathBuf::from(&metadata.vocab_index_path);
    if !artifact_path.is_file() {
        return Err(format!("artifact missing: {}", artifact_path.display()));
    }
    if !vocab_index_path.is_file() {
        return Err(format!(
            "vocab index missing: {}",
            vocab_index_path.display()
        ));
    }

    let key = OnnxCacheKey {
        artifact_mtime_ms: mtime_ms(&artifact_path)?,
        vocab_index_mtime_ms: mtime_ms(&vocab_index_path)?,
        artifact_path: artifact_path.clone(),
        vocab_index_path: vocab_index_path.clone(),
        data_fingerprint: metadata.data_fingerprint.clone(),
    };

    {
        let guard = ONNX_CACHE
            .lock()
            .map_err(|_| "onnx cache lock poisoned".to_string())?;
        if let Some(entry) = guard.as_ref() {
            if entry.key == key {
                return Ok(entry.model.clone());
            }
        }
    }

    let index_text = fs::read_to_string(&vocab_index_path)
        .map_err(|e| format!("read vocab index failed: {e}"))?;
    let mut vocab_index: VocabIndex =
        serde_json::from_str(&index_text).map_err(|e| format!("parse vocab index failed: {e}"))?;
    if vocab_index.max_len == 0 {
        vocab_index.max_len = DEFAULT_MAX_LEN;
    }
    if vocab_index.feature_names.is_empty() {
        vocab_index.feature_names = default_feature_names();
    }

    let session = ort::session::Session::builder()
        .map_err(|e| format!("onnx session builder failed: {e}"))?
        .commit_from_file(&artifact_path)
        .map_err(|e| format!("onnx load failed: {e}"))?;
    let model = Arc::new(OnnxModel {
        session: Mutex::new(session),
        vocab_index,
    });

    let mut guard = ONNX_CACHE
        .lock()
        .map_err(|_| "onnx cache lock poisoned".to_string())?;
    *guard = Some(OnnxCacheEntry {
        key,
        model: model.clone(),
    });
    Ok(model)
}

fn mtime_ms(path: &Path) -> Result<u128, String> {
    let modified = fs::metadata(path)
        .and_then(|m| m.modified())
        .map_err(|e| format!("metadata failed for {}: {e}", path.display()))?;
    modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .map_err(|e| format!("mtime before epoch for {}: {e}", path.display()))
}

impl OnnxModel {
    fn score(
        &self,
        token_norm: &str,
        candidate: &Candidate,
        deterministic_score: f64,
    ) -> Result<f64, String> {
        if self.vocab_index.is_tree_model() {
            return self.score_tree(token_norm, candidate, deterministic_score);
        }
        self.score_mlp(token_norm, candidate, deterministic_score)
    }

    fn score_tree(
        &self,
        token_norm: &str,
        candidate: &Candidate,
        deterministic_score: f64,
    ) -> Result<f64, String> {
        let features = self.features(token_norm, candidate, deterministic_score);
        let feature_tensor =
            ort::value::Tensor::from_array(([1usize, features.len()], features.into_boxed_slice()))
                .map_err(|e| format!("feature tensor failed: {e}"))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| "onnx session lock poisoned".to_string())?;
        let outputs = session
            .run(ort::inputs! {
                "features" => feature_tensor,
            })
            .map_err(|e| format!("onnx inference failed: {e}"))?;
        let output = outputs
            .get("probabilities")
            .ok_or_else(|| "onnx output 'probabilities' missing".to_string())?;
        let (_, values) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("onnx output extract failed: {e}"))?;
        values
            .get(1)
            .map(|v| (*v as f64).clamp(0.0, 1.0))
            .ok_or_else(|| "onnx probabilities empty".to_string())
    }

    fn score_mlp(
        &self,
        token_norm: &str,
        candidate: &Candidate,
        deterministic_score: f64,
    ) -> Result<f64, String> {
        let max_len = self.vocab_index.max_len.max(1);
        let token_ids = self.encode(token_norm, max_len);
        let candidate_ids = self.encode(&candidate.term, max_len);
        let features = self.features(token_norm, candidate, deterministic_score);

        let token_tensor =
            ort::value::Tensor::from_array(([1usize, max_len], token_ids.into_boxed_slice()))
                .map_err(|e| format!("token tensor failed: {e}"))?;
        let candidate_tensor =
            ort::value::Tensor::from_array(([1usize, max_len], candidate_ids.into_boxed_slice()))
                .map_err(|e| format!("candidate tensor failed: {e}"))?;
        let feature_tensor =
            ort::value::Tensor::from_array(([1usize, features.len()], features.into_boxed_slice()))
                .map_err(|e| format!("feature tensor failed: {e}"))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| "onnx session lock poisoned".to_string())?;
        let outputs = session
            .run(ort::inputs! {
                "token_ids" => token_tensor,
                "candidate_ids" => candidate_tensor,
                "features" => feature_tensor,
            })
            .map_err(|e| format!("onnx inference failed: {e}"))?;
        let output = outputs
            .get("score")
            .ok_or_else(|| "onnx output 'score' missing".to_string())?;
        let (_, values) = output
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("onnx output extract failed: {e}"))?;
        values
            .first()
            .map(|v| (*v as f64).clamp(0.0, 1.0))
            .ok_or_else(|| "onnx output empty".to_string())
    }

    fn encode(&self, text: &str, max_len: usize) -> Vec<i64> {
        let mut ids = vec![0_i64; max_len];
        let unknown = self
            .vocab_index
            .char_to_id
            .get("<unk>")
            .copied()
            .unwrap_or(1);
        for (idx, ch) in normalize_core(text).chars().take(max_len).enumerate() {
            let key = ch.to_string();
            ids[idx] = self
                .vocab_index
                .char_to_id
                .get(&key)
                .copied()
                .unwrap_or(unknown);
        }
        ids
    }

    fn features(
        &self,
        token_norm: &str,
        candidate: &Candidate,
        deterministic_score: f64,
    ) -> Vec<f32> {
        self.vocab_index
            .feature_names
            .iter()
            .map(|name| match name.as_str() {
                "edit_similarity" => edit_similarity(token_norm, &candidate.term) as f32,
                "phonetic_similarity" => phonetics::similarity(token_norm, &candidate.term) as f32,
                "length_ratio" => {
                    let lhs = token_norm.chars().count().max(1) as f32;
                    let rhs = candidate.term.chars().count().max(1) as f32;
                    lhs.min(rhs) / lhs.max(rhs)
                }
                "term_type_acronym" => bool_feature(candidate.term_type == "acronym"),
                "term_type_brand" => bool_feature(candidate.term_type == "brand"),
                "term_type_proper_noun" => bool_feature(candidate.term_type == "proper_noun"),
                "term_type_code_identifier" => {
                    bool_feature(candidate.term_type == "code_identifier")
                }
                "source_manual" => bool_feature(candidate.source == "manual"),
                "source_starred" => bool_feature(candidate.source == "starred"),
                "alias_count" => (candidate.aliases.len() as f32).min(10.0) / 10.0,
                "use_count_log" => (candidate.use_count.max(0) as f32).ln_1p() / 5.0,
                "weight" => (candidate.weight as f32 / 5.0).clamp(0.0, 1.0),
                "deterministic_score" => deterministic_score as f32,
                "is_dictionary_word" => bool_feature(is_in_dictionary(token_norm)),
                _ => 0.0,
            })
            .collect()
    }
}

fn bool_feature(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::stt_replacements::{ExportTier, ReviewStatus};
    use crate::store::tier2_policy::Tier2PolicyWeight;
    use std::collections::HashMap;

    fn vocab(term: &str, term_type: &str, source: &str) -> VocabTerm {
        VocabTerm {
            term: term.to_string(),
            weight: 1.0,
            use_count: 1,
            last_used: 0,
            source: source.to_string(),
            example_context: None,
            term_type: Some(term_type.to_string()),
            meaning: None,
        }
    }

    fn rule(transcript_form: &str, correct_form: &str) -> SttReplacement {
        SttReplacement {
            transcript_form: transcript_form.to_string(),
            correct_form: correct_form.to_string(),
            phonetic_key: phonetics::phonetic_key(transcript_form),
            weight: 1.0,
            use_count: 1,
            last_used: 0,
            language: None,
            export_tier: ExportTier::LocalOnly,
            contradiction_count: 0,
            review_status: ReviewStatus::Approved,
            review_reason: None,
            last_reviewed_at: None,
        }
    }

    fn edit_rule(
        variant_form: &str,
        correct_form: &str,
        left_context: Vec<&str>,
        right_context: Vec<&str>,
    ) -> Tier2EditPolicyRule {
        Tier2EditPolicyRule {
            variant_form: variant_form.to_string(),
            variant_norm: normalize_core(variant_form),
            correct_form: correct_form.to_string(),
            correct_form_norm: normalize_core(correct_form),
            edit_type: "replace".to_string(),
            positive_count: 2,
            negative_count: 0,
            status: "active".to_string(),
            left_context: left_context.into_iter().map(str::to_string).collect(),
            right_context: right_context.into_iter().map(str::to_string).collect(),
        }
    }

    fn policy_weight(
        token: &str,
        correct_form: &str,
        positive_count: i64,
        negative_count: i64,
        learned_weight: f64,
    ) -> HashMap<PolicyKey, Tier2PolicyWeight> {
        let token_norm = normalize_core(token);
        let correct_form_norm = normalize_core(correct_form);
        HashMap::from([(
            (token_norm.clone(), correct_form_norm.clone()),
            Tier2PolicyWeight {
                token_norm,
                correct_form: correct_form.to_string(),
                correct_form_norm,
                positive_count,
                negative_count,
                learned_weight,
                first_seen: 1,
                last_seen: 1,
                last_reward_source: Some("test".to_string()),
            },
        )])
    }

    #[test]
    fn tier2_deterministic_scores_near_alias_as_shadow_only() {
        let rules = vec![rule("meac", "EMIAC")];
        let vocab = vec![vocab("EMIAC", "acronym", "auto")];
        let result = CorrectionEngine::new(&rules, &vocab, None).correct("meah mein kaam hai");

        assert_eq!(result.text, "meah mein kaam hai");
        assert!(result.matches.is_empty());
        assert!(
            result
                .traces
                .iter()
                .any(|trace| trace.kind == MatchKind::Tier2Deterministic
                    && !trace.applied
                    && trace.candidate == "EMIAC")
        );
    }

    #[test]
    fn tier2_deterministic_scores_close_vocab_term_as_shadow_only() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let result = CorrectionEngine::new(&rules, &vocab, None).correct("mecobs ka IPO hai");

        assert_eq!(result.text, "mecobs ka IPO hai");
        assert!(result.matches.is_empty());
        assert!(
            result
                .traces
                .iter()
                .any(|trace| trace.kind == MatchKind::Tier2Deterministic
                    && !trace.applied
                    && trace.candidate == "Macobs")
        );
    }

    #[test]
    fn common_words_are_not_replaced() {
        let rules = vec![rule("mian", "EMIAC")];
        let vocab = vec![vocab("EMIAC", "acronym", "auto")];
        let result = CorrectionEngine::new(&rules, &vocab, None).correct("mein time can go");

        assert_eq!(result.text, "mein time can go");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn low_margin_candidates_are_rejected() {
        let rules = vec![rule("meac", "EMIAC"), rule("mead", "EMEAD")];
        let vocab = vec![
            vocab("EMIAC", "acronym", "auto"),
            vocab("EMEAD", "acronym", "auto"),
        ];
        let result = CorrectionEngine::new(&rules, &vocab, None).correct("meah launch hua");

        assert_eq!(result.text, "meah launch hua");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn exact_aliases_win_before_tier2() {
        let rules = vec![rule("meah", "EMIAC"), rule("meac", "EMACS")];
        let vocab = vec![
            vocab("EMIAC", "acronym", "auto"),
            vocab("EMACS", "acronym", "auto"),
        ];
        let result = CorrectionEngine::new(&rules, &vocab, None).correct("meah rollout");

        assert_eq!(result.text, "EMIAC rollout");
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].kind, MatchKind::Exact);
    }

    #[test]
    fn blocked_replacements_do_not_feed_tier2() {
        let mut blocked = rule("meah", "EMIAC");
        blocked.review_status = ReviewStatus::Blocked;
        let rules = vec![blocked];
        let vocab = vec![vocab("EMIAC", "acronym", "auto")];
        let result = CorrectionEngine::new(&rules, &vocab, None).correct("meah rollout");

        assert_eq!(result.text, "meah rollout");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn broken_onnx_metadata_falls_back_to_deterministic() {
        let rules = vec![rule("meac", "EMIAC")];
        let vocab = vec![vocab("EMIAC", "acronym", "auto")];
        let metadata = Tier2ModelMetadata {
            user_id: "u1".to_string(),
            artifact_path: std::env::temp_dir()
                .join("said-missing-correction-model.onnx")
                .to_string_lossy()
                .into_owned(),
            vocab_index_path: std::env::temp_dir()
                .join("said-missing-vocab-index.json")
                .to_string_lossy()
                .into_owned(),
            trained_at: 0,
            alias_count: 1,
            vocab_count: 1,
            data_fingerprint: None,
            metrics_json: None,
            last_error: Some("missing".to_string()),
            updated_at: 0,
        };
        let result =
            CorrectionEngine::new(&rules, &vocab, Some(metadata)).correct("meah mein kaam hai");

        // ONNX is missing, so deterministic scoring stays shadow-only.
        assert_eq!(result.text, "meah mein kaam hai");
        assert!(result.matches.is_empty());
        assert!(
            result
                .traces
                .iter()
                .any(|trace| trace.kind == MatchKind::Tier2Deterministic
                    && !trace.applied
                    && trace.candidate == "EMIAC")
        );
    }

    #[test]
    fn sqlite_policy_reward_now_applies() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        // "macops" is close enough to "macobs" (det >= 0.68) for the floor gate
        let policy = policy_weight("macops", "Macobs", 1, 0, 1.0);
        let result = CorrectionEngine::new_with_policy(&rules, &vocab, None, policy, vec![])
            .correct("macops ka IPO hai");

        assert_eq!(result.text, "Macobs ka IPO hai");
        assert_eq!(result.matches.len(), 1);
        assert!(!result.traces.is_empty());
        assert!(result.traces[0].applied);
    }

    #[test]
    fn deterministic_floor_blocks_dissimilar_policy_boost() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        // "bimmicop" is too dissimilar to "macobs" — policy can't override
        let policy = policy_weight("bimmicop", "Macobs", 1, 0, 1.0);
        let result = CorrectionEngine::new_with_policy(&rules, &vocab, None, policy, vec![])
            .correct("bimmicop ka IPO hai");

        assert_eq!(result.text, "bimmicop ka IPO hai");
        assert!(result.matches.is_empty());
        assert!(!result.traces.is_empty());
        assert!(!result.traces[0].applied);
    }

    #[test]
    fn active_edit_policy_rule_fixes_next_dictation() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let edit_rules = vec![edit_rule("macops", "Macobs", vec![], vec![])];
        let result =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules)
                .correct("macops ka IPO hai");

        assert_eq!(result.text, "Macobs ka IPO hai");
        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].kind, MatchKind::Tier2EditPolicy);
    }

    #[test]
    fn collect_evidence_preserves_raw_text_then_final_resolves_exact_alias() {
        let rules = vec![rule("macops", "Macobs")];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);

        let raw = "macops ka 50% growth hai";
        let evidence = engine.collect_evidence(raw);
        assert_eq!(evidence.source_text, raw);
        assert_eq!(evidence.as_apply_result().text, raw);
        assert_eq!(evidence.matches.len(), 1);
        assert_eq!(evidence.matches[0].kind, MatchKind::Exact);

        let final_result = engine.final_resolve("macops ka 50% growth hai", &evidence, raw);
        assert_eq!(final_result.text, "Macobs ka 50% growth hai");
        assert_eq!(final_result.matches.len(), 1);
        assert_eq!(final_result.matches[0].kind, MatchKind::Exact);
    }

    #[test]
    fn final_resolve_applies_active_edit_policy_after_polish() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let edit_rules = vec![edit_rule("macops", "Macobs", vec![], vec![])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        let raw = "macops ka 50% growth hai";
        let evidence = engine.collect_evidence(raw);
        assert_eq!(evidence.source_text, raw);
        assert_eq!(evidence.as_apply_result().text, raw);

        let final_result = engine.final_resolve("macops ka 50% growth hai", &evidence, raw);
        assert_eq!(final_result.text, "Macobs ka 50% growth hai");
        assert_eq!(final_result.matches.len(), 1);
        assert_eq!(final_result.matches[0].kind, MatchKind::Tier2EditPolicy);
    }

    #[test]
    fn final_resolve_applies_sqlite_policy_evidence_after_polish() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let policy = policy_weight("macops", "Macobs", 2, 0, 1.2);
        let engine = CorrectionEngine::new_with_policy(&rules, &vocab, None, policy, vec![]);

        let raw = "macops ka IPO hai";
        let evidence = engine.collect_evidence(raw);
        assert_eq!(evidence.source_text, raw);
        assert!(evidence.matches.is_empty());
        assert!(
            evidence.traces.iter().any(|trace| {
                trace.kind == MatchKind::Tier2Policy
                    && trace.applied
                    && trace.policy_score.is_some()
            }),
            "policy evidence should be collected without mutating source"
        );

        let final_result = engine.final_resolve("macops ka IPO hai", &evidence, raw);
        assert_eq!(final_result.text, "Macobs ka IPO hai");
        assert_eq!(final_result.matches.len(), 1);
        assert_eq!(final_result.matches[0].kind, MatchKind::Tier2Policy);
    }

    #[test]
    fn final_resolve_keeps_deterministic_fuzzy_shadow_only() {
        let rules = vec![rule("meac", "EMIAC")];
        let vocab = vec![vocab("EMIAC", "acronym", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);

        let raw = "meah ka status bhejo";
        let evidence = engine.collect_evidence(raw);
        assert!(evidence.matches.is_empty());
        assert!(
            evidence
                .traces
                .iter()
                .any(|trace| trace.kind == MatchKind::Tier2Deterministic && !trace.applied)
        );

        let final_result = engine.final_resolve("meah ka status bhejo", &evidence, raw);
        assert_eq!(final_result.text, "meah ka status bhejo");
        assert!(final_result.matches.is_empty());
    }

    #[test]
    fn policy_penalty_prevents_bad_mapping() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let policy = policy_weight("bimmicop", "Macobs", 1, 2, -2.0);
        let result = CorrectionEngine::new_with_policy(&rules, &vocab, None, policy, vec![])
            .correct("bimmicop ka IPO hai");

        assert_eq!(result.text, "bimmicop ka IPO hai");
        assert!(result.matches.is_empty());
        assert_eq!(result.traces.len(), 1);
        assert!(!result.traces[0].applied);
    }

    #[test]
    fn policy_boost_cannot_rewrite_common_words() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let policy = policy_weight("main", "Macobs", 10, 0, 5.0);
        let result = CorrectionEngine::new_with_policy(&rules, &vocab, None, policy, vec![])
            .correct("main time can go");

        assert_eq!(result.text, "main time can go");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn policy_boost_cannot_rewrite_known_vocab_terms() {
        let rules = vec![];
        let vocab = vec![
            vocab("Macobs", "proper_noun", "auto"),
            vocab("EMIAC", "acronym", "auto"),
        ];
        let policy = policy_weight("Macobs", "EMIAC", 10, 0, 5.0);
        let result = CorrectionEngine::new_with_policy(&rules, &vocab, None, policy, vec![])
            .correct("Macobs mein kaam hai");

        assert_eq!(result.text, "Macobs mein kaam hai");
        assert!(result.matches.is_empty());
        assert!(result.traces.is_empty());
    }

    #[test]
    fn adversarial_common_phrases_do_not_trigger_acronyms() {
        let rules = vec![];
        let vocab = vec![
            vocab("8GB", "code_identifier", "auto"),
            vocab("RAM", "acronym", "auto"),
            vocab("RT", "acronym", "auto"),
            vocab("PR", "acronym", "auto"),
            vocab("SQL", "acronym", "auto"),
            vocab("EMI", "acronym", "auto"),
        ];
        let samples = [
            "128 GB RAM hai mere laptop mein",
            "ramadan mubarak bhai",
            "return the value please",
            "prayer time ho gaya",
            "sequel to the movie",
            "emission control system",
        ];

        let engine = CorrectionEngine::new(&rules, &vocab, None);
        for sample in samples {
            let result = engine.correct(sample);
            assert_eq!(result.text, sample);
            assert!(
                result.matches.is_empty(),
                "{sample:?} should not be rewritten"
            );
        }
    }

    #[test]
    fn global_fuzzy_does_not_rewrite_adversarial_words() {
        let rules = vec![];
        let vocab = vec![
            vocab("EMIAC", "acronym", "auto"),
            vocab("Macobs", "proper_noun", "auto"),
            vocab("RBAC", "acronym", "auto"),
        ];
        let engine = CorrectionEngine::new(&rules, &vocab, None);
        for sample in [
            "think about the meaning",
            "accounts team ne corps ko mail kiya",
            "rbi policy aayi hai",
        ] {
            let result = engine.correct(sample);
            assert_eq!(result.text, sample);
            assert!(result.matches.is_empty());
        }
    }

    #[test]
    fn hindi_common_words_do_not_become_macobs_with_existing_aliases() {
        let rules = vec![
            rule("macos", "Macobs"),
            rule("micobs", "Macobs"),
            rule("mecobs", "Macobs"),
        ];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);
        for sample in ["ye kaisa laga", "kaisi lagi", "यह कैसा चल रहा है", "कैसा"]
        {
            let result = engine.correct(sample);
            assert_eq!(result.text, sample, "{sample:?} should not be rewritten");
            assert!(result.matches.is_empty());
        }
    }

    #[test]
    fn dictionary_like_edit_policy_variant_is_hard_blocked() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let edit_rules = vec![edit_rule("corps", "Macobs", vec![], vec!["ki"])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        let unrelated = engine.correct("army corps division");
        assert_eq!(unrelated.text, "army corps division");
        assert!(unrelated.matches.is_empty());

        let contextual = engine.correct("corps ki website kholo");
        assert_eq!(contextual.text, "corps ki website kholo");
        assert!(contextual.matches.is_empty());
    }

    #[test]
    fn hinglish_particle_context_does_not_enable_common_variant() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let edit_rules = vec![edit_rule(
            "corps",
            "Macobs",
            vec![],
            vec!["kaisi", "company", "hai"],
        )];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        let result = engine.correct("corps ka kaam ho gaya");
        assert_eq!(result.text, "corps ka kaam ho gaya");
        assert!(result.matches.is_empty());

        let result2 = engine.correct("mein corps ko bolta hun");
        assert_eq!(result2.text, "mein corps ko bolta hun");

        let english = engine.correct("army corps marched forward");
        assert_eq!(english.text, "army corps marched forward");
        assert!(english.matches.is_empty());
    }

    #[test]
    fn devanagari_context_enables_safe_non_common_variant() {
        let rules = vec![];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let edit_rules = vec![edit_rule(
            "mccorps",
            "Macobs",
            vec![],
            vec!["ka", "batana", "ipo"],
        )];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        // Devanagari "का" should romanize to "kaa" which matches particle "ka" context
        let result = engine.correct("मैं mccorps का बताना");
        assert_eq!(
            result.text, "मैं Macobs का बताना",
            "Devanagari का should provide Hinglish particle context for mccorps→Macobs"
        );
        assert_eq!(result.matches[0].kind, MatchKind::Tier2EditPolicy);

        // Devanagari "की" → "kee", still close enough to "ki" particle
        let result2 = engine.correct("मैं mccorps की website");
        assert!(
            !result2.matches.is_empty() || result2.text.contains("Macobs"),
            "Devanagari की should also work as context"
        );
    }

    #[test]
    fn cluster_fuzzy_catches_new_distortions_of_known_target() {
        let rules = vec![rule("mecobs", "Macobs")];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let edit_rules = vec![edit_rule("macops", "Macobs", vec![], vec![])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        let result = engine.correct("mecorbs ka data bhejo");
        assert_eq!(result.text, "Macobs ka data bhejo");
        assert_eq!(result.matches[0].kind, MatchKind::Tier2ClusterFuzzy);

        let result2 = engine.correct("mcorbs ka report hai");
        assert_eq!(result2.text, "Macobs ka report hai");
        assert_eq!(result2.matches[0].kind, MatchKind::Tier2ClusterFuzzy);
    }

    #[test]
    fn cluster_fuzzy_catches_kubernetes_variants() {
        let rules = vec![];
        let vocab = vec![vocab("Kubernetes", "brand", "auto")];
        let edit_rules = vec![edit_rule("cubinatis", "Kubernetes", vec![], vec![])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        let result = engine.correct("cubernetize bhi nahi kar pa raha");
        assert_eq!(result.text, "Kubernetes bhi nahi kar pa raha");
        assert_eq!(result.matches[0].kind, MatchKind::Tier2ClusterFuzzy);

        let result2 = engine.correct("cubernetis ka cluster setup karo");
        assert_eq!(result2.text, "Kubernetes ka cluster setup karo");
    }

    #[test]
    fn cluster_fuzzy_does_not_replace_common_words() {
        let rules = vec![];
        let vocab = vec![
            vocab("EMIAC", "acronym", "auto"),
            vocab("Macobs", "proper_noun", "auto"),
        ];
        let edit_rules = vec![
            edit_rule("meic", "EMIAC", vec![], vec![]),
            edit_rule("macops", "Macobs", vec![], vec![]),
        ];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        for sample in [
            "think about the meaning",
            "main time can go",
            "rbi policy aayi hai",
            "accounts team meeting hai",
        ] {
            let result = engine.correct(sample);
            assert_eq!(
                result.text, sample,
                "common words should not be replaced: {sample}"
            );
        }
    }

    #[test]
    fn cluster_fuzzy_does_not_fire_without_active_edit_rules() {
        let rules = vec![rule("mecobs", "Macobs")];
        let vocab = vec![vocab("Macobs", "proper_noun", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);

        let result = engine.correct("mecorbs ka data bhejo");
        // Cluster-fuzzy specifically should not fire without edit rules.
        assert!(
            result
                .matches
                .iter()
                .all(|m| m.kind != MatchKind::Tier2ClusterFuzzy),
            "cluster fuzzy should not apply without active edit policy rules"
        );
        // The global fuzzy scorer is shadow-only unless explicit policy evidence exists.
        assert_eq!(result.text, "mecorbs ka data bhejo");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn cluster_fuzzy_catches_emiac_new_variant() {
        let rules = vec![rule("meac", "EMIAC")];
        let vocab = vec![vocab("EMIAC", "acronym", "auto")];
        let edit_rules = vec![edit_rule("meic", "EMIAC", vec![], vec![])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        let result = engine.correct("emiag ka kaam ho gaya");
        assert_eq!(result.text, "EMIAC ka kaam ho gaya");
        assert_eq!(result.matches[0].kind, MatchKind::Tier2ClusterFuzzy);
    }

    #[test]
    fn cluster_fuzzy_does_not_replace_dock_worker_with_docker() {
        let rules = vec![];
        let vocab = vec![vocab("Docker", "proper_noun", "auto")];
        let docker_distortions = [
            "doker", "dockor", "dokar", "doccur", "dauker", "dokker", "dockar",
        ];
        let edit_rules: Vec<Tier2EditPolicyRule> = docker_distortions
            .iter()
            .map(|d| edit_rule(d, "Docker", vec!["check"], vec!["container"]))
            .collect();
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);

        let result = engine.correct("the dock worker checked the container manually");
        assert_eq!(
            result.text,
            "the dock worker checked the container manually"
        );
        assert!(result.matches.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Dictionary-gated protection: real words must NEVER be corrected.
    // Distortions of those words (not in any dictionary) MUST be corrected.
    // ═══════════════════════════════════════════════════════════════════════════

    // ── is_in_dictionary unit tests ──────────────────────────────────────────

    #[test]
    fn dictionary_rejects_gibberish() {
        for token in &[
            "macops",
            "airnot",
            "kafk",
            "cubernatis",
            "emiac",
            "meac",
            "mecobs",
            "grahk",
            "senshry",
            "dockar",
            "growk",
            "linuks",
        ] {
            assert!(
                !is_in_dictionary(token),
                "{token:?} is gibberish — must NOT be in dictionary"
            );
        }
    }

    #[test]
    fn dictionary_accepts_english_words() {
        for token in &[
            "mac", "agent", "server", "cloud", "host", "port", "build", "test", "node", "type",
            "class", "table", "merge", "push", "pull", "branch", "version", "release", "spring",
            "flask", "nest", "dart", "rust", "swift", "react", "hub", "lab", "dock", "tower",
            "guard", "press", "fire", "cell", "graph", "base", "strip", "local", "post", "super",
            "cool",
        ] {
            assert!(
                is_in_dictionary(token),
                "{token:?} is a real English word — must be in dictionary"
            );
        }
    }

    #[test]
    fn dictionary_accepts_hindi_common_words() {
        for token in &[
            "kar", "bol", "sun", "de", "le", "ja", "aa", "ho", "hai", "hain", "tha", "nahi", "kya",
            "kaise", "kab", "aaj", "kal", "aajkal", "din", "raat", "ghar", "subah", "shaam",
            "baat", "kaam", "cheez", "mil", "chal", "rakh",
        ] {
            assert!(
                is_in_dictionary(token),
                "{token:?} is a common Hindi word — must be in dictionary"
            );
        }
    }

    #[test]
    fn dictionary_is_case_insensitive() {
        assert!(is_in_dictionary("Mac"));
        assert!(is_in_dictionary("MAC"));
        assert!(is_in_dictionary("Agent"));
        assert!(is_in_dictionary("AGENT"));
        assert!(is_in_dictionary("Hai"));
        assert!(is_in_dictionary("KAR"));
    }

    #[test]
    fn dictionary_strips_punctuation() {
        assert!(is_in_dictionary("mac,"));
        assert!(is_in_dictionary("agent."));
        assert!(is_in_dictionary("server!"));
        assert!(is_in_dictionary("(cloud)"));
    }

    // ── is_suspicious_token unit tests ───────────────────────────────────────

    #[test]
    fn suspicious_rejects_dictionary_words() {
        for token in &["mac", "agent", "server", "cloud", "host", "port", "build"] {
            assert!(
                !is_suspicious_token(token),
                "{token:?} is a real word — must NOT be suspicious"
            );
        }
    }

    #[test]
    fn suspicious_accepts_gibberish() {
        for token in &["macops", "airnot", "emiac", "mecobs", "cubernatis", "kafk"] {
            assert!(
                is_suspicious_token(token),
                "{token:?} is gibberish — must be suspicious"
            );
        }
    }

    #[test]
    fn suspicious_rejects_short_tokens() {
        assert!(!is_suspicious_token("ab"));
        assert!(!is_suspicious_token("a"));
        assert!(!is_suspicious_token(""));
    }

    #[test]
    fn suspicious_rejects_all_digits() {
        assert!(!is_suspicious_token("12345"));
        assert!(!is_suspicious_token("007"));
    }

    // ── Full pipeline: English words survive all 4 passes ────────────────────

    fn engine_with_emiac() -> CorrectionEngine<'static> {
        // Leaking is fine for tests — avoids lifetime gymnastics.
        let rules: &'static [SttReplacement] =
            Box::leak(Box::new([rule("meac", "EMIAC"), rule("emiak", "EMIAC")]));
        let vocab: &'static [VocabTerm] = Box::leak(Box::new([vocab("EMIAC", "brand", "auto")]));
        CorrectionEngine::new(rules, vocab, None)
    }

    fn engine_with_many_terms() -> CorrectionEngine<'static> {
        let rules: &'static [SttReplacement] = Box::leak(Box::new([
            rule("meac", "EMIAC"),
            rule("emiak", "EMIAC"),
            rule("macops", "Macobs"),
            rule("airnot", "AirNote"),
            rule("air not", "AirNote"),
            rule("kafk", "Kafka"),
            rule("cubernatis", "Kubernetes"),
            rule("growk", "Groq"),
            rule("groq", "Groq"),
        ]));
        let vocab: &'static [VocabTerm] = Box::leak(Box::new([
            vocab("EMIAC", "brand", "auto"),
            vocab("Macobs", "proper_noun", "auto"),
            vocab("AirNote", "brand", "auto"),
            vocab("Kafka", "brand", "auto"),
            vocab("Kubernetes", "brand", "auto"),
            vocab("Groq", "brand", "auto"),
        ]));
        CorrectionEngine::new(rules, vocab, None)
    }

    #[test]
    fn mac_is_never_corrected_to_emiac() {
        let engine = engine_with_emiac();
        let result = engine.correct("open the mac settings");
        assert_eq!(result.text, "open the mac settings");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn agent_is_never_corrected_to_airnote() {
        let engine = engine_with_many_terms();
        let result = engine.correct("the agent will handle it");
        assert_eq!(result.text, "the agent will handle it");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn english_words_survive_full_pipeline() {
        let engine = engine_with_many_terms();
        let words = [
            ("mac", "I bought a mac today"),
            ("host", "the host is ready"),
            ("cloud", "deploy to cloud"),
            ("port", "check the port number"),
            ("merge", "merge the branch"),
            ("table", "drop the table"),
            ("spring", "spring is here"),
            ("flask", "the flask app"),
            ("nest", "a bird nest"),
            ("swift", "write in swift"),
            ("cell", "the cell value"),
            ("fire", "fire the event"),
            ("press", "press the button"),
            ("tower", "the tower of babel"),
            ("guard", "the guard clause"),
            ("base", "the base class"),
            ("dock", "the dock area"),
            ("rest", "the rest api"),
            ("corps", "peace corps"),
        ];
        for (word, sentence) in &words {
            let result = engine.correct(sentence);
            assert_eq!(
                &result.text, sentence,
                "{word:?} was wrongly corrected in {sentence:?} → {:?}",
                result.text
            );
            assert!(
                result.matches.is_empty(),
                "{word:?} produced matches: {:?}",
                result.matches
            );
        }
    }

    #[test]
    fn hindi_words_survive_full_pipeline() {
        let engine = engine_with_many_terms();
        let sentences = [
            "aaj mein kaam kar raha hoon",
            "kal subah meeting hai",
            "aajkal bahut kaam hai",
            "nahi yeh sahi nahi hai",
            "ghar pe aana hai shaam ko",
            "baat sun meri bol raha hoon",
            "din raat kaam karta hai",
            "cheez achhi hai bilkul",
        ];
        for sentence in &sentences {
            let result = engine.correct(sentence);
            assert_eq!(
                &result.text, sentence,
                "Hindi sentence was modified: {sentence:?} → {:?}",
                result.text
            );
            assert!(
                result.matches.is_empty(),
                "Hindi sentence produced matches: {:?} in {sentence:?}",
                result.matches
            );
        }
    }

    // ── Full pipeline: distortions ARE corrected ─────────────────────────────

    #[test]
    fn gibberish_distortions_are_corrected() {
        let engine = engine_with_many_terms();

        let result = engine.correct("meac ka kaam hai");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "EMIAC"),
            "meac → EMIAC exact alias must fire"
        );

        let result = engine.correct("macops ka IPO hai");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Macobs"),
            "macops → Macobs exact alias must fire"
        );

        let result = engine.correct("airnot install karo");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "AirNote"),
            "airnot → AirNote exact alias must fire"
        );

        let result = engine.correct("kafk cluster check karo");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Kafka"),
            "kafk → Kafka exact alias must fire"
        );
    }

    #[test]
    fn growk_exact_alias_becomes_groq() {
        let engine = engine_with_many_terms();
        let result = engine.correct("growk is fast");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Groq"),
            "growk → Groq exact alias must fire"
        );
    }

    // ── Mixed sentences: only distortions corrected, real words untouched ────

    #[test]
    fn mixed_sentence_corrects_only_distortions() {
        let engine = engine_with_many_terms();
        let result = engine.correct("meac ka agent ready hai");
        assert!(result.text.contains("EMIAC"), "meac should become EMIAC");
        assert!(result.text.contains("agent"), "agent must stay untouched");
        assert!(result.text.contains("hai"), "hai must stay untouched");
    }

    #[test]
    fn mixed_hinglish_sentence_only_fixes_distortions() {
        let engine = engine_with_many_terms();
        let result = engine.correct("aaj macops ka release hai bhai");
        assert!(
            result.text.contains("Macobs"),
            "macops should become Macobs"
        );
        assert!(result.text.contains("aaj"), "aaj must stay untouched");
        assert!(result.text.contains("hai"), "hai must stay untouched");
        assert!(result.text.contains("bhai"), "bhai must stay untouched");
    }

    // ── Edit-policy pass respects dictionary ─────────────────────────────────

    #[test]
    fn edit_policy_skips_dictionary_words() {
        let rules = vec![];
        let vocab = vec![vocab("EMIAC", "brand", "auto")];
        let edit_rules = vec![edit_rule("mac", "EMIAC", vec!["the"], vec!["settings"])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);
        let result = engine.correct("the mac settings are here");
        assert_eq!(result.text, "the mac settings are here");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn edit_policy_corrects_gibberish_variants() {
        let rules = vec![];
        let vocab = vec![vocab("EMIAC", "brand", "auto")];
        let edit_rules = vec![edit_rule("meac", "EMIAC", vec!["the"], vec![])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);
        let result = engine.correct("the meac product launch");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "EMIAC"),
            "edit-policy should correct 'meac' → EMIAC"
        );
    }

    // ── Cluster-fuzzy pass respects dictionary ───────────────────────────────

    #[test]
    fn cluster_fuzzy_skips_real_words_near_vocab() {
        let rules = vec![];
        let vocab = vec![
            vocab("EMIAC", "brand", "auto"),
            vocab("AirNote", "brand", "auto"),
            vocab("Kafka", "brand", "auto"),
        ];
        let edit_rules = vec![
            edit_rule("meac", "EMIAC", vec![], vec![]),
            edit_rule("airnot", "AirNote", vec![], vec![]),
            edit_rule("kafk", "Kafka", vec![], vec![]),
        ];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);
        let result = engine.correct("the agent and host are on fire today");
        assert_eq!(
            result.text, "the agent and host are on fire today",
            "all real words must survive cluster-fuzzy"
        );
        assert!(result.matches.is_empty());
    }

    // ── Exact alias pass: dictionary source words are filtered at load ───────

    #[test]
    fn exact_alias_from_dictionary_word_is_filtered() {
        let rules = vec![rule("mac", "EMIAC"), rule("meac", "EMIAC")];
        let vocab = vec![vocab("EMIAC", "brand", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);
        let result = engine.correct("mac is great and meac is here");
        assert!(
            result.text.contains("mac"),
            "mac must survive — {:?}",
            result.text
        );
        assert!(
            result.matches.iter().all(|m| m.transcript_form != "mac"),
            "mac must not appear as a corrected transcript_form"
        );
        assert!(
            result
                .matches
                .iter()
                .any(|m| m.transcript_form == "meac" && m.correct_form == "EMIAC"),
            "meac (gibberish) must still be corrected to EMIAC"
        );
    }

    // ── Boundary: vocab terms that ARE real words ────────────────────────────

    #[test]
    fn vocab_term_that_is_a_real_word_is_not_self_corrected() {
        let rules = vec![rule("rost", "Rust")];
        let vocab = vec![vocab("Rust", "brand", "manual")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);
        let result = engine.correct("I like rust and rost is close");
        assert!(
            result.text.contains("rust"),
            "rust (lowercase, a real word) must not be touched"
        );
    }

    // ── The exact false-positive cases from production ────────────────────────

    #[test]
    fn prod_false_positive_mac_to_emiac() {
        let rules = vec![rule("meac", "EMIAC"), rule("emiak", "EMIAC")];
        let vocab = vec![vocab("EMIAC", "brand", "auto")];
        let edit_rules = vec![edit_rule("meac", "EMIAC", vec![], vec![])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);
        let result = engine.correct("open mac settings and check");
        assert_eq!(result.text, "open mac settings and check");
        assert!(
            result.matches.is_empty(),
            "mac → EMIAC must NOT happen: {:?}",
            result.matches
        );
    }

    #[test]
    fn prod_false_positive_agent_to_airnote() {
        let rules = vec![rule("airnot", "AirNote"), rule("air not", "AirNote")];
        let vocab = vec![vocab("AirNote", "brand", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);
        let result = engine.correct("the agent responded quickly");
        assert_eq!(result.text, "the agent responded quickly");
        assert!(result.matches.is_empty());
    }

    #[test]
    fn prod_false_positive_database_survives() {
        let rules = vec![];
        let vocab = vec![vocab("DataBricks", "brand", "auto")];
        let edit_rules = vec![edit_rule("databrik", "DataBricks", vec![], vec![])];
        let engine =
            CorrectionEngine::new_with_policy(&rules, &vocab, None, HashMap::new(), edit_rules);
        let result = engine.correct("the database migration ran successfully");
        assert_eq!(result.text, "the database migration ran successfully");
    }

    #[test]
    fn prod_false_positive_nahi_to_n8n() {
        let rules = vec![rule("n8n", "n8n")];
        let vocab = vec![vocab("n8n", "brand", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);
        let result = engine.correct("nahi yeh sahi nahi hai");
        assert_eq!(result.text, "nahi yeh sahi nahi hai");
    }

    #[test]
    fn prod_false_positive_aajkal_survives() {
        let rules = vec![rule("kafk", "Kafka")];
        let vocab = vec![vocab("Kafka", "brand", "auto")];
        let engine = CorrectionEngine::new(&rules, &vocab, None);
        let result = engine.correct("aajkal bahut kaam ho raha hai");
        assert_eq!(result.text, "aajkal bahut kaam ho raha hai");
    }

    // ── Distortions close to real words still get corrected ──────────────────

    #[test]
    fn close_distortion_airnot_becomes_airnote() {
        let engine = engine_with_many_terms();
        let result = engine.correct("download airnot from the store");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "AirNote"),
            "airnot → AirNote must work — it's not a real word"
        );
    }

    #[test]
    fn close_distortion_meac_becomes_emiac() {
        let engine = engine_with_many_terms();
        let result = engine.correct("meac tech company hai");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "EMIAC"),
            "meac → EMIAC must work — it's not a real word"
        );
    }

    #[test]
    fn close_distortion_emiak_becomes_emiac() {
        let engine = engine_with_many_terms();
        let result = engine.correct("emiak ka office kahan hai");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "EMIAC"),
            "emiak → EMIAC must work — it's not a real word"
        );
    }

    #[test]
    fn close_distortion_growk_becomes_groq() {
        let engine = engine_with_many_terms();
        let result = engine.correct("growk api check karo");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Groq"),
            "growk → Groq must work — it's not a real word"
        );
    }

    // ── Multi-token aliases ──────────────────────────────────────────────────

    #[test]
    fn multi_token_alias_air_not_becomes_airnote() {
        let engine = engine_with_many_terms();
        let result = engine.correct("download air not now");
        let has_correction = result.matches.iter().any(|m| m.correct_form == "AirNote");
        assert!(
            has_correction || result.text.contains("AirNote"),
            "multi-token 'air not' → AirNote must work"
        );
    }

    // ── Stress test: batch of 20 real words in one sentence ──────────────────

    #[test]
    fn twenty_real_words_none_corrected() {
        let engine = engine_with_many_terms();
        let result = engine.correct(
            "the agent will host a cloud server on port eight \
             with a merge of the table and spring flask for the \
             nest swift cell fire press tower guard",
        );
        assert!(
            result.matches.is_empty(),
            "no real word should be corrected — got: {:?}",
            result
                .matches
                .iter()
                .map(|m| format!("{:?}→{}", m.transcript_form, m.correct_form))
                .collect::<Vec<_>>()
        );
    }

    // ── Stress test: batch of distortions in one sentence ────────────────────

    #[test]
    fn multiple_distortions_all_corrected() {
        let engine = engine_with_many_terms();
        let result = engine.correct("meac aur macops dono acche hain");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "EMIAC"),
            "meac → EMIAC must fire in multi-correction"
        );
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Macobs"),
            "macops → Macobs must fire in multi-correction"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Adversarial close-pair tests: real English/Hindi word vs. its distortion.
    // Each pair has a real word that must SURVIVE and a distortion that must be
    // CORRECTED. These simulate the hardest production cases where a brand name
    // differs from a common word by 1-2 characters.
    // ═══════════════════════════════════════════════════════════════════════════

    fn devtools_engine() -> CorrectionEngine<'static> {
        let rules: &'static [SttReplacement] = Box::leak(Box::new([
            // Kafka
            rule("kafk", "Kafka"),
            rule("kafkaa", "Kafka"),
            // Kubernetes
            rule("cubernatis", "Kubernetes"),
            rule("kubernets", "Kubernetes"),
            rule("kubr", "Kubernetes"),
            // Macobs
            rule("macops", "Macobs"),
            rule("mecobs", "Macobs"),
            // EMIAC
            rule("meac", "EMIAC"),
            rule("emiak", "EMIAC"),
            // AirNote
            rule("airnot", "AirNote"),
            rule("ajent", "AirNote"),
            // Groq
            rule("growk", "Groq"),
            rule("grok", "Groq"),
            // Prisma
            rule("prizm", "Prisma"),
            rule("prizma", "Prisma"),
            // Sentry
            rule("sentri", "Sentry"),
            rule("sentree", "Sentry"),
            // MongoDB
            rule("mongoo", "MongoDB"),
            rule("mongdb", "MongoDB"),
            // Docker
            rule("dockar", "Docker"),
            rule("dokker", "Docker"),
            // Slack
            rule("slak", "Slack"),
            rule("slck", "Slack"),
            // Stripe
            rule("stryp", "Stripe"),
            rule("stryep", "Stripe"),
            // Cursor
            rule("cursr", "Cursor"),
            rule("kursor", "Cursor"),
            // Notion
            rule("noshun", "Notion"),
            rule("noshon", "Notion"),
            // Consul
            rule("consl", "Consul"),
            rule("konsul", "Consul"),
            // Vault
            rule("valt", "Vault"),
            rule("vawlt", "Vault"),
            // Helm
            rule("helam", "Helm"),
            rule("hlm", "Helm"),
            // RabbitMQ
            rule("rabit", "RabbitMQ"),
            rule("rabitmq", "RabbitMQ"),
            // Celery
            rule("seleri", "Celery"),
            rule("selery", "Celery"),
            // Harbor
            rule("harbr", "Harbor"),
            rule("harbur", "Harbor"),
            // Nomad
            rule("nomeed", "Nomad"),
            rule("nomaad", "Nomad"),
            // Spark
            rule("spak", "Spark"),
            rule("sparrk", "Spark"),
            // Hive
            rule("hayv", "Hive"),
            rule("hiv", "Hive"),
            // Arrow
            rule("arow", "Arrow"),
            rule("aarro", "Arrow"),
            // LanceDB
            rule("lans", "LanceDB"),
            rule("lansdb", "LanceDB"),
            // Beam
            rule("beem", "Beam"),
            rule("beeam", "Beam"),
            // Flask
            rule("flsk", "Flask"),
            rule("flaask", "Flask"),
            // Redis
            rule("redis", "Redis"),
            rule("rediss", "Redis"),
        ]));
        let vocab: &'static [VocabTerm] = Box::leak(Box::new([
            vocab("Kafka", "brand", "auto"),
            vocab("Kubernetes", "brand", "auto"),
            vocab("Macobs", "proper_noun", "auto"),
            vocab("EMIAC", "brand", "auto"),
            vocab("AirNote", "brand", "auto"),
            vocab("Groq", "brand", "auto"),
            vocab("Prisma", "brand", "auto"),
            vocab("Sentry", "brand", "auto"),
            vocab("MongoDB", "brand", "auto"),
            vocab("Docker", "brand", "auto"),
            vocab("Slack", "brand", "auto"),
            vocab("Stripe", "brand", "auto"),
            vocab("Cursor", "brand", "auto"),
            vocab("Notion", "brand", "auto"),
            vocab("Consul", "brand", "auto"),
            vocab("Vault", "brand", "auto"),
            vocab("Helm", "brand", "auto"),
            vocab("RabbitMQ", "brand", "auto"),
            vocab("Celery", "brand", "auto"),
            vocab("Harbor", "brand", "auto"),
            vocab("Nomad", "brand", "auto"),
            vocab("Spark", "brand", "auto"),
            vocab("Hive", "brand", "auto"),
            vocab("Arrow", "brand", "auto"),
            vocab("LanceDB", "brand", "auto"),
            vocab("Beam", "brand", "auto"),
            vocab("Flask", "brand", "auto"),
            vocab("Redis", "brand", "auto"),
        ]));
        CorrectionEngine::new(rules, vocab, None)
    }

    // ── Real English words that sit 1-2 edits from a brand name ──────────────

    #[test]
    fn adversarial_english_words_never_corrected() {
        let engine = devtools_engine();
        let cases: &[(&str, &str, &str)] = &[
            // (real word, sentence, brand it must NOT become)
            ("slack", "give me some slack please", "Slack"),
            ("stripe", "the stripe on the shirt is red", "Stripe"),
            ("cursor", "move the cursor to the left", "Cursor"),
            ("notion", "I have a notion about this", "Notion"),
            ("consul", "the consul issued a visa", "Consul"),
            ("vault", "the vault has gold inside", "Vault"),
            ("helm", "he took the helm of the ship", "Helm"),
            ("rabbit", "the rabbit ran across the field", "RabbitMQ"),
            ("celery", "add celery to the soup", "Celery"),
            ("harbor", "the ship entered the harbor", "Harbor"),
            ("nomad", "he lived as a nomad for years", "Nomad"),
            ("spark", "the spark lit the fire", "Spark"),
            ("hive", "the bee hive is buzzing", "Hive"),
            ("arrow", "the arrow hit the target", "Arrow"),
            ("lance", "the knight carried a lance", "LanceDB"),
            ("beam", "the beam of light was bright", "Beam"),
            ("flask", "fill the flask with water", "Flask"),
            ("prism", "light through a prism splits", "Prisma"),
            ("sentry", "the sentry stood guard all night", "Sentry"),
            ("drone", "the drone flew over the field", "Docker"),
            ("mongo", "mongo the character was fun", "MongoDB"),
            ("docker", "the docker loaded the ship", "Docker"),
            ("mint", "the mint flavor ice cream", "Mint"),
            ("agent", "the agent signed the deal", "AirNote"),
            ("mac", "I love my mac", "Macobs"),
            ("cube", "the ice cube melted fast", "Kubernetes"),
            ("flak", "he took a lot of flak for it", "Flask"),
        ];
        for (word, sentence, forbidden_brand) in cases {
            let result = engine.correct(sentence);
            assert_eq!(
                &result.text, sentence,
                "{word:?} was wrongly corrected in: {sentence:?} → {:?}",
                result.text
            );
            assert!(
                result
                    .matches
                    .iter()
                    .all(|m| m.correct_form != *forbidden_brand),
                "{word:?} became {forbidden_brand}: {:?}",
                result.matches
            );
        }
    }

    // ── Hindi words that sit close to brand names ────────────────────────────

    #[test]
    fn adversarial_hindi_words_never_corrected() {
        let engine = devtools_engine();
        let cases: &[(&str, &str, &str)] = &[
            ("kaafi", "kaafi accha kaam kiya", "Kafka"),
            ("kafi", "kafi zyada ho gaya", "Kafka"),
            ("nahi", "nahi yeh nahi chalega", "Nomad"),
            ("aaj", "aaj ka din achha tha", "AirNote"),
            ("baat", "baat sun meri", "Beam"),
            ("baar", "ek baar aur try karo", "Beam"),
            ("hain", "sab log yahan hain", "Hive"),
            ("hai", "bahut accha hai yeh", "Hive"),
            ("kar", "kaam kar raha hoon", "Kafka"),
            ("kal", "kal milte hain", "Kafka"),
            ("din", "poora din kaam kiya", "Docker"),
            ("ghar", "ghar pe aana hai", "Groq"),
            ("log", "bahut log aaye the", "LanceDB"),
            ("mil", "sabse mil liya", "MongoDB"),
        ];
        for (word, sentence, forbidden_brand) in cases {
            let result = engine.correct(sentence);
            assert_eq!(
                &result.text, sentence,
                "Hindi {word:?} was wrongly corrected in: {sentence:?} → {:?}",
                result.text
            );
            assert!(
                result
                    .matches
                    .iter()
                    .all(|m| m.correct_form != *forbidden_brand),
                "Hindi {word:?} became {forbidden_brand}: {:?}",
                result.matches
            );
        }
    }

    // ── Distortions of the SAME brands that must still get corrected ─────────

    #[test]
    fn adversarial_distortions_still_corrected() {
        let engine = devtools_engine();
        let cases: &[(&str, &str, &str)] = &[
            // (distortion, sentence, expected brand)
            ("kafk", "kafk cluster check karo", "Kafka"),
            ("cubernatis", "cubernatis deploy karo", "Kubernetes"),
            ("macops", "macops ka IPO aaya", "Macobs"),
            ("meac", "meac tech company", "EMIAC"),
            ("airnot", "airnot download karo", "AirNote"),
            ("growk", "growk api use karo", "Groq"),
            ("prizm", "prizm schema check karo", "Prisma"),
            ("sentri", "sentri alerts dekho", "Sentry"),
            ("mongoo", "mongoo database setup karo", "MongoDB"),
            ("dockar", "dockar container start karo", "Docker"),
            ("slak", "slak pe message bhejo", "Slack"),
            ("stryp", "stryp payment integrate karo", "Stripe"),
            ("cursr", "cursr editor open karo", "Cursor"),
            ("noshun", "noshun workspace create karo", "Notion"),
            ("consl", "consl service register karo", "Consul"),
            ("valt", "valt secrets manage karo", "Vault"),
            ("helam", "helam chart install karo", "Helm"),
            ("rabit", "rabit queue check karo", "RabbitMQ"),
            ("seleri", "seleri task run karo", "Celery"),
            ("harbr", "harbr registry push karo", "Harbor"),
            ("beem", "beem pipeline deploy karo", "Beam"),
            ("flsk", "flsk app run karo", "Flask"),
        ];
        for (distortion, sentence, expected_brand) in cases {
            let result = engine.correct(sentence);
            assert!(
                result
                    .matches
                    .iter()
                    .any(|m| m.correct_form == *expected_brand),
                "{distortion:?} → {expected_brand} must fire in: {sentence:?}\n\
                 got text: {:?}, matches: {:?}",
                result.text,
                result
                    .matches
                    .iter()
                    .map(|m| format!("{:?}→{}", m.transcript_form, m.correct_form))
                    .collect::<Vec<_>>()
            );
        }
    }

    // ── Same sentence, real word + distortion side by side ────────────────────

    #[test]
    fn side_by_side_real_word_untouched_distortion_fixed() {
        let engine = devtools_engine();

        let result = engine.correct("the slack channel has slak issues");
        assert!(result.text.contains("slack"), "real 'slack' must stay");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Slack"),
            "distortion 'slak' must become Slack"
        );

        let result = engine.correct("move cursor to cursr editor");
        assert!(result.text.contains("cursor"), "real 'cursor' must stay");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Cursor"),
            "distortion 'cursr' must become Cursor"
        );

        let result = engine.correct("kaafi achha hai kafk cluster");
        assert!(result.text.contains("kaafi"), "hindi 'kaafi' must stay");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Kafka"),
            "distortion 'kafk' must become Kafka"
        );

        let result = engine.correct("the vault is secure but valt config is wrong");
        assert!(result.text.contains("vault"), "real 'vault' must stay");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Vault"),
            "distortion 'valt' must become Vault"
        );

        let result = engine.correct("the agent said ajent is the app");
        assert!(result.text.contains("agent"), "real 'agent' must stay");

        let result = engine.correct("mac pe macops install karo");
        assert!(result.text.contains("mac"), "real 'mac' must stay");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Macobs"),
            "distortion 'macops' must become Macobs"
        );
    }

    // ── Plural and inflected forms of real words ─────────────────────────────

    #[test]
    fn plural_forms_of_real_words_survive() {
        let engine = devtools_engine();
        let sentences = [
            "the cubes are stacked neatly",
            "multiple agents were deployed",
            "all the rabbits escaped the farm",
            "the drones surveyed the area",
            "both flasks were empty",
            "the arrows pointed north",
            "all sentries reported clear",
            "the beams supported the roof",
            "several harbors were closed",
            "the sparks flew everywhere",
        ];
        for sentence in &sentences {
            let result = engine.correct(sentence);
            assert_eq!(
                &result.text, sentence,
                "plural form wrongly corrected: {sentence:?} → {:?}",
                result.text
            );
        }
    }

    // ── Mixed Hinglish with devtools distortions ─────────────────────────────

    #[test]
    fn hinglish_devtools_mixed_sentence() {
        let engine = devtools_engine();
        let result = engine.correct("aaj kaafi kaam kiya macops pe aur sentri alerts bhi dekhe");
        assert!(result.text.contains("aaj"), "aaj must survive");
        assert!(result.text.contains("kaafi"), "kaafi must survive");
        assert!(result.text.contains("kaam"), "kaam must survive");
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Macobs"),
            "macops → Macobs must fire"
        );
        assert!(
            result.matches.iter().any(|m| m.correct_form == "Sentry"),
            "sentri → Sentry must fire"
        );
    }

    // ── Dictionary boundary: verify is_in_dictionary for new additions ───────

    #[test]
    fn dictionary_has_devtools_adjacent_words() {
        for word in &[
            "slack", "stripe", "cursor", "notion", "consul", "vault", "helm", "drone", "spark",
            "hive", "celery", "rabbit", "harbor", "nomad", "beam", "arrow", "lance", "flask",
            "prism", "sentry", "mongo", "docker", "mint", "flak", "agent", "cube", "cubes",
        ] {
            assert!(
                is_in_dictionary(word),
                "{word:?} must be in dictionary — it's a real English word"
            );
        }
    }

    #[test]
    fn dictionary_has_hindi_additions() {
        for word in &["kaafi", "kafi", "pura", "poora", "accha", "sahi", "galat"] {
            assert!(
                is_in_dictionary(word),
                "Hindi {word:?} must be in dictionary"
            );
        }
    }

    #[test]
    fn dictionary_rejects_all_distortions() {
        for distortion in &[
            "kafk",
            "cubernatis",
            "kubernets",
            "macops",
            "mecobs",
            "meac",
            "emiak",
            "airnot",
            "ajent",
            "growk",
            "prizm",
            "prizma",
            "sentri",
            "sentree",
            "mongoo",
            "mongdb",
            "dockar",
            "dokker",
            "slak",
            "slck",
            "stryp",
            "stryep",
            "cursr",
            "kursor",
            "noshun",
            "noshon",
            "consl",
            "konsul",
            "valt",
            "vawlt",
            "helam",
            "hlm",
            "rabit",
            "rabitmq",
            "seleri",
            "selery",
            "harbr",
            "harbur",
            "nomeed",
            "nomaad",
            "sparrk",
            "hayv",
            "aarro",
            "lans",
            "lansdb",
            "beem",
            "beeam",
            "flsk",
            "flaask",
        ] {
            assert!(
                !is_in_dictionary(distortion),
                "{distortion:?} is a distortion — must NOT be in dictionary"
            );
        }
    }
}
