use std::collections::VecDeque;

const MAX_EVENTS: usize = 128;
const MAX_CANDIDATES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    SttEntityError,
    SttPhraseError,
    FormattingPreference,
    StylePreference,
    ContentPreservationFix,
    TranslationFix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceBand {
    Low,
    Medium,
    High,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKind {
    Entity,
    Phrase,
    Guard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStage {
    Candidate,
    Contextual,
    Stable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleStats {
    pub accepts: u32,
    pub reverts: u32,
    pub dropped_word_penalties: u32,
    pub translation_penalties: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFingerprint {
    pub left_context: Vec<String>,
    pub right_context: Vec<String>,
    pub source_sentence: String,
    pub target_sentence: String,
    pub app_hint: Option<String>,
    pub language_mode: String,
    pub output_language: String,
    pub confidence_band: ConfidenceBand,
    pub topic_hints: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionObservation {
    pub source_span: String,
    pub target_span: String,
    pub error_class: ErrorClass,
    pub context: ContextFingerprint,
    pub why_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrectionEvent {
    pub event_id: u64,
    pub observed_at_ms: i64,
    pub observation: CorrectionObservation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LearnedRule {
    pub kind: MemoryKind,
    pub stage: RuleStage,
    pub source_span: String,
    pub target_span: String,
    pub error_class: ErrorClass,
    pub seed_context: ContextFingerprint,
    pub why_summary: Option<String>,
    pub evidence_count: u32,
    pub stats: RuleStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualCorrectionInput {
    pub transcript: String,
    pub polished: String,
    pub corrected: String,
    pub app_hint: Option<String>,
    pub language_mode: String,
    pub output_language: String,
    pub confidence_band: ConfidenceBand,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicySnapshot {
    pub event_count: usize,
    pub candidate_count: usize,
    pub entity_count: usize,
    pub phrase_count: usize,
    pub guard_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserveOutcome {
    pub event: CorrectionEvent,
    pub created_candidate: bool,
    pub candidate_index: usize,
    pub candidate_evidence: u32,
}

#[derive(Debug, Clone, Default)]
pub struct RuntimePolicy {
    events: VecDeque<CorrectionEvent>,
    candidates: Vec<LearnedRule>,
}

impl RuntimePolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn observe(&mut self, observation: CorrectionObservation) -> ObserveOutcome {
        let event = CorrectionEvent {
            event_id: self.events.back().map(|e| e.event_id + 1).unwrap_or(1),
            observed_at_ms: now_ms(),
            observation: observation.clone(),
        };
        self.push_event(event.clone());

        if let Some(idx) = self.find_candidate(&observation) {
            let rule = &mut self.candidates[idx];
            rule.evidence_count += 1;
            return ObserveOutcome {
                event,
                created_candidate: false,
                candidate_index: idx,
                candidate_evidence: rule.evidence_count,
            };
        }

        if self.candidates.len() >= MAX_CANDIDATES {
            self.candidates.remove(0);
        }

        let rule = LearnedRule {
            kind: classify_memory_kind(&observation),
            stage: RuleStage::Candidate,
            source_span: observation.source_span.clone(),
            target_span: observation.target_span.clone(),
            error_class: observation.error_class,
            seed_context: observation.context.clone(),
            why_summary: observation.why_summary.clone(),
            evidence_count: 1,
            stats: RuleStats::default(),
        };
        self.candidates.push(rule);
        let idx = self.candidates.len() - 1;

        ObserveOutcome {
            event,
            created_candidate: true,
            candidate_index: idx,
            candidate_evidence: 1,
        }
    }

    pub fn snapshot(&self) -> PolicySnapshot {
        let mut entity_count = 0usize;
        let mut phrase_count = 0usize;
        let mut guard_count = 0usize;
        for rule in &self.candidates {
            match rule.kind {
                MemoryKind::Entity => entity_count += 1,
                MemoryKind::Phrase => phrase_count += 1,
                MemoryKind::Guard => guard_count += 1,
            }
        }
        PolicySnapshot {
            event_count: self.events.len(),
            candidate_count: self.candidates.len(),
            entity_count,
            phrase_count,
            guard_count,
        }
    }

    pub fn candidates(&self) -> &[LearnedRule] {
        &self.candidates
    }

    pub fn prompt_block(&self) -> Option<String> {
        if self.candidates.is_empty() {
            return None;
        }

        let lines = self
            .candidates
            .iter()
            .take(8)
            .map(|rule| match rule.kind {
                MemoryKind::Entity | MemoryKind::Phrase => {
                    format!(
                        "- In contexts like \"{}\", prefer \"{}\" over \"{}\"",
                        compact_sentence(&rule.seed_context.source_sentence),
                        rule.target_span,
                        rule.source_span
                    )
                }
                MemoryKind::Guard => {
                    format!(
                        "- Guard: avoid repeating the mistake around \"{}\"",
                        compact_sentence(&rule.seed_context.source_sentence)
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        Some(format!(
            "<runtime_policy>\n\
             Session-only learning hints from this run. Use them only when the same local context clearly appears.\n\
             {lines}\n\
             </runtime_policy>"
        ))
    }

    fn push_event(&mut self, event: CorrectionEvent) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    fn find_candidate(&self, observation: &CorrectionObservation) -> Option<usize> {
        self.candidates.iter().position(|rule| {
            rule.source_span
                .eq_ignore_ascii_case(&observation.source_span)
                && rule
                    .target_span
                    .eq_ignore_ascii_case(&observation.target_span)
                && rule.error_class == observation.error_class
                && rule.seed_context.language_mode == observation.context.language_mode
                && rule.seed_context.output_language == observation.context.output_language
                && rule.seed_context.app_hint == observation.context.app_hint
        })
    }
}

pub fn observation_from_manual_correction(
    input: ManualCorrectionInput,
) -> Option<CorrectionObservation> {
    let polished_tokens = tokenize(&input.polished);
    let corrected_tokens = tokenize(&input.corrected);

    if polished_tokens.is_empty() || corrected_tokens.is_empty() {
        return None;
    }

    let prefix = common_prefix_len(&polished_tokens, &corrected_tokens);
    let suffix = common_suffix_len(&polished_tokens[prefix..], &corrected_tokens[prefix..]);

    let mut src_end = polished_tokens.len().saturating_sub(suffix);
    let mut dst_end = corrected_tokens.len().saturating_sub(suffix);
    if prefix >= src_end || prefix >= dst_end {
        return None;
    }

    if src_end - prefix == 1
        && dst_end - prefix == 1
        && src_end < polished_tokens.len()
        && dst_end < corrected_tokens.len()
        && normalize_for_compare(polished_tokens[src_end])
            == normalize_for_compare(corrected_tokens[dst_end])
    {
        src_end += 1;
        dst_end += 1;
    }

    let source_span = polished_tokens[prefix..src_end]
        .join(" ")
        .trim()
        .to_string();
    let target_span = corrected_tokens[prefix..dst_end]
        .join(" ")
        .trim()
        .to_string();
    if source_span.is_empty() || target_span.is_empty() {
        return None;
    }

    let left_context = polished_tokens[prefix.saturating_sub(4)..prefix]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let right_context = polished_tokens[src_end..(src_end + 4).min(polished_tokens.len())]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    let error_class = classify_error(&input.transcript, &source_span, &target_span);
    let topic_hints = derive_topic_hints(&input.transcript, &source_span, &target_span);

    Some(CorrectionObservation {
        source_span,
        target_span,
        error_class,
        context: ContextFingerprint {
            left_context,
            right_context,
            source_sentence: input.polished,
            target_sentence: input.corrected,
            app_hint: input.app_hint,
            language_mode: input.language_mode,
            output_language: input.output_language,
            confidence_band: input.confidence_band,
            topic_hints,
        },
        why_summary: None,
    })
}

fn classify_memory_kind(observation: &CorrectionObservation) -> MemoryKind {
    match observation.error_class {
        ErrorClass::ContentPreservationFix | ErrorClass::TranslationFix => MemoryKind::Guard,
        ErrorClass::FormattingPreference | ErrorClass::StylePreference => {
            if token_count(&observation.target_span) <= 2 {
                MemoryKind::Phrase
            } else {
                MemoryKind::Guard
            }
        }
        ErrorClass::SttEntityError => MemoryKind::Entity,
        ErrorClass::SttPhraseError => {
            if looks_like_entity(&observation.target_span) {
                MemoryKind::Entity
            } else {
                MemoryKind::Phrase
            }
        }
    }
}

fn classify_error(transcript: &str, source_span: &str, target_span: &str) -> ErrorClass {
    let source_norm = normalize_for_compare(source_span);
    let target_norm = normalize_for_compare(target_span);
    if source_norm == target_norm {
        return ErrorClass::FormattingPreference;
    }

    let target_is_entity = looks_like_entity(target_span);
    let source_is_entity = looks_like_entity(source_span);
    if target_is_entity || source_is_entity {
        return ErrorClass::SttEntityError;
    }

    if transcript.contains('[') && transcript.contains('?') {
        return ErrorClass::SttPhraseError;
    }

    ErrorClass::SttPhraseError
}

fn token_count(s: &str) -> usize {
    s.split_whitespace().count()
}

fn looks_like_entity(s: &str) -> bool {
    let trimmed = s.trim();
    let has_mixed_case = trimmed.chars().any(|c| c.is_ascii_uppercase());
    let is_short = token_count(trimmed) <= 3;
    let has_digit = trimmed.chars().any(|c| c.is_ascii_digit());
    (has_mixed_case || has_digit) && is_short
}

fn compact_sentence(s: &str) -> String {
    let trimmed = s.trim();
    const MAX: usize = 64;
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(MAX).collect();
    format!("{head}...")
}

fn tokenize(s: &str) -> Vec<&str> {
    s.split_whitespace().collect()
}

fn common_prefix_len(a: &[&str], b: &[&str]) -> usize {
    let mut i = 0usize;
    while i < a.len() && i < b.len() && normalize_for_compare(a[i]) == normalize_for_compare(b[i]) {
        i += 1;
    }
    i
}

fn common_suffix_len(a: &[&str], b: &[&str]) -> usize {
    let mut i = 0usize;
    while i < a.len()
        && i < b.len()
        && normalize_for_compare(a[a.len() - 1 - i]) == normalize_for_compare(b[b.len() - 1 - i])
    {
        i += 1;
    }
    i
}

fn normalize_for_compare(s: &str) -> String {
    s.trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

fn derive_topic_hints(transcript: &str, source_span: &str, target_span: &str) -> Vec<String> {
    let mut hints = Vec::new();
    for token in transcript
        .split_whitespace()
        .filter(|t| t.len() > 3)
        .take(8)
    {
        let norm = normalize_for_compare(token);
        if norm.is_empty()
            || norm == normalize_for_compare(source_span)
            || norm == normalize_for_compare(target_span)
        {
            continue;
        }
        if !hints.contains(&norm) {
            hints.push(norm);
        }
    }
    hints
}

fn now_ms() -> i64 {
    use std::time::UNIX_EPOCH;
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_observation() -> CorrectionObservation {
        CorrectionObservation {
            source_span: "main corps".into(),
            target_span: "MACOBS".into(),
            error_class: ErrorClass::SttEntityError,
            context: ContextFingerprint {
                left_context: vec!["legal".into(), "team".into()],
                right_context: vec!["dashboard".into()],
                source_sentence: "legal team from main corps dashboard".into(),
                target_sentence: "legal team from MACOBS dashboard".into(),
                app_hint: Some("slack".into()),
                language_mode: "auto".into(),
                output_language: "hinglish".into(),
                confidence_band: ConfidenceBand::Low,
                topic_hints: vec!["legal".into(), "dashboard".into()],
            },
            why_summary: Some("Entity correction in company/dashboard context.".into()),
        }
    }

    #[test]
    fn observe_creates_candidate() {
        let mut policy = RuntimePolicy::new();
        let outcome = policy.observe(sample_observation());
        assert!(outcome.created_candidate);
        assert_eq!(outcome.candidate_evidence, 1);
        assert_eq!(policy.snapshot().candidate_count, 1);
        assert_eq!(policy.snapshot().entity_count, 1);
    }

    #[test]
    fn repeated_observation_increments_evidence() {
        let mut policy = RuntimePolicy::new();
        policy.observe(sample_observation());
        let second = policy.observe(sample_observation());
        assert!(!second.created_candidate);
        assert_eq!(second.candidate_evidence, 2);
        assert_eq!(policy.snapshot().candidate_count, 1);
    }

    #[test]
    fn manual_correction_extracts_contextual_span() {
        let obs = observation_from_manual_correction(ManualCorrectionInput {
            transcript: "MAH technologies ki listing ka kaam batao".into(),
            polished: "MAH technologies ki listing ka kaam batao.".into(),
            corrected: "MII Technologies ki listing ka kaam batao.".into(),
            app_hint: Some("terminal".into()),
            language_mode: "auto".into(),
            output_language: "hinglish".into(),
            confidence_band: ConfidenceBand::Unknown,
        })
        .expect("observation");

        assert_eq!(obs.source_span, "MAH technologies");
        assert_eq!(obs.target_span, "MII Technologies");
        assert_eq!(obs.context.left_context.len(), 0);
        assert_eq!(obs.context.right_context[0], "ki");
    }
}
