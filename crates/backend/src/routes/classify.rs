//! POST /v1/classify-edit  —  Learning Pipeline v2
//!
//! Simplified LLM-driven edit classification. Replaces the 10-stage
//! deterministic pipeline with:
//!
//!   1. **Capture gate** (cheap): reject stale / clipboard / app-switched edits
//!   2. **Branch** — no-edit (reward active vocab), full deletion, or stale
//!   3. **Demotion** — unconditional negative signal for removed terms
//!   4. **LLM Analyzer** — single call classifies all changes with reasons
//!   5. **Save** — persist learnable changes by reason type
//!
//! The analyzer replaces: pre-filter, diff, phonetic triage, LLM classifier,
//! merge, and promotion gates — all in one structured LLM call.

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    AppState,
    llm::{
        analyzer::{self, AnalyzedChange, AnalyzerInput, ChangeReason, ExistingTerm},
        promotion_gate,
    },
    store::{
        corrections, history, prefs::get_prefs, stt_replacements, vocab_embeddings, vocab_fts,
        vocabulary,
    },
    stt::{background as stt_background, bias as stt_bias},
};

#[derive(Deserialize)]
pub struct ClassifyBody {
    pub recording_id: String,
    pub ai_output: String,
    pub user_kept: String,
    /// How the desktop captured the edit text.  Drives auto-promotion gating:
    ///   "ax" | "keystroke_verified" → high confidence, may auto-promote
    ///   "clipboard"                 → medium, may auto-promote with strict gates
    ///   "keystroke_only"            → LOW, store as pending only
    /// Missing/unknown values are treated as `"ax"` for backward compatibility.
    #[serde(default = "default_capture_method")]
    pub capture_method: String,
    /// Milliseconds elapsed between paste-completed and the captured edit.
    /// Used by the CAPTURE_ERROR pre-filter to reject edits that arrived too
    /// long after the paste (likely an unrelated edit, not a correction).
    /// Missing → 0 (treated as "no signal", does not trigger the gate).
    #[serde(default)]
    pub time_since_paste_ms: u64,
    /// True if the active app/window changed between paste and capture.
    /// Almost always means the user moved on; the captured text rarely
    /// belongs to our paste.
    #[serde(default)]
    pub app_switched: bool,
    /// True if the captured `user_kept` matches the contents of the user's
    /// clipboard at capture time.  Strong signal that what we read was the
    /// user pasting more text on top of our paste — not a typed edit.
    #[serde(default)]
    pub matches_clipboard: bool,
}

fn default_capture_method() -> String {
    "ax".to_string()
}

/// Maximum elapsed-since-paste before we treat the edit as unrelated to
/// our paste.  30 seconds is generous (covers slow human typing, longer
/// thinking pauses) without being unbounded.
const CAPTURE_STALE_MS: u64 = 30_000;

/// Stricter subset: captures whose source is an *atomic* read of a specific
/// text element. An AX read returning a value means it came from the targeted
/// element at that moment; a focus change after the read doesn't invalidate it.
fn is_high_confidence_capture(capture_method: &str) -> bool {
    matches!(capture_method, "ax" | "keystroke_verified")
}

#[derive(Serialize)]
pub struct ClassifyResponse {
    pub class: String,
    pub reason: String,
    pub pending_id: Option<String>,
    pub learned: bool,
    pub notify: bool,
    pub promoted_count: usize,
    pub is_repeat: bool,
    pub promoted_terms: Vec<String>,
    pub queued_terms: Vec<String>,
    /// Pass-through from the analyzer — each change the LLM identified.
    pub changes: Vec<AnalyzedChange>,
}

pub async fn classify(
    State(state): State<AppState>,
    Json(body): Json<ClassifyBody>,
) -> (StatusCode, Json<ClassifyResponse>) {
    // ── Step 1: Look up recording + preferences ──────────────────────────────
    let rec = match history::get_recording(&state.pool, &body.recording_id) {
        Some(r) => r,
        None => {
            warn!("[classify] recording {} not found", body.recording_id);
            return (
                StatusCode::NOT_FOUND,
                Json(empty_response("not_found", "recording not found")),
            );
        }
    };
    let transcript = rec.transcript;
    let prefs = get_prefs(&state.pool, &state.default_user_id);
    if prefs.as_ref().map(|p| !p.learning_enabled).unwrap_or(false) {
        info!(
            "[classify] learning disabled — skipping {}",
            body.recording_id
        );
        return (
            StatusCode::OK,
            Json(empty_response("no_edit", "learning disabled")),
        );
    }
    let output_language = prefs
        .as_ref()
        .map(|p| p.output_language.clone())
        .unwrap_or_else(|| "hinglish".into());

    // ── Capture-error gate ───────────────────────────────────────────────────
    // Reject obviously bad signals before spending LLM budget.
    if body.matches_clipboard {
        info!(
            "[classify] capture_error: kept text matches clipboard for {}",
            body.recording_id
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "no_edit",
                "capture_error: kept matches clipboard (user pasted)",
            )),
        );
    }
    if body.app_switched && !is_high_confidence_capture(&body.capture_method) {
        info!(
            "[classify] capture_error: app_switched + low-confidence capture ({:?}) for {}",
            body.capture_method, body.recording_id,
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "no_edit",
                "capture_error: app changed during low-confidence capture",
            )),
        );
    }

    // ── Step 2: Branch — no edit / full deletion / stale ─────────────────────

    // Stale capture: > 30s after paste
    if body.time_since_paste_ms > CAPTURE_STALE_MS {
        info!(
            "[classify] stale capture ({}ms after paste) for {}",
            body.time_since_paste_ms, body.recording_id,
        );
        return (
            StatusCode::OK,
            Json(empty_response("stale", "edit arrived > 30 s after paste")),
        );
    }

    // Full deletion: user cleared everything
    if body.user_kept.trim().is_empty() {
        info!("[classify] full deletion for {}", body.recording_id,);
        return (
            StatusCode::OK,
            Json(empty_response("full_deletion", "user deleted all text")),
        );
    }

    // No edit: polished output kept verbatim
    if body.ai_output.trim() == body.user_kept.trim() {
        // Positive reinforcement: bump weight of known vocab terms used in output
        let rewarded = vocabulary::reward_active_terms(
            &state.pool,
            &state.default_user_id,
            &body.ai_output,
            0.1,
        );
        if rewarded > 0 {
            info!(
                "[classify] no-edit reward: bumped {rewarded} active vocab term(s) for {}",
                body.recording_id,
            );
        }
        return (
            StatusCode::OK,
            Json(empty_response("no_edit", "no changes detected")),
        );
    }

    // ── Step 3: Demotion (unconditional on every edit) ───────────────────────
    let demoted = run_demotion_pass(&state, &body.ai_output, &body.user_kept);
    if demoted > 0 {
        info!("[classify] demoted {demoted} vocabulary term(s) on this edit");
    }

    // ── Step 4: Call LLM Analyzer ────────────────────────────────────────────
    // Try Codex (GPT-5.4-mini) first for smarter learning decisions,
    // fall back to Groq 8B if no OpenAI token is connected.
    let groq_key = prefs
        .as_ref()
        .and_then(|p| p.groq_api_key.clone())
        .or_else(|| std::env::var("GROQ_API_KEY").ok())
        .unwrap_or_default();
    let codex_token = {
        let pool_tok = state.pool.clone();
        let uid_tok = state.default_user_id.clone();
        tokio::task::spawn_blocking(move || {
            crate::store::openai_oauth::get_token(&pool_tok, &uid_tok)
        })
        .await
        .unwrap_or(None)
        .map(|t| t.access_token)
    };

    // Build existing_vocab for the analyzer context
    let top_vocab = vocabulary::top_terms(&state.pool, &state.default_user_id, 100);
    let existing_vocab: Vec<ExistingTerm> = top_vocab
        .iter()
        .map(|v| {
            let examples = if let Some(ref ctx) = v.example_context {
                vec![ctx.clone()]
            } else {
                vec![]
            };
            ExistingTerm {
                term: v.term.clone(),
                current_meaning: v.meaning.clone(),
                sighting_count: v.use_count,
                examples,
            }
        })
        .collect();

    let analyzer_input = AnalyzerInput {
        transcript: transcript.clone(),
        polished: body.ai_output.clone(),
        user_kept: body.user_kept.clone(),
        output_language: output_language.clone(),
        existing_vocab,
    };

    info!(
        "[classify] learning model: {} (codex_available={})",
        if codex_token.is_some() {
            "gpt-5.4-mini (Codex)"
        } else {
            "llama-3.1-8b (Groq)"
        },
        codex_token.is_some()
    );

    let analyzer_output = match analyzer::analyze_edit(
        &state.http_client,
        &groq_key,
        codex_token.as_deref(),
        &analyzer_input,
    )
    .await
    {
        Ok(output) => output,
        Err(e) => {
            warn!(
                "[classify] analyzer failed: {e} — skipping for {}",
                body.recording_id
            );
            return (
                StatusCode::OK,
                Json(empty_response(
                    "analyzer_unavailable",
                    &format!("analyzer error: {e}"),
                )),
            );
        }
    };

    // ── Step 5: Save learnable changes ───────────────────────────────────────
    let mut promoted_count = 0_usize;
    let mut promoted_terms: Vec<String> = Vec::new();
    let mut queued_terms: Vec<String> = Vec::new();
    let mut has_repeat = false;
    let mut learned = false;

    for change in &analyzer_output.changes {
        let corrected = change.corrected.trim();
        let original = change.original.trim();
        if corrected.is_empty() {
            continue;
        }

        // Reject full sentences — max 4 words for a vocab term
        if corrected.split_whitespace().count() > 4 {
            tracing::info!(
                "[classify] skipping term too long ({} words): {:?}",
                corrected.split_whitespace().count(),
                corrected
            );
            continue;
        }

        // If analyzer says don't learn but provides a refined meaning,
        // update the existing entry's meaning (deepening, not duplicating)
        if !change.should_learn {
            if let Some(ref meaning) = change.meaning {
                if !meaning.trim().is_empty() {
                    // Case-insensitive lookup for existing term
                    if let Some(existing) =
                        vocabulary::find_by_term_ci(&state.pool, &state.default_user_id, corrected)
                    {
                        vocabulary::update_meaning(
                            &state.pool,
                            &state.default_user_id,
                            &existing.term,
                            meaning,
                        );
                        tracing::info!(
                            "[classify] deepened meaning for existing term {:?}",
                            existing.term
                        );
                    }
                }
            }
            continue;
        }

        match change.reason {
            ChangeReason::SttError => {
                if promotion_gate::is_common_word(corrected) {
                    info!("[classify] STT_ERROR skipped — common word: {corrected:?}");
                    queued_terms.push(corrected.to_string());
                    continue;
                }
                if promotion_gate::is_numeric_junk(corrected) {
                    info!("[classify] STT_ERROR skipped — numeric junk: {corrected:?}");
                    continue;
                }
                let term_type = vocabulary::classify_term_type(corrected);
                if matches!(term_type, "phrase" | "other") {
                    info!(
                        "[classify] STT_ERROR skipped — not a proper noun (type={term_type}): {corrected:?}"
                    );
                    continue;
                }

                let (canonical_term, weight_bump) = if let Some(existing) =
                    vocabulary::find_by_term_ci(&state.pool, &state.default_user_id, corrected)
                {
                    has_repeat = true;
                    (existing.term.clone(), 0.5)
                } else {
                    (corrected.to_string(), 1.0)
                };

                let ctx = change
                    .context_example
                    .clone()
                    .or_else(|| surrounding_sentence(&body.user_kept, &canonical_term));

                if vocabulary::upsert_for_language_with_context(
                    &state.pool,
                    &state.default_user_id,
                    &canonical_term,
                    weight_bump,
                    "auto",
                    &output_language,
                    ctx.as_deref(),
                ) {
                    learned = true;
                    promoted_count += 1;
                    promoted_terms.push(canonical_term.clone());

                    // Sync FTS index
                    vocab_fts::upsert(
                        &state.pool,
                        &state.default_user_id,
                        &canonical_term,
                        ctx.as_deref(),
                    );

                    // Fire-and-forget: embed + meaning refresh
                    spawn_vocab_embedding(
                        state.clone(),
                        canonical_term.clone(),
                        ctx.clone(),
                        codex_token.clone(),
                    );
                }

                // ── Update meaning if provided ───────────────────────────
                if let Some(ref meaning) = change.meaning {
                    if !meaning.trim().is_empty() {
                        vocabulary::update_meaning(
                            &state.pool,
                            &state.default_user_id,
                            &canonical_term,
                            meaning,
                        );
                    }
                }

                // ── STT replacement aliases ──────────────────────────────
                let aliases_written = stt_replacements::upsert_aliases_for_language(
                    &state.pool,
                    &state.default_user_id,
                    original,
                    original,
                    &canonical_term,
                    1.0,
                    &output_language,
                );
                if aliases_written > 0 {
                    promoted_count += aliases_written;
                }

                // ── Auto-classify term type ──────────────────────────────
                // classify_term_type is already called inside upsert, but
                // we call it here for any term that already existed without
                // a type classification.
                let _ = vocabulary::classify_term_type(corrected);
            }

            ChangeReason::PolishError => {
                // Store as a correction rule: wrong → right
                let wrong = original.to_ascii_lowercase();
                if !wrong.is_empty() && wrong != corrected.to_ascii_lowercase() {
                    corrections::upsert(
                        &state.pool,
                        &state.default_user_id,
                        &[(wrong, corrected.to_ascii_lowercase())],
                    );
                    learned = true;
                    promoted_count += 1;
                    promoted_terms.push(corrected.to_string());
                }
            }

            ChangeReason::FormatPreference => {
                // Store as a correction rule so the polish prompt picks it up.
                // e.g. "8am" → "8:00 AM"
                let wrong = original.to_ascii_lowercase();
                if !wrong.is_empty() && wrong != corrected.to_ascii_lowercase() {
                    corrections::upsert(
                        &state.pool,
                        &state.default_user_id,
                        &[(wrong, corrected.to_lowercase())],
                    );
                    learned = true;
                    promoted_count += 1;
                    promoted_terms.push(corrected.to_string());
                }
            }

            ChangeReason::StylePreference | ChangeReason::StructuralRewrite => {
                // Not learnable — intentional no-op.
            }
        }
    }

    // Invalidate lexicon cache if any corrections or stt_replacements were written
    if learned {
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    }

    let notify = learned && promoted_count > 0;

    info!(
        "[classify] {} overall={} changes={} promoted={} notify={} learned={}",
        body.recording_id,
        analyzer_output.overall_class,
        analyzer_output.changes.len(),
        promoted_count,
        notify,
        learned,
    );

    (
        StatusCode::OK,
        Json(ClassifyResponse {
            class: analyzer_output.overall_class,
            reason: format!(
                "analyzer identified {} change(s)",
                analyzer_output.changes.len()
            ),
            pending_id: None,
            learned,
            notify,
            promoted_count,
            is_repeat: has_repeat,
            promoted_terms,
            queued_terms,
            changes: analyzer_output.changes,
        }),
    )
}

/// Demote vocabulary terms that appear in polish but are removed in user_kept.
fn run_demotion_pass(state: &AppState, polish: &str, user_kept: &str) -> usize {
    let polish_lower = polish.to_ascii_lowercase();
    let kept_lower = user_kept.to_ascii_lowercase();
    let vocab = vocabulary::top_terms(&state.pool, &state.default_user_id, 1000);

    let mut demoted = 0_usize;
    for v in vocab {
        let term_lower = v.term.to_ascii_lowercase();
        if polish_lower.contains(&term_lower)
            && !kept_lower.contains(&term_lower)
            && vocabulary::demote(&state.pool, &state.default_user_id, &v.term, 1.0)
        {
            demoted += 1;
        }
    }
    demoted
}

/// Fire-and-forget: embed a newly learned vocab term (with its context)
/// and persist the vector so polish-time relevance retrieval can find it.
fn spawn_vocab_embedding(
    state: AppState,
    term: String,
    example_context: Option<String>,
    codex_token_for_meaning: Option<String>,
) {
    tokio::spawn(async move {
        let _guard = crate::bg_task_guard();
        if state.watchdog.is_shedding() {
            tracing::debug!("[bg] embed for {term:?} skipped — watchdog shedding load");
            return;
        }
        info!(
            "[bg] embed for {term:?} started (active={})",
            crate::BG_TASK_COUNT.load(std::sync::atomic::Ordering::Relaxed)
        );
        let bg_start = std::time::Instant::now();
        let Some(prefs) = get_prefs(&state.pool, &state.default_user_id) else {
            return;
        };
        if !prefs.learning_enabled {
            return;
        }
        let key = prefs
            .gemini_api_key
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        if key.is_empty() {
            return;
        }
        let text = match &example_context {
            Some(ctx) if !ctx.trim().is_empty() => format!("{term}. {ctx}"),
            _ => term.clone(),
        };
        let Some(embedding) =
            crate::embedder::gemini::embed(&state.http_client, &state.pool, &text, &key).await
        else {
            return;
        };
        let pool = state.pool.clone();
        let uid = state.default_user_id.clone();
        let term2 = term.clone();
        let example = text.clone();
        let blocking = tokio::task::spawn_blocking(move || {
            vocab_embeddings::record_example_and_recentre(
                &pool, &uid, &term2, &embedding, &example,
            );
            vocabulary::bump_examples_since_meaning(&pool, &uid, &term2);
            let spread = vocab_embeddings::cluster_spread(&pool, &uid, &term2);
            if spread > 0.5 {
                tracing::info!(
                    "[vocab-emb] high cluster spread for {term2:?}: {:.2} — bimodal usage",
                    spread,
                );
            }
        });
        let _ = blocking.await;
        info!(
            "[bg] embed for {term:?} done in {}ms",
            bg_start.elapsed().as_millis()
        );
        spawn_meaning_refresh(
            state,
            term,
            example_context.unwrap_or_default(),
            codex_token_for_meaning.clone(),
        );
    });
}

/// Fire-and-forget: refresh a term's distilled meaning when needed.
fn spawn_meaning_refresh(
    state: AppState,
    term: String,
    latest_example: String,
    codex_token: Option<String>,
) {
    tokio::spawn(async move {
        let _guard = crate::bg_task_guard();
        if state.watchdog.is_shedding() {
            tracing::debug!("[bg] meaning for {term:?} skipped — watchdog shedding load");
            return;
        }
        let uid = state.default_user_id.clone();
        let pool = state.pool.clone();

        if !vocabulary::meaning_needs_refresh(&pool, &uid, &term) {
            return;
        }
        info!(
            "[bg] meaning for {term:?} started (active={})",
            crate::BG_TASK_COUNT.load(std::sync::atomic::Ordering::Relaxed)
        );
        let bg_start = std::time::Instant::now();

        let prefs = get_prefs(&pool, &uid);
        let groq_key = prefs
            .as_ref()
            .and_then(|p| p.groq_api_key.clone())
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .unwrap_or_default();
        let openai_key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
        if groq_key.is_empty() && openai_key.is_empty() {
            warn!(
                "[meaning] no Groq key AND no OPENAI_API_KEY — meaning will stay NULL for {term:?}"
            );
            return;
        }

        let current = vocabulary::get_meaning(&pool, &uid, &term);
        let result = match &current {
            None => {
                let example = if latest_example.trim().is_empty() {
                    term.clone()
                } else {
                    latest_example.clone()
                };
                crate::llm::meaning::generate_initial(
                    &state.http_client,
                    &groq_key,
                    &openai_key,
                    codex_token.as_deref(),
                    &term,
                    &example,
                )
                .await
            }
            Some(prev) => {
                let examples = vocab_embeddings::support_example_texts(&pool, &uid, &term, 4);
                if examples.is_empty() {
                    None
                } else {
                    crate::llm::meaning::refine(
                        &state.http_client,
                        &groq_key,
                        &openai_key,
                        codex_token.as_deref(),
                        &term,
                        prev,
                        &examples,
                    )
                    .await
                }
            }
        };

        if let Some(new_meaning) = result {
            let pool2 = pool.clone();
            let uid2 = uid.clone();
            let term2 = term.clone();
            let _ = tokio::task::spawn_blocking(move || {
                vocabulary::update_meaning(&pool2, &uid2, &term2, &new_meaning);
            })
            .await;
        }
        info!(
            "[bg] meaning for {term:?} done in {}ms",
            bg_start.elapsed().as_millis()
        );
    });
}

/// Find the sentence inside `text` that contains `term`, returning it
/// trimmed. Sentence boundaries: '.', '!', '?', '\n'.
fn surrounding_sentence(text: &str, term: &str) -> Option<String> {
    let term_l = term.to_ascii_lowercase();
    if term_l.is_empty() {
        return None;
    }
    let text_l = text.to_ascii_lowercase();
    let pos = text_l.find(&term_l)?;
    let start = text[..pos]
        .rfind(|c: char| matches!(c, '.' | '!' | '?' | '\n'))
        .map(|i| i + 1)
        .unwrap_or(0);
    let after_term = pos + term.len();
    let end = text[after_term..]
        .find(|c: char| matches!(c, '.' | '!' | '?' | '\n'))
        .map(|i| after_term + i + 1)
        .unwrap_or(text.len());
    let snippet = text[start..end].trim();
    if snippet.is_empty() {
        None
    } else {
        Some(snippet.to_string())
    }
}

fn empty_response(class: &str, reason: &str) -> ClassifyResponse {
    ClassifyResponse {
        class: class.to_string(),
        reason: reason.to_string(),
        pending_id: None,
        learned: false,
        notify: false,
        promoted_count: 0,
        is_repeat: false,
        promoted_terms: vec![],
        queued_terms: vec![],
        changes: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrounding_sentence_returns_the_containing_clause() {
        let text = "Hello there. MACOBS ka IPO ka 12 hazaar batana. Then bye.";
        let got = surrounding_sentence(text, "MACOBS");
        assert_eq!(got.as_deref(), Some("MACOBS ka IPO ka 12 hazaar batana."));
    }

    #[test]
    fn surrounding_sentence_handles_no_terminator() {
        let text = "MACOBS ka IPO ka 12 hazaar batana";
        let got = surrounding_sentence(text, "MACOBS");
        assert_eq!(got.as_deref(), Some("MACOBS ka IPO ka 12 hazaar batana"));
    }

    #[test]
    fn surrounding_sentence_handles_term_at_start() {
        let text = "MACOBS! Then more text.";
        let got = surrounding_sentence(text, "MACOBS");
        assert_eq!(got.as_deref(), Some("MACOBS!"));
    }

    #[test]
    fn surrounding_sentence_returns_none_for_missing_term() {
        assert!(surrounding_sentence("nothing here", "MACOBS").is_none());
    }

    #[test]
    fn surrounding_sentence_is_case_insensitive() {
        let text = "Hello. macobs ka IPO. Bye.";
        let got = surrounding_sentence(text, "MACOBS");
        assert_eq!(got.as_deref(), Some("macobs ka IPO."));
    }
}
