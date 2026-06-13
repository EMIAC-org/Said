//! Backend prompt builder — a thin adapter over the shared
//! [`said_core::polish::prompt`].
//!
//! The voice-polish prompt template, block assembly, and script-fidelity rules
//! now live in `said-core` so the local backend and the control-plane server
//! run exactly one copy and can never drift apart again. This module:
//!   * re-exports the shared types and pure builders unchanged,
//!   * keeps the store-coupled `VocabTerm` → `VocabEntry` converters local
//!     (they read `crate::store::vocabulary::VocabTerm`),
//!   * wraps the `&Preferences` / `&[Correction]` entry points so every existing
//!     call site stays byte-identical and the backend's real `is_common_word`
//!     guard is injected into the shared builder.

use crate::store::{corrections::Correction, prefs::Preferences, vocabulary::VocabTerm};

pub use said_core::polish::prompt::{
    FormatPreference, RagExample, VOICE_PROMPT_BASE_VERSION, VOICE_PROMPT_KIND, VOICE_PROMPT_TITLE,
    VocabEntry, VocabResolution, build_message_polish_system_prompt,
    build_message_polish_user_message, build_refine_last_transform_prompt,
    build_refine_last_transform_user_message, build_tray_format_user_message,
    build_tray_system_prompt, build_tray_user_message, build_user_message,
    build_user_message_with_hints, build_voice_repair_system_prompt,
    build_voice_repair_user_message, default_voice_prompt_template, format_fewshot_block,
};
pub use said_core::polish::types::PolishPrefs;

/// The backend's real common-word guard, injected into the shared prompt
/// builder so vocab-alias filtering matches historical behavior exactly.
fn is_common(form: &str) -> bool {
    super::promotion_gate::is_common_word(form)
}

fn to_polish_prefs(p: &Preferences) -> PolishPrefs {
    PolishPrefs {
        output_language: p.output_language.clone(),
        tone_preset: p.tone_preset.clone(),
        custom_prompt: p.custom_prompt.clone(),
    }
}

fn to_core_corrections(corrections: &[Correction]) -> Vec<said_core::polish::types::Correction> {
    corrections
        .iter()
        .map(|c| said_core::polish::types::Correction {
            wrong: c.wrong.clone(),
            right: c.right.clone(),
            count: c.count,
        })
        .collect()
}

// ── &Preferences entry points (wrapped to inject is_common + lean types) ──────

pub fn build_system_prompt(
    prefs: &Preferences,
    rag_examples: &[RagExample],
    corrections: &[Correction],
) -> String {
    said_core::polish::prompt::build_system_prompt(
        &to_polish_prefs(prefs),
        rag_examples,
        &to_core_corrections(corrections),
        is_common,
    )
}

pub fn build_system_prompt_with_vocab(
    prefs: &Preferences,
    rag_examples: &[RagExample],
    corrections: &[Correction],
    vocabulary_terms: &[String],
) -> String {
    said_core::polish::prompt::build_system_prompt_with_vocab(
        &to_polish_prefs(prefs),
        rag_examples,
        &to_core_corrections(corrections),
        vocabulary_terms,
        is_common,
    )
}

pub fn build_system_prompt_with_vocab_entries(
    prefs: &Preferences,
    rag_examples: &[RagExample],
    corrections: &[Correction],
    vocabulary_entries: &[VocabEntry],
) -> String {
    said_core::polish::prompt::build_system_prompt_with_vocab_entries(
        &to_polish_prefs(prefs),
        rag_examples,
        &to_core_corrections(corrections),
        vocabulary_entries,
        is_common,
    )
}

pub fn render_voice_system_prompt_template(
    template: &str,
    prefs: &Preferences,
    rag_examples: &[RagExample],
    corrections: &[Correction],
    vocabulary_entries: &[VocabEntry],
) -> String {
    said_core::polish::prompt::render_voice_system_prompt_template(
        template,
        &to_polish_prefs(prefs),
        rag_examples,
        &to_core_corrections(corrections),
        vocabulary_entries,
        is_common,
    )
}

pub fn build_tray_format_system_prompt(
    vocab_entries: &[VocabEntry],
    corrections: &[Correction],
) -> String {
    said_core::polish::prompt::build_tray_format_system_prompt(
        vocab_entries,
        &to_core_corrections(corrections),
        is_common,
    )
}

// ── Store-coupled converters (kept local — they read store::VocabTerm) ────────

pub fn vocab_terms_to_entries(terms: Vec<VocabTerm>) -> Vec<VocabEntry> {
    terms
        .into_iter()
        .map(|v| VocabEntry {
            term: v.term,
            context: v.example_context,
            resolution: VocabResolution::Candidate,
            term_type: v.term_type,
            meaning: v.meaning,
            stt_aliases: vec![],
        })
        .collect()
}

pub fn resolved_vocab_terms_to_entries(terms: Vec<VocabTerm>) -> Vec<VocabEntry> {
    terms
        .into_iter()
        .map(|v| VocabEntry {
            term: v.term,
            context: v.example_context,
            resolution: VocabResolution::Resolved,
            term_type: v.term_type,
            meaning: v.meaning,
            stt_aliases: vec![],
        })
        .collect()
}

pub fn resolved_vocab_terms_to_entries_with_aliases(
    terms: Vec<VocabTerm>,
    alias_map: &std::collections::HashMap<String, Vec<(String, i64)>>,
) -> Vec<VocabEntry> {
    terms
        .into_iter()
        .map(|v| {
            let key = v.term.to_ascii_lowercase();
            let aliases = alias_map.get(&key).cloned().unwrap_or_default();
            VocabEntry {
                term: v.term,
                context: v.example_context,
                resolution: VocabResolution::Resolved,
                term_type: v.term_type,
                meaning: v.meaning,
                stt_aliases: aliases,
            }
        })
        .collect()
}
