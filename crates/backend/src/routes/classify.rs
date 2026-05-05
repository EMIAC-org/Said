//! POST /v1/classify-edit
//!
//! Four-stage learning pipeline:
//!
//!   1. **Pre-filter** (cheap, no LLM): drop no-ops, USER_REWRITE shapes
//!      (huge length deltas, polish-kept-verbatim-with-prefix), and script
//!      mismatches. Most edits exit here without an API call.
//!
//!   2. **Diff** (deterministic): compute structural hunks from polish vs
//!      user_kept. Each hunk carries `(transcript_window, polish_window,
//!      kept_window)` taken directly from the texts. The classifier in stage
//!      3 will *label* these — it can never *invent* candidates.
//!
//!   3. **Classify** (LLM as labeler): hand the hunks to Groq, which assigns
//!      one class label per hunk + an overall class for the edit. Strict
//!      JSON schema, missing labels default to USER_REPHRASE.
//!
//!   4. **Promotion gates** (data-driven, defense-in-depth): for each STT_ERROR
//!      / POLISH_ERROR labelled hunk, verify the proposed correct_form
//!      actually appears in user_kept, has the right script for the user's
//!      output language, and is plausibly an STT mishearing (phonetic or
//!      jargon evidence) before promoting to the vocabulary / replacement
//!      stores.
//!
//! The architectural invariant: a learning artifact is written ONLY when a
//! diff-derived hunk passes all four stages. The LLM cannot bypass this by
//! claiming a correction that doesn't exist in the actual edit text.

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    AppState,
    llm::{
        classifier,
        classifier::{EditClass, LabelledHunk},
        edit_diff,
        edit_diff::Hunk,
        meaning, phonetic_triage, phonetics, pre_filter, promotion_gate,
    },
    store::{
        corrections, history, pending_edits, pending_promotions, prefs::get_prefs,
        stt_replacements, vocab_embeddings, vocab_fts, vocabulary,
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

/// Capture-confidence policy.  Maps the wire-level `capture_method` string
/// into a single bool: "is this capture trustworthy enough to auto-promote?"
fn capture_allows_auto_promote(capture_method: &str) -> bool {
    matches!(capture_method, "ax" | "keystroke_verified" | "clipboard")
}

/// Stricter subset of `capture_allows_auto_promote`: captures whose source
/// is an *atomic* read of a specific text element.  An AX read returning a
/// value means it came from the targeted element at that moment; a focus
/// change after the read doesn't invalidate it.  Keystroke / clipboard
/// captures, by contrast, can include events from a window the user already
/// switched to, so they're treated as low-confidence under app-switch.
fn is_high_confidence_capture(capture_method: &str) -> bool {
    matches!(capture_method, "ax" | "keystroke_verified")
}

#[derive(Serialize)]
pub struct ClassifyResponse {
    pub class: String,
    pub reason: String,
    pub candidates: Vec<LabelledHunk>,
    pub learned: bool,
    pub notify: bool,
    pub pending_id: Option<String>,
    pub promoted_count: usize,
    pub is_repeat: bool,
    /// Flat list of correct_form values that survived all gates and were
    /// promoted to vocabulary.  The desktop uses this for the in-app toast
    /// so it doesn't have to deserialize the full LabelledHunk schema.
    #[serde(default)]
    pub promoted_terms: Vec<String>,
    /// Terms recorded into the pending-promotions queue but not yet promoted
    /// (k-threshold not met).  Surface this to the user as a soft "noticed —
    /// once more and I'll remember" toast so the system never feels silent.
    #[serde(default)]
    pub queued_terms: Vec<String>,
}

pub async fn classify(
    State(state): State<AppState>,
    Json(body): Json<ClassifyBody>,
) -> (StatusCode, Json<ClassifyResponse>) {
    // 1. Look up the transcript and the user's output language preference.
    let rec = match history::get_recording(&state.pool, &body.recording_id) {
        Some(r) => r,
        None => {
            warn!("[classify] recording {} not found", body.recording_id);
            return (
                StatusCode::NOT_FOUND,
                Json(empty_response("USER_REPHRASE", "recording not found")),
            );
        }
    };
    let transcript = rec.transcript;
    let prefs = get_prefs(&state.pool, &state.default_user_id);
    let output_language = prefs
        .as_ref()
        .map(|p| p.output_language.clone())
        .unwrap_or_else(|| "hinglish".into());

    // 2. CAPTURE_ERROR gate.  Each check rejects an *obviously* bad signal
    //    so the LLM doesn't burn budget on it.  Critical: we never reject a
    //    high-confidence AX or keystroke_verified capture just because of an
    //    out-of-band condition like app_switch — those reads are atomic
    //    snapshots of a specific element at a specific moment, so a later
    //    focus change doesn't invalidate them.
    //
    //      • matches_clipboard — kept text equals the user's clipboard at
    //                            capture time.  They pasted on top of our
    //                            paste; the diff is meaningless regardless of
    //                            capture method.
    //      • app_switched      — focus left the original window mid-watch.
    //                            ONLY rejected for keystroke-only / clipboard
    //                            captures (those signals can be polluted by
    //                            the new window).  AX captures are kept —
    //                            an AX read returning text means it came from
    //                            the targeted element atomically, before we
    //                            noticed the switch.
    //      • too late          — > 30 s after paste.  Universal reject; a
    //                            very late edit is rarely tied to our paste.
    if body.matches_clipboard {
        info!(
            "[classify] capture_error: kept text matches clipboard for {}",
            body.recording_id
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "USER_REPHRASE",
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
                "USER_REPHRASE",
                "capture_error: app changed during low-confidence capture",
            )),
        );
    }
    if body.time_since_paste_ms > CAPTURE_STALE_MS {
        info!(
            "[classify] capture_error: stale capture ({}ms after paste) for {}",
            body.time_since_paste_ms, body.recording_id,
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "USER_REPHRASE",
                "capture_error: edit arrived > 30 s after paste",
            )),
        );
    }

    // 3. Run negative-signal demotion (independent of class).  If a previously
    //    promoted vocab term appears in polish but is removed in user_kept,
    //    decrement its weight.  Always runs — even on REWRITE/REPHRASE.
    let demoted = run_demotion_pass(&state, &body.ai_output, &body.user_kept);
    if demoted > 0 {
        info!("[classify] demoted {demoted} vocabulary term(s) on this edit");
    }

    // 4. STAGE 1 — Pre-filter.
    match pre_filter::run(&body.ai_output, &body.user_kept, &output_language) {
        pre_filter::PreFilter::Drop => {
            info!(
                "[classify] pre-filter: drop (no real edit) for {}",
                body.recording_id
            );
            return (
                StatusCode::OK,
                Json(empty_response("USER_REPHRASE", "no learnable change")),
            );
        }
        pre_filter::PreFilter::EarlyClass(d) => {
            info!(
                "[classify] pre-filter: early-class={} reason={:?} (skipping LLM)",
                d.class, d.reason
            );
            return (StatusCode::OK, Json(empty_response(d.class, d.reason)));
        }
        pre_filter::PreFilter::Pass => {} // continue
    }

    // 4. STAGE 2 — Compute diff hunks.
    let hunks = edit_diff::diff(&transcript, &body.ai_output, &body.user_kept);
    if hunks.is_empty() {
        // Pre-filter said pass but diff found no structural change — vacuous edit.
        info!(
            "[classify] diff produced no hunks for {}",
            body.recording_id
        );
        return (
            StatusCode::OK,
            Json(empty_response("USER_REPHRASE", "no structural diff hunks")),
        );
    }
    info!(
        "[classify] diff produced {} hunk(s) for {}",
        hunks.len(),
        body.recording_id
    );

    // 5. STAGE 2.5 — Phonetic triage.  For each hunk, decide cheaply whether
    //    its class is obvious (clear typo/case-fix → STT_ERROR; clear synonym
    //    swap → USER_REPHRASE).  Hunks that resolve here skip the LLM entirely.
    //    Ambiguous hunks fall through.
    let triage = phonetic_triage::triage(&hunks);
    let resolved_count = triage
        .iter()
        .filter(|d| matches!(d, phonetic_triage::TriageDecision::Resolved(_)))
        .count();
    if resolved_count > 0 {
        info!(
            "[classify] triage resolved {}/{} hunk(s) without LLM",
            resolved_count,
            triage.len(),
        );
    }
    let ambiguous_hunks: Vec<Hunk> = triage
        .iter()
        .zip(hunks.iter())
        .filter(|(d, _)| matches!(d, phonetic_triage::TriageDecision::Ambiguous))
        .map(|(_, h)| h.clone())
        .collect();

    // 6. STAGE 3 — LLM labeler (only for hunks the triage couldn't resolve).
    //    If everything was resolved, we skip the API call entirely.
    let llm_result = if ambiguous_hunks.is_empty() {
        info!("[classify] all hunks resolved by triage — no LLM call");
        None
    } else {
        let groq_key = prefs
            .as_ref()
            .and_then(|p| p.groq_api_key.clone())
            .or_else(|| std::env::var("GROQ_API_KEY").ok())
            .unwrap_or_default();
        let http = state.http_client.clone();
        match classifier::classify_edit(
            &http,
            &groq_key,
            &transcript,
            &body.ai_output,
            &body.user_kept,
            &ambiguous_hunks,
            &output_language,
        )
        .await
        {
            Some(r) => Some(r),
            None => {
                // LLM failed — but we may still have triage-resolved hunks to act on.
                // If we have NOTHING actionable, return the unavailable signal.
                if resolved_count == 0 {
                    info!(
                        "[classify] classifier unavailable — skipping for {}",
                        body.recording_id
                    );
                    return (
                        StatusCode::OK,
                        Json(empty_response("USER_REPHRASE", "classifier unavailable")),
                    );
                }
                None
            }
        }
    };

    // 7. Merge triage-resolved + LLM-labelled hunks back into one ordered set.
    let result = merge_triage_with_llm(triage, llm_result);

    // 6. STAGE 4 — Promotion gates + write artifacts.
    let mut promoted_count = 0_usize;
    let mut is_repeat = false;
    let mut pending_id = None;
    let mut learned = false;
    let mut promoted_terms: Vec<String> = Vec::new();

    // Capture-confidence master switch.  When false, no auto-promotion regardless
    // of class — we store as pending and let the user review.  This is the
    // foundational guard against unreliable AX-blind keystroke reconstruction
    // (where CGEventTap can't see selection events, so a "select X + type Y"
    // edit looks identical to "type Y at cursor", producing concatenations).
    let auto_promote_allowed = capture_allows_auto_promote(&body.capture_method);
    if !auto_promote_allowed {
        info!(
            "[classify] capture_method={:?} → low-confidence capture, no auto-promotion (will store as pending if learnable)",
            body.capture_method
        );
    }

    let mut queued_terms: Vec<String> = Vec::new();

    for cand in &result.candidates {
        let correct = cand.correct_form().trim();
        if correct.is_empty() {
            continue;
        }

        match cand.class {
            EditClass::SttError => {
                if !auto_promote_allowed {
                    continue;
                }
                let mut promoted_via_pending = false;
                match stt_promotion_disposition(cand, correct, &body.user_kept, &output_language) {
                    SttPromotionDisposition::Reject => continue,
                    SttPromotionDisposition::QueuePending => {
                        let Some(decision) = pending_promotions::record_sighting(
                            &state.pool,
                            &state.default_user_id,
                            correct,
                            cand.transcript_form(),
                            &output_language,
                            pending_promotions::DEFAULT_K,
                        ) else {
                            continue;
                        };
                        match decision {
                            pending_promotions::PromotionDecision::Pending { .. } => {
                                if !queued_terms.iter().any(|t| t.eq_ignore_ascii_case(correct)) {
                                    queued_terms.push(correct.to_string());
                                }
                                continue;
                            }
                            pending_promotions::PromotionDecision::Promote { .. } => {
                                promoted_via_pending = true;
                                is_repeat = true;
                            }
                        }
                    }
                    SttPromotionDisposition::PromoteNow => {}
                }

                // Promote on the first sighting. The capture-confidence gate
                // (auto_promote_allowed) and semantic safety gate
                // (stt_promotion_disposition: hard structural gates + strong
                // plausibility signal) already filter low-trust signals out.
                // Weak-but-plausible STT corrections now take the pending-
                // promotions path above so they can accumulate repeat
                // evidence instead of disappearing.
                let from = cand.transcript_form().trim();

                // Capture the surrounding sentence as example_context.
                // Find the sentence containing `correct` in user_kept; if no
                // sentence boundary, use the whole user_kept (it's already a
                // short message in nearly all cases).
                let example_ctx = surrounding_sentence(&body.user_kept, correct);
                if vocabulary::upsert_for_language_with_context(
                    &state.pool,
                    &state.default_user_id,
                    correct,
                    1.0,
                    "auto",
                    &output_language,
                    example_ctx.as_deref(),
                ) {
                    learned = true;
                    promoted_count += 1;
                    promoted_terms.push(correct.to_string());

                    // Sync FTS index so BM25 retrieval (the keyword half of
                    // hybrid retrieval) can find this term. Cheap, sync.
                    vocab_fts::upsert(
                        &state.pool,
                        &state.default_user_id,
                        correct,
                        example_ctx.as_deref(),
                    );

                    // Fire-and-forget: embed the new sighting and recompute
                    // the term's centroid (mean of up to 10 example
                    // embeddings). Failure here is non-fatal — the term
                    // still works via weight-based selection, just not via
                    // vector relevance until it's embedded.
                    spawn_vocab_embedding(state.clone(), correct.to_string(), example_ctx.clone());
                    // Foundational decoupling: meaning generation must run
                    // independently of the embedder. Previously it was fired
                    // *inside* spawn_vocab_embedding's success path, which
                    // meant a missing Gemini key or a single embed-API hiccup
                    // silently skipped meaning forever — leaving terms with
                    // NULL meaning that the polish prompt then filtered out.
                    // Both jobs run in parallel; either one's failure no
                    // longer kills the other.
                    spawn_meaning_refresh(
                        state.clone(),
                        correct.to_string(),
                        example_ctx.clone().unwrap_or_default(),
                    );
                }
                // Foundational: store BOTH the polish-side span AND the
                // raw transcript-side span as aliases for the canonical.
                // The polish span ("Main corps") matches future polish-shaped
                // transcripts; the transcript span ("मैं Corps") matches the
                // raw STT output before polish runs. Without storing both,
                // the rule stored on one shape can never fire on the other —
                // the bug we hit when MACOBS learned but next recording's
                // raw STT ("मैं corps") didn't match the polish-side rule.
                let aliases_written = stt_replacements::upsert_aliases_for_language(
                    &state.pool,
                    &state.default_user_id,
                    cand.hunk.transcript_window.as_str(), // raw STT span
                    from,                                 // polish span
                    correct,
                    1.0,
                    &output_language,
                );
                promoted_count += aliases_written;
                if aliases_written > 0 {
                    if let Some(canonical) =
                        vocabulary::get_term(&state.pool, &state.default_user_id, correct)
                    {
                        let mut alias_candidates = vec![from.to_string()];
                        let transcript_alias = cand.hunk.transcript_window.trim();
                        if !transcript_alias.is_empty() && transcript_alias != from {
                            alias_candidates.push(transcript_alias.to_string());
                        }
                        for alias in alias_candidates {
                            if let Some(rule) = stt_replacements::get_for_language(
                                &state.pool,
                                &state.default_user_id,
                                &alias,
                                correct,
                                &output_language,
                            ) {
                                let tier = stt_bias::deterministic_export_tier(&canonical, &rule);
                                let _ = stt_replacements::update_export_metadata(
                                    &state.pool,
                                    &state.default_user_id,
                                    &alias,
                                    correct,
                                    tier,
                                    stt_replacements::ReviewStatus::Pending,
                                    Some("Deterministic export tier assigned at learn time."),
                                    &output_language,
                                );
                                stt_background::spawn_alias_review(
                                    state.clone(),
                                    alias,
                                    correct.to_string(),
                                    output_language.clone(),
                                );
                            }
                        }
                    }
                }
                if vocabulary::top_terms(&state.pool, &state.default_user_id, 200)
                    .iter()
                    .any(|t| t.term.eq_ignore_ascii_case(correct) && t.use_count > 1)
                {
                    is_repeat = true;
                }
                if promoted_via_pending {
                    pending_promotions::delete(
                        &state.pool,
                        &state.default_user_id,
                        correct,
                        &output_language,
                    );
                }
            }
            EditClass::PolishError => {
                if !auto_promote_allowed {
                    continue;
                }
                if !polish_promotion_allowed(cand, correct, &body.user_kept, &output_language) {
                    continue;
                }
                let wrong = cand.polish_form().trim().to_ascii_lowercase();
                if wrong.is_empty() || wrong == correct.to_ascii_lowercase() {
                    continue;
                }
                // Promote on the first sighting. polish_promotion_allowed
                // already verified the correction appears in user_kept and
                // the script matches; the previous correction_exists gate was
                // an implicit k=2 (had to see the same wrong→right pair
                // twice) layered on top, which made obvious LLM mistakes
                // take two recordings to learn. correction_exists is now
                // tracked only as the is_repeat signal returned to the UI.
                let already_seen = correction_exists(&state, &wrong);
                if already_seen {
                    is_repeat = true;
                }
                corrections::upsert(
                    &state.pool,
                    &state.default_user_id,
                    &[(wrong, correct.to_ascii_lowercase())],
                );
                learned = true;
                promoted_count += 1;
                promoted_terms.push(correct.to_string());
            }
            EditClass::UserRephrase | EditClass::UserRewrite => {
                // No-op; safe defaults already kept by the labeler when uncertain.
            }
        }
    }

    // Invalidate lexicon cache if any corrections or stt_replacements were written
    if learned {
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    }

    // Pending-edit safety valve.  Two reasons to write a pending row:
    //   1. POLISH_ERROR with no auto-promotion fired (single-shot, user reviews).
    //   2. ANY learnable class (STT_ERROR | POLISH_ERROR) when the capture was
    //      low-confidence (auto_promote_allowed=false).  Without this, low-
    //      confidence captures would silently disappear.
    let learnable = result.class.is_learnable();
    let needs_pending = (result.class == EditClass::PolishError && !learned)
        || (learnable && !auto_promote_allowed);
    if needs_pending {
        pending_id = pending_edits::insert(
            &state.pool,
            &state.default_user_id,
            Some(&body.recording_id),
            &body.ai_output,
            &body.user_kept,
        );
    }

    let notify = match result.class {
        EditClass::SttError => promoted_count > 0,
        EditClass::PolishError => learned && is_repeat,
        _ => false,
    };

    info!(
        "[classify] {} overall={} hunks={} promoted={} repeat={} notify={} learned={} pending={:?}",
        body.recording_id,
        result.class.as_str(),
        result.candidates.len(),
        promoted_count,
        is_repeat,
        notify,
        learned,
        pending_id,
    );

    (
        StatusCode::OK,
        Json(ClassifyResponse {
            class: result.class.as_str().to_string(),
            reason: result.reason,
            candidates: result.candidates,
            learned,
            notify,
            pending_id,
            promoted_count,
            is_repeat,
            promoted_terms,
            queued_terms,
        }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SttPromotionDisposition {
    PromoteNow,
    QueuePending,
    Reject,
}

/// Defense-in-depth decision for STT_ERROR learning.
///
/// Hard structural failures are rejected outright. Candidates that are
/// structurally valid but too weak for immediate promotion are sent to the
/// pending-promotions queue so repeated corrections can accumulate.
fn stt_promotion_disposition(
    cand: &LabelledHunk,
    correct: &str,
    user_kept: &str,
    output_language: &str,
) -> SttPromotionDisposition {
    if !promotion_gate::appears_in_user_kept(correct, user_kept) {
        warn!(
            "[classify] STT_ERROR rejected — correct_form {correct:?} not in user_kept (LLM hallucination?)"
        );
        return SttPromotionDisposition::Reject;
    }
    if !promotion_gate::script_matches(correct, output_language) {
        warn!(
            "[classify] STT_ERROR rejected — script of {correct:?} doesn't match output_language={output_language:?}"
        );
        return SttPromotionDisposition::Reject;
    }

    // Concatenation guard: when correct_form contains polish_form as a
    // substring (e.g. polish="MAAR" + kept="EMIACMAAR"), the user almost
    // certainly intended a *replacement* but cursor positioning produced an
    // *insertion*.  Refuse to promote — it's ambiguous and produces noisy vocab.
    //
    // Exception: if extracted_term is present and points at a sub-token (which
    // by parser invariant must be a whole-word substring of the hunk windows),
    // we trust the extraction over the raw concatenation pattern.
    if cand.extracted_term.is_none()
        && promotion_gate::is_concatenation_pattern(cand.polish_form(), correct)
    {
        warn!(
            "[classify] STT_ERROR rejected — concatenation pattern: polish_form {:?} ⊂ correct_form {:?} (likely insertion-without-deletion)",
            cand.polish_form(),
            correct,
        );
        return SttPromotionDisposition::Reject;
    }

    // Plausibility: the candidate must look STT-error-like.
    // Either phonetic similarity to the wrong form, or independent jargon-ness.
    let phon_sim = phonetics::similarity(cand.transcript_form(), correct)
        .max(phonetics::similarity(cand.polish_form(), correct));
    let jargon = phonetics::jargon_score(correct);
    let confident = cand.confidence >= 0.7;

    if phon_sim < 0.5 && jargon < 0.4 && !confident {
        info!(
            "[classify] STT_ERROR queued — weak signal (phon={phon_sim:.2}, jargon={jargon:.2}, conf={:.2}) for {correct:?}",
            cand.confidence,
        );
        return SttPromotionDisposition::QueuePending;
    }
    SttPromotionDisposition::PromoteNow
}

/// Gate for POLISH_ERROR auto-promotion.
fn polish_promotion_allowed(
    _cand: &LabelledHunk,
    correct: &str,
    user_kept: &str,
    output_language: &str,
) -> bool {
    promotion_gate::appears_in_user_kept(correct, user_kept)
        && promotion_gate::script_matches(correct, output_language)
}

/// Demote vocabulary terms that appear in polish but are removed in user_kept.
fn run_demotion_pass(state: &AppState, polish: &str, user_kept: &str) -> usize {
    let polish_lower = polish.to_ascii_lowercase();
    let kept_lower = user_kept.to_ascii_lowercase();
    let vocab = vocabulary::top_terms(&state.pool, &state.default_user_id, 200);

    let mut demoted = 0_usize;
    for v in vocab {
        if v.source == "starred" {
            continue;
        }
        let term_lower = v.term.to_ascii_lowercase();
        if polish_lower.contains(&term_lower)
            && !kept_lower.contains(&term_lower)
            && vocabulary::demote(&state.pool, &state.default_user_id, &v.term, 0.5)
        {
            demoted += 1;
        }
    }
    demoted
}

fn correction_exists(state: &AppState, wrong: &str) -> bool {
    let conn = match state.pool.get() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM word_corrections
         WHERE user_id = ?1 AND wrong_text = ?2",
            rusqlite::params![state.default_user_id.as_str(), wrong],
            |row| row.get(0),
        )
        .unwrap_or(0);
    count > 0
}

/// Merge phonetic-triage decisions with the LLM's labels for the ambiguous
/// hunks.  Triage-resolved hunks keep their synthetic labels; ambiguous hunks
/// take their labels from the LLM result (in input order).  The returned
/// `ClassifyResult` looks identical to one produced by the all-LLM path, so
/// the rest of the route doesn't care which source labelled which hunk.
fn merge_triage_with_llm(
    triage: Vec<phonetic_triage::TriageDecision>,
    llm_result: Option<classifier::ClassifyResult>,
) -> classifier::ClassifyResult {
    let mut llm_iter = llm_result
        .as_ref()
        .map(|r| r.candidates.clone())
        .unwrap_or_default()
        .into_iter();

    let mut candidates: Vec<LabelledHunk> = Vec::with_capacity(triage.len());
    for d in triage {
        match d {
            phonetic_triage::TriageDecision::Resolved(lh) => candidates.push(lh),
            phonetic_triage::TriageDecision::Ambiguous => {
                if let Some(lh) = llm_iter.next() {
                    candidates.push(lh);
                }
                // If the LLM didn't return a label for an ambiguous hunk
                // (failure / response-shape mismatch), we drop it — better
                // to ignore one signal than to hallucinate a class.
            }
        }
    }

    // Compute overall class — priority order STT_ERROR > POLISH_ERROR
    // > USER_REWRITE > USER_REPHRASE.
    let overall = candidates
        .iter()
        .map(|c| c.class)
        .max_by_key(|c| match c {
            EditClass::SttError => 4,
            EditClass::PolishError => 3,
            EditClass::UserRewrite => 2,
            EditClass::UserRephrase => 1,
        })
        .unwrap_or(EditClass::UserRephrase);

    let confidence = if candidates.is_empty() {
        0.0
    } else {
        candidates.iter().map(|c| c.confidence).sum::<f64>() / candidates.len() as f64
    };

    let reason = match &llm_result {
        Some(r) if !r.reason.is_empty() => r.reason.clone(),
        _ => "phonetic triage labelled all hunks without LLM".to_string(),
    };

    classifier::ClassifyResult {
        class: overall,
        reason,
        candidates,
        confidence,
    }
}

/// Fire-and-forget: embed a newly learned vocab term (with its context)
/// and persist the vector so polish-time relevance retrieval can find it.
///
/// Why fire-and-forget: the embedder is a Gemini network call (~50–150 ms).
/// Blocking the classify response on it would slow every learning event
/// and tie the user-visible "learned a new word" toast to an external
/// API's availability. If the embedder is down, the term is still useful
/// (fallback to starred + weight selection); it just won't get the
/// relevance boost until a future re-embed.
fn spawn_vocab_embedding(state: AppState, term: String, example_context: Option<String>) {
    tokio::spawn(async move {
        // Resolve Gemini key from prefs, fall back to env var.
        let key = get_prefs(&state.pool, &state.default_user_id)
            .and_then(|p| p.gemini_api_key)
            .or_else(|| std::env::var("GEMINI_API_KEY").ok())
            .unwrap_or_default();
        if key.is_empty() {
            // No key — silent skip. Term still works without embedding.
            return;
        }
        // Embed `"{term}. {example}"` so the vector captures both the
        // canonical surface form and the situation it's used in. When the
        // user has no example yet, embed the bare term.
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
            // Append this sighting to the per-term FIFO ring (cap N=10) and
            // recompute the centroid. The centroid replaces the legacy
            // single-embedding representation in vocab_embeddings.embedding
            // so retrieval still uses one vector per term — but that vector
            // is now the mean of the user's recent usages, not just the
            // first one we saw.
            vocab_embeddings::record_example_and_recentre(
                &pool, &uid, &term2, &embedding, &example,
            );
            // Increment the per-term counter so meaning_needs_refresh fires
            // every K=MEANING_REFRESH_THRESHOLD examples.
            vocabulary::bump_examples_since_meaning(&pool, &uid, &term2);
            // Diagnostic: log cluster spread so we can see when a term is
            // being used in semantically distinct contexts (future: trigger
            // an auto-split into two prototypes).
            let spread = vocab_embeddings::cluster_spread(&pool, &uid, &term2);
            if spread > 0.5 {
                tracing::info!(
                    "[vocab-emb] high cluster spread for {term2:?}: {:.2} — bimodal usage",
                    spread,
                );
            }
        });
        // Persist centroid + bumped counter, then return. Meaning generation
        // is now triggered separately from the promotion path so it doesn't
        // depend on the embedder running successfully — see
        // spawn_meaning_refresh call site in the STT_ERROR handler.
        let _ = blocking.await;
        // For long-tail use bumps (term retrieved + used in polish) we still
        // want meaning to refresh as the counter crosses the threshold.
        // meaning_needs_refresh is cheap (one row read) so calling it here
        // when the counter just bumped is the right place — the call exits
        // immediately if the threshold isn't crossed.
        spawn_meaning_refresh(state, term, example_context.unwrap_or_default());
    });
}

/// Fire-and-forget: refresh a term's distilled meaning when needed.
///
/// Trigger conditions (computed in vocabulary::meaning_needs_refresh):
///   • meaning is NULL (first time after promotion), OR
///   • examples_since_meaning ≥ MEANING_REFRESH_THRESHOLD (default 3).
///
/// Why fire-and-forget: the Groq call is ~50–200ms and tied to a third-party
/// API. The vocab term works without a meaning (the polish prompt simply
/// omits that line); we never want to block the user-visible learning toast
/// on this. Failures log and degrade gracefully — the next promotion or
/// refresh tick will retry.
fn spawn_meaning_refresh(state: AppState, term: String, latest_example: String) {
    tokio::spawn(async move {
        let uid = state.default_user_id.clone();
        let pool = state.pool.clone();

        // Cheap synchronous gate — most calls exit here without touching the LLM.
        if !vocabulary::meaning_needs_refresh(&pool, &uid, &term) {
            return;
        }

        // Resolve BOTH keys up-front. meaning::generate_initial / refine do a
        // Groq → OpenAI fallback internally; we just plumb the keys through
        // so a missing Groq key (or a Groq outage) still gets the meaning
        // generated via OpenAI.
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
            // First-time generation from the most recently observed example.
            None => {
                let example = if latest_example.trim().is_empty() {
                    term.clone()
                } else {
                    latest_example.clone()
                };
                meaning::generate_initial(
                    &state.http_client,
                    &groq_key,
                    &openai_key,
                    &term,
                    &example,
                )
                .await
            }
            // Refinement: hand the LLM the prior description + recent ring.
            Some(prev) => {
                let examples = vocab_embeddings::support_example_texts(&pool, &uid, &term, 4);
                if examples.is_empty() {
                    None
                } else {
                    meaning::refine(
                        &state.http_client,
                        &groq_key,
                        &openai_key,
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
    });
}

/// Find the sentence inside `text` that contains `term`, returning it
/// trimmed. When the term appears inside a longer message, this gives the
/// polish LLM exactly the surrounding context the user used the term in
/// (the foundational signal for context-aware mishearing recognition).
///
/// Sentence boundaries: '.', '!', '?', '\n'. Falls back to the whole text
/// when no boundary brackets the term.
fn surrounding_sentence(text: &str, term: &str) -> Option<String> {
    let term_l = term.to_ascii_lowercase();
    if term_l.is_empty() {
        return None;
    }
    let text_l = text.to_ascii_lowercase();
    let pos = text_l.find(&term_l)?;
    // Walk backward to nearest sentence terminator (or start of text).
    let start = text[..pos]
        .rfind(|c: char| matches!(c, '.' | '!' | '?' | '\n'))
        .map(|i| i + 1)
        .unwrap_or(0);
    // Walk forward to nearest terminator after the term.
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
        candidates: vec![],
        learned: false,
        notify: false,
        pending_id: None,
        promoted_count: 0,
        is_repeat: false,
        promoted_terms: vec![],
        queued_terms: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{classifier::ExtractedTerm, edit_diff::Hunk};

    fn test_stt_candidate(
        transcript_window: &str,
        polish_window: &str,
        kept_window: &str,
        transcript_form: &str,
        correct_form: &str,
        confidence: f64,
    ) -> LabelledHunk {
        LabelledHunk {
            hunk: Hunk {
                transcript_window: transcript_window.to_string(),
                polish_window: polish_window.to_string(),
                kept_window: kept_window.to_string(),
            },
            class: EditClass::SttError,
            confidence,
            extracted_term: Some(ExtractedTerm {
                transcript_form: transcript_form.to_string(),
                correct_form: correct_form.to_string(),
            }),
        }
    }

    #[test]
    fn surrounding_sentence_returns_the_containing_clause() {
        let text = "Hello there. MACOBS ka IPO ka 12 hazaar batana. Then bye.";
        let got = surrounding_sentence(text, "MACOBS");
        assert_eq!(got.as_deref(), Some("MACOBS ka IPO ka 12 hazaar batana."));
    }

    #[test]
    fn surrounding_sentence_handles_no_terminator() {
        let text = "MACOBS ka IPO ka 12 hazaar batana"; // no '.', '!', '?'
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
        // user_kept may have the term in any case; we still want to find it
        let text = "Hello. macobs ka IPO. Bye.";
        let got = surrounding_sentence(text, "MACOBS");
        assert_eq!(got.as_deref(), Some("macobs ka IPO."));
    }

    #[test]
    fn capture_method_policy_table() {
        assert!(capture_allows_auto_promote("ax"));
        assert!(capture_allows_auto_promote("keystroke_verified"));
        assert!(capture_allows_auto_promote("clipboard"));
        // The bug case: keystroke alone is NOT allowed to auto-promote.
        assert!(!capture_allows_auto_promote("keystroke_only"));
        // Unknown values default to refused (safe fallback).
        assert!(!capture_allows_auto_promote(""));
        assert!(!capture_allows_auto_promote("anything_else"));
    }

    #[test]
    fn stt_disposition_rejects_hallucinated_term() {
        let cand = test_stt_candidate("hrmmn", "HRMMN", "HRM8", "HRMMN", "MACOBS", 0.61);
        assert_eq!(
            stt_promotion_disposition(&cand, "MACOBS", "Aur kya raha hai HRM8?", "hinglish"),
            SttPromotionDisposition::Reject
        );
    }

    #[test]
    fn stt_disposition_queues_weak_but_structurally_valid_signal() {
        let cand = test_stt_candidate("return", "return", "Atlas", "return", "Atlas", 0.41);
        assert_eq!(
            stt_promotion_disposition(
                &cand,
                "Atlas",
                "Can you open Atlas for me?",
                "english"
            ),
            SttPromotionDisposition::QueuePending
        );
    }

    #[test]
    fn stt_disposition_promotes_confident_signal_now() {
        let cand = test_stt_candidate("MacOps", "MacOps", "MACOBS", "MacOps", "MACOBS", 0.93);
        assert_eq!(
            stt_promotion_disposition(
                &cand,
                "MACOBS",
                "MACOBS ka kitna profit hai is saal?",
                "hinglish"
            ),
            SttPromotionDisposition::PromoteNow
        );
    }
}
