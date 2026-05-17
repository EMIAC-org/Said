use std::collections::{HashMap, HashSet};

use crate::llm::phonetics;
use crate::store::{stt_replacements::ApplyResult, vocabulary::VocabTerm};

#[derive(Debug, Clone)]
pub struct ResolutionResult {
    pub transcript: String,
    pub resolved_terms: Vec<VocabTerm>,
    pub candidate_terms: Vec<VocabTerm>,
    pub alias_match_count: usize,
    pub context_match_count: usize,
}

pub fn resolve_for_prompt(
    transcript: &str,
    selected_terms: &[VocabTerm],
    all_terms: &[VocabTerm],
    alias_result: &ApplyResult,
) -> ResolutionResult {
    let by_term_lower: HashMap<String, &VocabTerm> = all_terms
        .iter()
        .map(|t| (t.term.to_ascii_lowercase(), t))
        .collect();

    let mut resolved_keys: HashSet<String> = HashSet::new();
    let mut resolved_terms: Vec<VocabTerm> = Vec::new();

    for m in &alias_result.matches {
        let key = m.correct_form.to_ascii_lowercase();
        if let Some(term) = by_term_lower.get(&key) {
            if resolved_keys.insert(key) {
                resolved_terms.push((*term).clone());
            }
        }
    }

    for term in selected_terms {
        if should_auto_resolve_exact_term(term) && contains_term_exactly(transcript, &term.term) {
            let key = term.term.to_ascii_lowercase();
            if resolved_keys.insert(key) {
                resolved_terms.push(term.clone());
            }
        }
    }

    let mut resolved_text = transcript.to_string();
    let mut context_match_count = 0;

    for term in selected_terms {
        let key = term.term.to_ascii_lowercase();
        if resolved_keys.contains(&key) {
            continue;
        }
        if let Some(next_text) = try_context_resolve(&resolved_text, term) {
            resolved_text = next_text;
            resolved_keys.insert(key);
            resolved_terms.push(term.clone());
            context_match_count += 1;
        }
    }

    let candidate_terms = selected_terms
        .iter()
        .filter(|t| !resolved_keys.contains(&t.term.to_ascii_lowercase()))
        .cloned()
        .collect();

    ResolutionResult {
        transcript: resolved_text,
        resolved_terms,
        candidate_terms,
        alias_match_count: alias_result.matches.len(),
        context_match_count,
    }
}

fn try_context_resolve(transcript: &str, term: &VocabTerm) -> Option<String> {
    let context = term.example_context.as_deref()?.trim();
    if context.is_empty() {
        return None;
    }
    if matches!(term.term_type.as_deref(), Some("other") | None) {
        return None;
    }

    let anchors = strong_anchor_tokens(context, &term.term);
    if anchors.len() < 2 {
        return None;
    }

    let chunks = split_chunks(transcript);
    let cores: Vec<String> = chunks
        .iter()
        .map(|chunk| word_core(chunk).to_ascii_lowercase())
        .collect();
    let token_positions: Vec<usize> = cores
        .iter()
        .enumerate()
        .filter_map(|(i, core)| (!core.is_empty()).then_some(i))
        .collect();

    let anchor_positions: Vec<usize> = cores
        .iter()
        .enumerate()
        .filter_map(|(i, core)| anchors.contains(core).then_some(i))
        .collect();
    if anchor_positions.len() < 2 {
        return None;
    }

    let max_words = match term.term_type.as_deref() {
        Some("phrase") => term.term.split_whitespace().count().clamp(1, 4),
        _ => 3,
    };

    let mut best: Option<(f64, usize, usize)> = None;
    for start_idx in 0..token_positions.len() {
        let start = token_positions[start_idx];
        for word_len in 1..=max_words {
            let end_token_idx = start_idx + word_len;
            if end_token_idx > token_positions.len() {
                break;
            }
            let end = token_positions[end_token_idx - 1];
            let phrase = token_positions[start_idx..end_token_idx]
                .iter()
                .map(|&idx| cores[idx].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if phrase.is_empty() || phrase == term.term.to_ascii_lowercase() {
                continue;
            }

            let sim = phonetics::similarity(&phrase, &term.term);
            let min_sim = match term.term_type.as_deref() {
                Some("acronym") => 0.45,
                Some("code_identifier") => 0.60,
                Some("brand") | Some("proper_noun") => 0.70,
                _ => 0.75,
            };
            if sim < min_sim {
                continue;
            }

            let nearest_anchor = anchor_positions
                .iter()
                .map(|&pos| {
                    if pos < start {
                        start - pos
                    } else if pos > end {
                        pos - end
                    } else {
                        0
                    }
                })
                .min()
                .unwrap_or(usize::MAX);
            if nearest_anchor > 6 {
                continue;
            }

            let score = sim - (nearest_anchor as f64 * 0.02);
            if best
                .as_ref()
                .map(|(best_score, _, _)| score > *best_score)
                .unwrap_or(true)
            {
                best = Some((score, start, end));
            }
        }
    }

    let (_, start, end) = best?;
    Some(replace_span(&chunks, start, end, &term.term))
}

fn contains_term_exactly(text: &str, term: &str) -> bool {
    let target_tokens: Vec<String> = term
        .split_whitespace()
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if target_tokens.is_empty() {
        return false;
    }
    let cores: Vec<String> = split_chunks(text)
        .into_iter()
        .map(word_core)
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    if target_tokens.len() == 1 {
        return cores.iter().any(|w| w == &target_tokens[0]);
    }
    cores
        .windows(target_tokens.len())
        .any(|window| window == target_tokens.as_slice())
}

fn should_auto_resolve_exact_term(term: &VocabTerm) -> bool {
    if term.source == "starred" {
        return true;
    }
    match term.term_type.as_deref() {
        Some("acronym" | "code_identifier" | "brand" | "proper_noun" | "phrase") => true,
        Some("other") | None => phonetics::jargon_score(&term.term) >= 0.50,
        Some(_) => true,
    }
}

fn strong_anchor_tokens(example_context: &str, term: &str) -> HashSet<String> {
    let term_lower = term.to_ascii_lowercase();
    split_chunks(example_context)
        .into_iter()
        .map(word_core)
        .filter(|w| !w.is_empty())
        .map(|w| w.to_ascii_lowercase())
        .filter(|w| *w != term_lower)
        .filter(|w| w.chars().any(|c| c.is_ascii_digit()) || w.chars().count() >= 3)
        .collect()
}

fn replace_span(chunks: &[&str], start: usize, end: usize, canonical: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < chunks.len() {
        if i == start {
            let first = chunks[start];
            let last = chunks[end];
            let (lead, _) = split_punct(first);
            let (_, trail) = split_punct_trailing(last);
            out.push_str(lead);
            out.push_str(canonical);
            out.push_str(trail);
            i = end + 1;
            continue;
        }
        out.push_str(chunks[i]);
        i += 1;
    }
    out
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
    let split = chunk
        .find(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .unwrap_or(chunk.len());
    (&chunk[..split], &chunk[split..])
}

fn split_punct_trailing(chunk: &str) -> (&str, &str) {
    let split = chunk
        .rfind(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
        .map(|i| i + chunk[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(0);
    (&chunk[..split], &chunk[split..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::stt_replacements::{AppliedMatch, ApplyResult, MatchKind};

    fn vocab(
        term: &str,
        context: Option<&str>,
        term_type: Option<&str>,
        meaning: Option<&str>,
        source: &str,
    ) -> VocabTerm {
        VocabTerm {
            term: term.into(),
            weight: 1.0,
            use_count: 1,
            last_used: 0,
            source: source.into(),
            example_context: context.map(|s| s.into()),
            term_type: term_type.map(|s| s.into()),
            meaning: meaning.map(|s| s.into()),
        }
    }

    #[test]
    fn exact_alias_match_becomes_resolved_term() {
        let term = vocab(
            "n8n",
            Some("I use n8n for automation"),
            Some("code_identifier"),
            Some("Automation tool."),
            "auto",
        );
        let apply = ApplyResult {
            text: "I use n8n daily".into(),
            matches: vec![AppliedMatch {
                transcript_form: "written".into(),
                correct_form: "n8n".into(),
                kind: MatchKind::Exact,
            }],
        };
        let out = resolve_for_prompt(
            &apply.text,
            std::slice::from_ref(&term),
            std::slice::from_ref(&term),
            &apply,
        );
        assert_eq!(out.transcript, "I use n8n daily");
        assert_eq!(out.resolved_terms.len(), 1);
        assert!(out.candidate_terms.is_empty());
    }

    #[test]
    fn phonetic_alias_match_becomes_resolved_term() {
        let term = vocab(
            "Aiden",
            Some("Aiden shipped the patch"),
            Some("proper_noun"),
            Some("A person's name."),
            "auto",
        );
        let apply = ApplyResult {
            text: "Aiden shipped the patch".into(),
            matches: vec![AppliedMatch {
                transcript_form: "aidan".into(),
                correct_form: "Aiden".into(),
                kind: MatchKind::Phonetic,
            }],
        };
        let out = resolve_for_prompt(
            &apply.text,
            std::slice::from_ref(&term),
            std::slice::from_ref(&term),
            &apply,
        );
        assert_eq!(out.resolved_terms.len(), 1);
        assert_eq!(out.alias_match_count, 1);
    }

    #[test]
    fn context_resolution_recovers_macobs_before_llm() {
        let term = vocab(
            "MACOBS",
            Some("MACOBS ka IPO ka 12 hazaar batana"),
            Some("acronym"),
            Some("Indian SME stock acronym."),
            "auto",
        );
        let apply = ApplyResult {
            text: "Main corps ka IPO ka 12 hazaar batana".into(),
            matches: vec![],
        };
        let out = resolve_for_prompt(
            &apply.text,
            std::slice::from_ref(&term),
            std::slice::from_ref(&term),
            &apply,
        );
        assert_eq!(out.transcript, "MACOBS ka IPO ka 12 hazaar batana");
        assert_eq!(out.context_match_count, 1);
        assert_eq!(out.resolved_terms[0].term, "MACOBS");
    }

    #[test]
    fn semantic_neighbor_without_anchor_stays_unresolved() {
        let term = vocab(
            "tembeess",
            Some("tembeess Friday team meeting"),
            Some("proper_noun"),
            Some("Internal project term."),
            "auto",
        );
        let apply = ApplyResult {
            text: "what time is it".into(),
            matches: vec![],
        };
        let out = resolve_for_prompt(
            &apply.text,
            std::slice::from_ref(&term),
            std::slice::from_ref(&term),
            &apply,
        );
        assert_eq!(out.transcript, "what time is it");
        assert!(out.resolved_terms.is_empty());
        assert_eq!(out.candidate_terms.len(), 1);
    }

    #[test]
    fn common_word_collision_is_not_context_resolved() {
        let term = vocab(
            "time",
            Some("time tracking issue in sprint"),
            Some("other"),
            Some("Common word."),
            "auto",
        );
        let apply = ApplyResult {
            text: "what time is it".into(),
            matches: vec![],
        };
        let out = resolve_for_prompt(
            &apply.text,
            std::slice::from_ref(&term),
            std::slice::from_ref(&term),
            &apply,
        );
        assert_eq!(out.transcript, "what time is it");
        assert!(out.resolved_terms.is_empty());
    }
}
