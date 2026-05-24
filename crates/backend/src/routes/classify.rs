//! POST /v1/classify-edit  —  Learning Pipeline v3
//!
//! Deterministic edit classification with LLM fallback only for meaning
//! generation. Replaces the v2 LLM-driven pipeline:
//!
//!   1. **Capture gate** (cheap): reject stale / clipboard / app-switched edits
//!   2. **Branch** — no-edit (reward active vocab), full deletion, or stale
//!   3. **Demotion** — unconditional negative signal for removed terms
//!   4. **Deterministic classifier** — classify hunks from diff without LLM
//!   5. **Meaning generation** — Groq call ONLY for new STT correction terms
//!   6. **Save** — persist learnable changes by reason type

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::{
    AppState,
    llm::{
        analyzer::{self, AnalyzedChange, ChangeReason},
        edit_diff, promotion_gate,
    },
    store::{
        corrections, history, pending_promotions, prefs::get_prefs, stt_replacements,
        tier2_edit_policy, vectors, vocab_embeddings, vocab_fts, vocabulary,
    },
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
    pub queued_terms: Vec<QueuedTerm>,
    /// Pass-through from the analyzer — each change the LLM identified.
    pub changes: Vec<AnalyzedChange>,
    /// Terms where the classifier can't decide — needs user confirmation via status bar toast.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguous_terms: Vec<AmbiguousTerm>,
    /// Corrections the system keeps making wrong — needs user to confirm blocking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub negative_terms: Vec<NegativeTerm>,
    /// Changes the user should review before learning (multi-change edits).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_candidates: Vec<ReviewCandidate>,
}

#[derive(Serialize, Clone)]
pub struct AmbiguousTerm {
    pub original: String,
    pub corrected: String,
    pub context: String,
    pub recording_id: String,
}

#[derive(Serialize, Clone)]
pub struct NegativeTerm {
    pub term: String,
    pub wrong_replacement: String,
    pub correction_count: i64,
}

/// A candidate change the user should review before learning.
#[derive(Serialize, Deserialize, Clone)]
pub struct ReviewCandidate {
    pub original: String,
    pub corrected: String,
    pub term_type: String,
    pub learnable: bool,
    pub tag: String,
}

#[derive(Serialize, Clone)]
pub struct QueuedTerm {
    pub term: String,
    pub sighting_count: i64,
    pub k: i64,
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
    let transcript = rec
        .raw_transcript
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| rec.transcript.clone());
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

    history::apply_edit_feedback(&state.pool, &body.recording_id, &body.user_kept);
    if let Some(edit_event_id) = vectors::insert_edit_event(
        &state.pool,
        &rec.user_id,
        Some(&rec.id),
        &transcript,
        &body.ai_output,
        &body.user_kept,
        rec.target_app.as_deref(),
    ) {
        info!(
            "[classify] edit_event {} created for recording {}",
            edit_event_id, rec.id
        );
    } else {
        warn!(
            "[classify] failed to insert edit_event for {}",
            body.recording_id
        );
    }

    // ── Step 3: Revert wrong aliases (unconditional on every edit) ────────────
    // Detects vocab terms in the polish output that the user replaced with a
    // different word. Deletes only the offending STT alias — the vocab entry
    // itself is kept because the term is valid.
    let reverted = run_alias_revert_pass(&state, &body.ai_output, &body.user_kept);
    let mut negative_terms: Vec<NegativeTerm> = Vec::new();
    if !reverted.is_empty() {
        info!(
            "[classify] reverted {} wrong alias(es) on this edit",
            reverted.len()
        );
        for r in &reverted {
            negative_terms.push(NegativeTerm {
                term: r.replaced_with.clone(),
                wrong_replacement: r.term.clone(),
                correction_count: 1,
            });
        }
    }
    let policy_feedback = tier2_edit_policy::mark_removed_feedback(
        &state.pool,
        &state.default_user_id,
        &body.recording_id,
        &body.user_kept,
    );
    let mut policy_touched = policy_feedback.marked_kept > 0 || policy_feedback.penalized > 0;
    if policy_touched {
        info!(
            "[classify] tier2 edit-policy feedback: kept_marked={} penalized={} for {}",
            policy_feedback.marked_kept, policy_feedback.penalized, body.recording_id,
        );
    }
    let edit_hunks = edit_diff::diff(&transcript, &body.ai_output, &body.user_kept);

    // ── Step 4: Deterministic classification ──────────────────────────────────
    // Classify hunks purely from the diff — no LLM call at all.
    // Meaning is generated iteratively in the background (GPT-5.4-mini).
    let det_changes = deterministic_classify_hunks(
        &edit_hunks,
        &transcript,
        &body.ai_output,
        &body.user_kept,
        &state.pool,
        &state.default_user_id,
    );

    info!(
        "[classify] deterministic: {} change(s) from {} hunk(s)",
        det_changes.len(),
        edit_hunks.len(),
    );

    // For new STT terms that need meaning, batch a single Groq call
    // No synchronous LLM call — meaning is generated iteratively in the
    // background by spawn_vocab_embedding → spawn_meaning_refresh (GPT-5.4-mini).
    // Each sighting adds context; the meaning deepens over time.
    let mut analyzer_changes = det_changes;
    for change in &mut analyzer_changes {
        if change.context_example.is_none() && change.should_learn {
            change.context_example = surrounding_sentence(&body.user_kept, &change.corrected);
        }
    }

    let overall_class = analyzer_changes
        .iter()
        .max_by_key(|c| match c.reason {
            ChangeReason::SttError => 5u8,
            ChangeReason::PolishError => 4,
            ChangeReason::FormatPreference => 3,
            ChangeReason::StylePreference => 2,
            ChangeReason::StructuralRewrite => 1,
        })
        .map(|c| c.reason.as_str().to_string())
        .unwrap_or_else(|| "no_change".into());

    // ── Detect ambiguous terms — ask user instead of guessing ─────────────
    let mut ambiguous_terms: Vec<AmbiguousTerm> = Vec::new();
    for change in &analyzer_changes {
        if change.reason != ChangeReason::StylePreference || change.should_learn {
            continue;
        }
        let skip = change.skip_reason.as_deref().unwrap_or("");
        let is_ambiguous_case =
            skip.contains("common word") || skip.contains("might be rephrasing");
        if !is_ambiguous_case {
            continue;
        }
        // Brand signal: corrected has uppercase letter (Said, React, Vite, Mac)
        let has_brand_signal = change.corrected.chars().any(|c| c.is_ascii_uppercase());
        if has_brand_signal {
            info!(
                "[classify] ambiguous: {:?} → {:?} — asking user",
                change.original, change.corrected,
            );
            ambiguous_terms.push(AmbiguousTerm {
                original: change.original.clone(),
                corrected: change.corrected.clone(),
                context: surrounding_sentence(&body.user_kept, &change.corrected)
                    .unwrap_or_else(|| body.user_kept.clone()),
                recording_id: body.recording_id.clone(),
            });
        }
    }

    let analyzer_output = analyzer::AnalyzerOutput {
        changes: analyzer_changes,
        overall_class,
    };

    // ── Codex token — only needed for embedding/meaning refresh, not classification
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

    // ── Step 5: Save learnable changes ───────────────────────────────────────
    // Pre-count STT changes to decide: auto-learn vs review card.
    let stt_change_count = analyzer_output
        .changes
        .iter()
        .filter(|c| matches!(c.reason, ChangeReason::SttError) && c.should_learn)
        .count();

    let mut promoted_count = 0_usize;
    let mut promoted_terms: Vec<String> = Vec::new();
    let mut queued_terms: Vec<QueuedTerm> = Vec::new();
    let mut review_candidates: Vec<ReviewCandidate> = Vec::new();
    let mut has_repeat = false;
    let mut learned = false;

    for change in &analyzer_output.changes {
        // Build edit pair directly from the deterministic change (which already
        // came from the hunk). No need to re-validate against hunks.
        let deterministic_pair = if matches!(change.reason, ChangeReason::SttError) {
            let corrected_clean = clean_surface(&change.corrected);
            let original_clean = clean_surface(&change.original);
            if corrected_clean.is_empty() {
                None
            } else {
                let edit_type = if original_clean.is_empty() {
                    "insert"
                } else {
                    "replace"
                };
                let (left_context, right_context) =
                    context_around_kept_term(&body.user_kept, &corrected_clean, 3);
                Some(DeterministicEditPair {
                    variant_form: original_clean,
                    correct_form: corrected_clean,
                    edit_type: edit_type.to_string(),
                    left_context,
                    right_context,
                })
            }
        } else {
            None
        };
        if matches!(change.reason, ChangeReason::SttError) && deterministic_pair.is_none() {
            continue;
        }
        let corrected = deterministic_pair
            .as_ref()
            .map(|pair| preferred_corrected_surface(&pair.correct_form, &change.corrected))
            .unwrap_or_else(|| clean_surface(&change.corrected));
        let original = deterministic_pair
            .as_ref()
            .map(|pair| pair.variant_form.clone())
            .unwrap_or_else(|| clean_surface(&change.original));
        let corrected = corrected.as_str();
        let original = original.as_str();
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
        // update the existing entry's meaning (deepening, not duplicating).
        // STT corrections to an already protected local term still flow
        // through, because the analyzer is allowed to say "not a new vocab
        // term" but it is not allowed to block a concrete edit-policy pair.
        if !change.should_learn && !matches!(change.reason, ChangeReason::SttError) {
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
        if !change.should_learn
            && matches!(change.reason, ChangeReason::SttError)
            && protected_vocab_lookup(&state.pool, &state.default_user_id, corrected).is_none()
            && canonical_developer_term(corrected).is_none()
        {
            continue;
        }

        match change.reason {
            ChangeReason::SttError => {
                if promotion_gate::is_common_word(corrected) {
                    info!("[classify] STT_ERROR skipped — common word: {corrected:?}");
                    continue;
                }
                if promotion_gate::is_numeric_junk(corrected) {
                    info!("[classify] STT_ERROR skipped — numeric junk: {corrected:?}");
                    continue;
                }
                if !promotion_gate::appears_in_user_kept(corrected, &body.user_kept) {
                    info!(
                        "[classify] STT_ERROR skipped — corrected form not present in kept text: {corrected:?}"
                    );
                    continue;
                }
                if !promotion_gate::script_matches(corrected, &output_language) {
                    info!(
                        "[classify] STT_ERROR skipped — script mismatch for {output_language:?}: {corrected:?}"
                    );
                    continue;
                }
                let existing_protected =
                    protected_vocab_lookup(&state.pool, &state.default_user_id, corrected).or_else(
                        || {
                            let hint = clean_surface(&change.corrected);
                            let same_token = tier2_edit_policy::normalize_token(&hint)
                                == tier2_edit_policy::normalize_token(corrected);
                            (same_token && !hint.is_empty()).then(|| {
                                protected_vocab_lookup(&state.pool, &state.default_user_id, &hint)
                            })?
                        },
                    );
                let existing_term = existing_protected.as_ref().and_then(|canonical| {
                    vocabulary::find_by_term_ci(&state.pool, &state.default_user_id, canonical)
                });
                let existing_canonical =
                    existing_term.as_ref().map(|existing| existing.term.clone());
                let canonical_for_policy = existing_canonical
                    .as_deref()
                    .or(existing_protected.as_deref())
                    .unwrap_or(corrected);
                let term_type = existing_term
                    .as_ref()
                    .and_then(|existing| existing.term_type.as_deref())
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| vocabulary::classify_term_type(canonical_for_policy));
                if existing_protected.is_none() && matches!(term_type, "phrase" | "other") {
                    info!(
                        "[classify] STT_ERROR skipped — not a proper noun (type={term_type}): {corrected:?}"
                    );
                    continue;
                }
                if let Some(reason) =
                    unsafe_stt_source_reason(&state.pool, &state.default_user_id, original)
                {
                    info!("[classify] STT_ERROR skipped — unsafe source {original:?}: {reason}");
                    continue;
                }

                // Record edit-policy rule (bookkeeping, does not count as "learned")
                if let Some(pair) = deterministic_pair.as_ref() {
                    if tier2_edit_policy::record_explicit_edit(
                        &state.pool,
                        &state.default_user_id,
                        &pair.variant_form,
                        canonical_for_policy,
                        pair.edit_type.as_str(),
                        &pair.left_context,
                        &pair.right_context,
                        Some(&body.recording_id),
                    ) {
                        policy_touched = true;
                        info!(
                            "[classify] recorded tier2 edit-policy {} rule: {:?} -> {:?}",
                            pair.edit_type, pair.variant_form, canonical_for_policy
                        );
                    }
                }

                // ── Route decision: auto-learn vs review card ───────────
                //
                // When 2+ STT changes exist, ALL go to the review card —
                // the user picks which to learn. Nothing auto-fires.
                //
                // Only a single unambiguous K=1 change with no other STT
                // changes bypasses the review card.
                if stt_change_count >= 2 {
                    let tag = if existing_term.is_some() {
                        "stt"
                    } else {
                        "stt"
                    };
                    review_candidates.push(ReviewCandidate {
                        original: original.to_string(),
                        corrected: corrected.to_string(),
                        term_type: term_type.to_string(),
                        learnable: true,
                        tag: tag.to_string(),
                    });
                    info!(
                        "[classify] review candidate ({} total): {:?} → {:?} (type={term_type}, in_vocab={})",
                        stt_change_count,
                        original,
                        corrected,
                        existing_term.is_some(),
                    );
                    continue;
                }

                // Single change path — decide auto-learn vs review
                if existing_term.is_some() {
                    // Self-upgrade: capture new distortion for existing term
                    has_repeat = true;
                    if !original.is_empty() {
                        stt_replacements::upsert_aliases_for_language(
                            &state.pool,
                            &state.default_user_id,
                            original,
                            original,
                            canonical_for_policy,
                            1.0,
                            &output_language,
                        );
                        info!(
                            "[classify] self-upgrade: new distortion {:?} for known term {:?}",
                            original, canonical_for_policy,
                        );
                    }
                } else if matches!(term_type, "brand" | "acronym" | "code_identifier")
                    && !promotion_gate::is_common_word(original)
                {
                    info!(
                        "[classify] auto-learn K=1: {:?} → {:?} (type={term_type})",
                        original, corrected,
                    );
                } else {
                    // Single change but not K=1 unambiguous — show review card
                    review_candidates.push(ReviewCandidate {
                        original: original.to_string(),
                        corrected: corrected.to_string(),
                        term_type: term_type.to_string(),
                        learnable: true,
                        tag: "stt".to_string(),
                    });
                    info!(
                        "[classify] review candidate (single): {:?} → {:?} (type={term_type})",
                        original, corrected,
                    );
                    continue;
                }

                let should_promote_now = true;

                if !should_promote_now {
                    continue;
                }

                let (canonical_term, weight_bump) = if let Some(existing) = existing_term {
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

                    // Auto-activate ALL candidate edit-policy rules for this term.
                    // The term is now confirmed (3 sightings) — no reason to keep
                    // rules as candidates. Each Deepgram distortion is different,
                    // so individual rules rarely reach 2 positives on their own.
                    let activated = tier2_edit_policy::activate_all_for_term(
                        &state.pool,
                        &state.default_user_id,
                        &canonical_term,
                    );
                    if activated > 0 {
                        policy_touched = true;
                        info!(
                            "[classify] auto-activated {activated} edit-policy rule(s) for promoted term {canonical_term:?}"
                        );
                    }
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

                // ── Proactive distortion seeding ────────────────────────
                // Pre-generate 8-12 likely Deepgram distortions as aliases
                // so the system handles novel mis-hearings immediately.
                let proactive = stt_replacements::generate_proactive_distortions(
                    &state.pool,
                    &state.default_user_id,
                    &canonical_term,
                    original,
                    &output_language,
                );
                if proactive > 0 {
                    info!(
                        "[classify] seeded {proactive} proactive distortion(s) for {canonical_term:?}"
                    );
                }

                // ── Auto-classify term type ──────────────────────────────
                // classify_term_type is already called inside upsert, but
                // we call it here for any term that already existed without
                // a type classification.
                let _ = vocabulary::classify_term_type(corrected);
                pending_promotions::delete(
                    &state.pool,
                    &state.default_user_id,
                    &canonical_term,
                    &output_language,
                );
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

            ChangeReason::StructuralRewrite => {
                if stt_change_count >= 2 && !corrected.is_empty() {
                    review_candidates.push(ReviewCandidate {
                        original: original.to_string(),
                        corrected: corrected.to_string(),
                        term_type: vocabulary::classify_term_type(corrected).to_string(),
                        learnable: false,
                        tag: "added".to_string(),
                    });
                }
            }

            ChangeReason::StylePreference => {
                // Not learnable — intentional no-op.
            }
        }
    }

    let has_negatives = !negative_terms.is_empty();
    let has_review = !review_candidates.is_empty();

    // Invalidate after any corrections, stt_replacements, or Tier 2 policy writes.
    if learned || policy_touched || has_negatives {
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    }

    // Only retrain if something was actually committed (not pending review).
    // When review candidates exist, retrain happens after confirm-batch.
    if (learned || has_negatives) && !has_review {
        schedule_onnx_retrain(state.clone());
    }

    let notify = learned && (promoted_count > 0 || policy_touched);

    info!(
        "[classify] {} overall={} changes={} promoted={} notify={} learned={} negatives={} review={}",
        body.recording_id,
        analyzer_output.overall_class,
        analyzer_output.changes.len(),
        promoted_count,
        notify,
        learned,
        negative_terms.len(),
        review_candidates.len(),
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
            ambiguous_terms,
            negative_terms,
            review_candidates,
        }),
    )
}

/// Info about a wrong alias the system applied that the user reverted.
struct RevertedAlias {
    /// The vocab term the system wrongly output (e.g. "Macobs")
    term: String,
    /// What the user actually wanted (e.g. "access")
    replaced_with: String,
}

/// Detect vocab terms in polish that the user replaced with a different word.
/// Only deletes the offending STT alias — the vocabulary entry itself stays
/// intact because the term is valid, just the alias was wrong.
fn run_alias_revert_pass(state: &AppState, polish: &str, user_kept: &str) -> Vec<RevertedAlias> {
    let polish_lower = polish.to_ascii_lowercase();
    let kept_lower = user_kept.to_ascii_lowercase();
    let vocab = vocabulary::top_terms(&state.pool, &state.default_user_id, 1000);

    let mut reverted = Vec::new();
    for v in vocab {
        let term_lower = v.term.to_ascii_lowercase();
        if !polish_lower.contains(&term_lower) || kept_lower.contains(&term_lower) {
            continue;
        }

        let replaced_with =
            find_replacement_at_position(polish, user_kept, &v.term).unwrap_or_default();

        if replaced_with.is_empty() {
            continue;
        }

        // Delete only the specific alias that caused the wrong replacement.
        // The vocabulary entry for "Macobs" stays — it's a real word. Only
        // the alias "access" → "Macobs" was wrong.
        let alias_deleted = stt_replacements::delete_alias_pair(
            &state.pool,
            &state.default_user_id,
            &replaced_with,
            &v.term,
        );
        if alias_deleted > 0 {
            info!(
                "[revert] deleted {alias_deleted} wrong alias(es) {:?} → {:?} (vocab kept)",
                replaced_with, v.term,
            );
            reverted.push(RevertedAlias {
                term: v.term.clone(),
                replaced_with,
            });
        }
    }
    reverted
}

/// Given polish and user_kept, find what word the user wrote in place of `term`.
fn find_replacement_at_position(polish: &str, user_kept: &str, term: &str) -> Option<String> {
    let p_tokens: Vec<&str> = polish.split_whitespace().collect();
    let k_tokens: Vec<&str> = user_kept.split_whitespace().collect();
    let term_lower = term.to_ascii_lowercase();

    for (i, pt) in p_tokens.iter().enumerate() {
        if pt
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_ascii_lowercase()
            == term_lower
        {
            if let Some(kt) = k_tokens.get(i) {
                let cleaned = kt.trim_matches(|c: char| !c.is_alphanumeric());
                if !cleaned.is_empty() && cleaned.to_ascii_lowercase() != term_lower {
                    return Some(cleaned.to_string());
                }
            }
        }
    }
    None
}

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

static RETRAIN_SCHEDULED: AtomicBool = AtomicBool::new(false);
static RETRAIN_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_EDIT_EPOCH: AtomicI64 = AtomicI64::new(0);
static RETRAIN_STARTED_AT: AtomicI64 = AtomicI64::new(0);
static RETRAIN_FINISHED_AT: AtomicI64 = AtomicI64::new(0);
static RETRAIN_DURATION_MS: AtomicI64 = AtomicI64::new(0);
static RETRAIN_SUCCESS: AtomicBool = AtomicBool::new(false);

const DEBOUNCE_SECS: u64 = 5;

// ── GET /v1/retrain-status ──────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RetrainStatus {
    pub scheduled: bool,
    pub running: bool,
    pub started_at: i64,
    pub finished_at: i64,
    pub duration_ms: i64,
    pub success: bool,
}

pub async fn retrain_status() -> Json<RetrainStatus> {
    Json(RetrainStatus {
        scheduled: RETRAIN_SCHEDULED.load(Ordering::SeqCst),
        running: RETRAIN_RUNNING.load(Ordering::SeqCst),
        started_at: RETRAIN_STARTED_AT.load(Ordering::SeqCst),
        finished_at: RETRAIN_FINISHED_AT.load(Ordering::SeqCst),
        duration_ms: RETRAIN_DURATION_MS.load(Ordering::SeqCst),
        success: RETRAIN_SUCCESS.load(Ordering::SeqCst),
    })
}

pub fn schedule_retrain_public(state: crate::AppState) {
    schedule_onnx_retrain(state);
}

fn schedule_onnx_retrain(state: crate::AppState) {
    let epoch = crate::store::now_ms();
    LAST_EDIT_EPOCH.store(epoch, Ordering::SeqCst);

    if RETRAIN_SCHEDULED.swap(true, Ordering::SeqCst) {
        info!("[retrain] edit batched — timer resets to {DEBOUNCE_SECS}s");
        return;
    }

    let db_path = crate::store::default_db_path();
    let uid = state.default_user_id.to_string();

    tokio::spawn(async move {
        loop {
            info!("[retrain] waiting {DEBOUNCE_SECS}s for edits to settle...");
            tokio::time::sleep(std::time::Duration::from_secs(DEBOUNCE_SECS)).await;

            let now = crate::store::now_ms();
            let last = LAST_EDIT_EPOCH.load(Ordering::SeqCst);
            if now - last < (DEBOUNCE_SECS as i64 * 1000) {
                info!("[retrain] new edit arrived — resetting timer");
                continue;
            }
            break;
        }

        RETRAIN_SCHEDULED.store(false, Ordering::SeqCst);

        if RETRAIN_RUNNING.swap(true, Ordering::SeqCst) {
            info!("[retrain] another train already running — re-scheduling");
            RETRAIN_SCHEDULED.store(false, Ordering::SeqCst);
            schedule_onnx_retrain(state);
            return;
        }

        RETRAIN_STARTED_AT.store(crate::store::now_ms(), Ordering::SeqCst);
        RETRAIN_FINISHED_AT.store(0, Ordering::SeqCst);

        let db = db_path.clone();
        let user = uid.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let train_start = std::time::Instant::now();
            info!("[retrain] ══════ ONNX RETRAIN STARTED (--micro) ══════");
            let script = std::path::Path::new("tools/tier2/train_correction_model.py");
            let script_path = if script.is_file() {
                script.to_path_buf()
            } else if let Ok(exe) = std::env::current_exe() {
                exe.parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .map(|root| root.join("tools/tier2/train_correction_model.py"))
                    .unwrap_or_default()
            } else {
                warn!("[retrain] ══════ ONNX RETRAIN ABORTED — cannot find exe path ══════");
                return;
            };
            if !script_path.is_file() {
                warn!(
                    "[retrain] ══════ ONNX RETRAIN SKIPPED — script not found at {} ══════",
                    script_path.display()
                );
                return;
            }
            info!(
                "[retrain] script={} db={}",
                script_path.display(),
                db.display()
            );
            match std::process::Command::new("python3")
                .arg(&script_path)
                .arg("--db")
                .arg(db.to_str().unwrap_or_default())
                .arg("--user-id")
                .arg(&user)
                .arg("--micro")
                .output()
            {
                Ok(out) => {
                    let elapsed = train_start.elapsed();
                    let dur_ms = elapsed.as_millis() as i64;
                    if out.status.success() {
                        let stdout = String::from_utf8_lossy(&out.stdout);
                        for line in stdout.lines() {
                            info!("[retrain]   {line}");
                        }
                        RETRAIN_SUCCESS.store(true, Ordering::SeqCst);
                        info!(
                            "[retrain] ══════ ONNX RETRAIN FINISHED in {:.1}s ══════",
                            elapsed.as_secs_f64()
                        );
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr);
                        for line in stderr.lines().rev().take(5) {
                            warn!("[retrain]   {line}");
                        }
                        RETRAIN_SUCCESS.store(false, Ordering::SeqCst);
                        warn!(
                            "[retrain] ══════ ONNX RETRAIN FAILED after {:.1}s (exit={}) ══════",
                            elapsed.as_secs_f64(),
                            out.status,
                        );
                    }
                    RETRAIN_DURATION_MS.store(dur_ms, Ordering::SeqCst);
                    RETRAIN_FINISHED_AT.store(crate::store::now_ms(), Ordering::SeqCst);
                }
                Err(e) => {
                    RETRAIN_SUCCESS.store(false, Ordering::SeqCst);
                    RETRAIN_FINISHED_AT.store(crate::store::now_ms(), Ordering::SeqCst);
                    warn!("[retrain] ══════ ONNX RETRAIN SPAWN FAILED: {e} ══════");
                }
            }
        })
        .await;

        RETRAIN_RUNNING.store(false, Ordering::SeqCst);
    });
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

// ── Deterministic hunk classifier ────────────────────────────────────────────

/// Classify edit hunks deterministically — no LLM needed.
///
/// For each hunk from `edit_diff::diff()`, compare transcript/polish/kept
/// windows to determine the change reason:
///   - transcript ≈ polish ≠ kept → stt_error (both STT and polish had wrong word)
///   - transcript ≠ polish, kept ≈ transcript → polish_error (user reverted to transcript)
///   - format pattern match → format_preference
///   - everything else → style_preference (not learnable)
fn deterministic_classify_hunks(
    hunks: &[edit_diff::Hunk],
    _transcript: &str,
    _polished: &str,
    user_kept: &str,
    pool: &crate::store::DbPool,
    user_id: &str,
) -> Vec<AnalyzedChange> {
    let mut changes = Vec::new();

    for hunk in hunks {
        let kept = hunk.kept_window.trim();
        let polish = hunk.polish_window.trim();

        if kept.is_empty() && polish.is_empty() {
            continue;
        }

        // Pure deletion — user removed words from polish
        if kept.is_empty() {
            continue;
        }

        // Pure insertion — user added words not in polish
        if polish.is_empty() {
            changes.push(AnalyzedChange {
                original: String::new(),
                corrected: kept.to_string(),
                reason: ChangeReason::StructuralRewrite,
                meaning: None,
                context_example: None,
                should_learn: false,
                confidence: 0.8,
                skip_reason: Some("user inserted new content".into()),
                format_rule: None,
            });
            continue;
        }

        let kept_tokens = token_surfaces(kept);
        let polish_tokens = token_surfaces(polish);

        if kept_tokens.len() > 4 || polish_tokens.len() > 4 {
            changes.push(AnalyzedChange {
                original: polish.to_string(),
                corrected: kept.to_string(),
                reason: ChangeReason::StructuralRewrite,
                meaning: None,
                context_example: None,
                should_learn: false,
                confidence: 0.7,
                skip_reason: Some("large structural change".into()),
                format_rule: None,
            });
            continue;
        }

        // ── Multi-token hunk with equal counts → split into per-token pairs ──
        // "automot hi" vs "AutoNote ye" → ("automot"→"AutoNote") + ("hi"→"ye")
        // Each sub-pair is classified independently so grammar changes don't
        // pollute word corrections.
        if kept_tokens.len() == polish_tokens.len() && kept_tokens.len() > 1 {
            let transcript_tokens = token_surfaces(hunk.transcript_window.trim());
            for (idx, (p_tok, k_tok)) in polish_tokens.iter().zip(kept_tokens.iter()).enumerate() {
                if p_tok.to_ascii_lowercase() == k_tok.to_ascii_lowercase() {
                    continue;
                }
                let t_tok = transcript_tokens.get(idx).map(|s| s.as_str()).unwrap_or("");
                let sub_change =
                    classify_single_token_change(p_tok, k_tok, t_tok, user_kept, pool, user_id);
                changes.push(sub_change);
            }
            continue;
        }

        let transcript_window = hunk.transcript_window.trim();

        // Check format preference first (time, date, email patterns)
        if is_format_change(polish, kept) {
            changes.push(AnalyzedChange {
                original: polish.to_string(),
                corrected: kept.to_string(),
                reason: ChangeReason::FormatPreference,
                meaning: None,
                context_example: surrounding_sentence(user_kept, kept),
                should_learn: true,
                confidence: 0.9,
                skip_reason: None,
                format_rule: Some(infer_format_rule(polish, kept)),
            });
            continue;
        }

        // Single-token or unequal-count hunk — classify as a whole
        let kept_norm = kept.to_ascii_lowercase();
        let transcript_norm = transcript_window.to_ascii_lowercase();
        let polish_norm = polish.to_ascii_lowercase();

        // Polish error: user reverted to what STT heard
        if !transcript_window.is_empty()
            && transcript_norm != polish_norm
            && kept_norm == transcript_norm
        {
            changes.push(AnalyzedChange {
                original: polish.to_string(),
                corrected: kept.to_string(),
                reason: ChangeReason::PolishError,
                meaning: None,
                context_example: surrounding_sentence(user_kept, kept),
                should_learn: true,
                confidence: 0.95,
                skip_reason: None,
                format_rule: None,
            });
            continue;
        }

        // STT error: both STT and polish had the wrong word, user corrected it
        if kept_norm != polish_norm {
            let corrected_surface = clean_surface(kept);
            let original_surface = clean_surface(polish);

            if corrected_surface.is_empty() || original_surface.is_empty() {
                continue;
            }

            // Check if the corrected term is already in vocabulary
            let in_vocab = vocabulary::find_by_term_ci(pool, user_id, &corrected_surface).is_some();
            // Check if original is a common word (Hindi/English)
            let original_is_common = promotion_gate::is_common_word(&original_surface);
            // Check if corrected is a common/stop word — don't learn stop words
            let corrected_is_common = promotion_gate::is_common_word(&corrected_surface);

            if corrected_is_common {
                changes.push(AnalyzedChange {
                    original: original_surface,
                    corrected: corrected_surface,
                    reason: ChangeReason::StylePreference,
                    meaning: None,
                    context_example: None,
                    should_learn: false,
                    confidence: 0.8,
                    skip_reason: Some("corrected form is a common word".into()),
                    format_rule: None,
                });
                continue;
            }

            // If original is a common Hindi/English word and corrected is NOT
            // in vocabulary, this might be rephrasing, not STT error.
            // But if corrected IS in vocabulary, STT likely garbled it.
            if original_is_common && !in_vocab {
                changes.push(AnalyzedChange {
                    original: original_surface,
                    corrected: corrected_surface,
                    reason: ChangeReason::StylePreference,
                    meaning: None,
                    context_example: None,
                    should_learn: false,
                    confidence: 0.6,
                    skip_reason: Some(
                        "original is a real word, corrected not yet in vocab — might be rephrasing"
                            .into(),
                    ),
                    format_rule: None,
                });
                continue;
            }

            // Genuine STT correction
            let ctx = surrounding_sentence(user_kept, &corrected_surface);
            changes.push(AnalyzedChange {
                original: original_surface,
                corrected: corrected_surface,
                reason: ChangeReason::SttError,
                meaning: None, // will be filled by Groq if needed
                context_example: ctx,
                should_learn: true,
                confidence: if in_vocab { 0.95 } else { 0.85 },
                skip_reason: None,
                format_rule: None,
            });
            continue;
        }

        // Same word, different casing only
        if kept_norm == polish_norm && kept != polish {
            continue;
        }
    }

    // Log for debugging
    for ch in &changes {
        info!(
            "[classify-det] {:?} → {:?} reason={} learn={} conf={:.2}",
            ch.original,
            ch.corrected,
            ch.reason.as_str(),
            ch.should_learn,
            ch.confidence,
        );
    }

    changes
}

/// Classify a single token pair (extracted from a multi-token hunk).
fn classify_single_token_change(
    polish_tok: &str,
    kept_tok: &str,
    transcript_tok: &str,
    user_kept: &str,
    pool: &crate::store::DbPool,
    user_id: &str,
) -> AnalyzedChange {
    let p_norm = polish_tok.to_ascii_lowercase();
    let k_norm = kept_tok.to_ascii_lowercase();
    let t_norm = transcript_tok.to_ascii_lowercase();

    // Polish error: user reverted to transcript
    if !t_norm.is_empty() && t_norm != p_norm && k_norm == t_norm {
        return AnalyzedChange {
            original: polish_tok.to_string(),
            corrected: kept_tok.to_string(),
            reason: ChangeReason::PolishError,
            meaning: None,
            context_example: surrounding_sentence(user_kept, kept_tok),
            should_learn: true,
            confidence: 0.95,
            skip_reason: None,
            format_rule: None,
        };
    }

    let corrected = clean_surface(kept_tok);
    let original = clean_surface(polish_tok);
    if corrected.is_empty() || original.is_empty() {
        return AnalyzedChange {
            original,
            corrected,
            reason: ChangeReason::StylePreference,
            meaning: None,
            context_example: None,
            should_learn: false,
            confidence: 0.5,
            skip_reason: Some("empty token".into()),
            format_rule: None,
        };
    }

    let in_vocab = vocabulary::find_by_term_ci(pool, user_id, &corrected).is_some();
    let original_is_common = promotion_gate::is_common_word(&original);
    let corrected_is_common = promotion_gate::is_common_word(&corrected);

    if corrected_is_common {
        return AnalyzedChange {
            original,
            corrected,
            reason: ChangeReason::StylePreference,
            meaning: None,
            context_example: None,
            should_learn: false,
            confidence: 0.8,
            skip_reason: Some("corrected form is a common word".into()),
            format_rule: None,
        };
    }

    if original_is_common && !in_vocab {
        return AnalyzedChange {
            original,
            corrected,
            reason: ChangeReason::StylePreference,
            meaning: None,
            context_example: None,
            should_learn: false,
            confidence: 0.6,
            skip_reason: Some("original is a real word, corrected not in vocab".into()),
            format_rule: None,
        };
    }

    AnalyzedChange {
        original,
        corrected,
        reason: ChangeReason::SttError,
        meaning: None,
        context_example: surrounding_sentence(user_kept, &clean_surface(kept_tok)),
        should_learn: true,
        confidence: if in_vocab { 0.95 } else { 0.85 },
        skip_reason: None,
        format_rule: None,
    }
}

/// Detect format-related changes (time, date, numbers, email, URL).
fn is_format_change(polish: &str, kept: &str) -> bool {
    let time_re = regex::Regex::new(r"(?i)\d{1,2}:\d{2}\s*(am|pm)?").unwrap();
    let date_re = regex::Regex::new(r"\d{1,2}[/\-]\d{1,2}([/\-]\d{2,4})?").unwrap();
    let email_re = regex::Regex::new(r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}").unwrap();
    let number_re = regex::Regex::new(r"^\d[\d,.]*%?$").unwrap();

    let p_has_format = time_re.is_match(polish)
        || date_re.is_match(polish)
        || email_re.is_match(polish)
        || number_re.is_match(polish.trim());
    let k_has_format = time_re.is_match(kept)
        || date_re.is_match(kept)
        || email_re.is_match(kept)
        || number_re.is_match(kept.trim());

    p_has_format || k_has_format
}

fn infer_format_rule(polish: &str, kept: &str) -> String {
    let time_re = regex::Regex::new(r"(?i)\d{1,2}:\d{2}\s*(am|pm)?").unwrap();
    let date_re = regex::Regex::new(r"\d{1,2}[/\-]\d{1,2}").unwrap();
    let email_re = regex::Regex::new(r"@").unwrap();

    if time_re.is_match(kept) || time_re.is_match(polish) {
        format!("time format: {polish:?} → {kept:?}")
    } else if date_re.is_match(kept) || date_re.is_match(polish) {
        format!("date format: {polish:?} → {kept:?}")
    } else if email_re.is_match(kept) {
        format!("email format: {polish:?} → {kept:?}")
    } else {
        format!("number/format: {polish:?} → {kept:?}")
    }
}

/// Generate a meaning for a term using Groq (simple prompt, no classification).
async fn generate_meaning_for_term(
    http: &reqwest::Client,
    groq_key: &str,
    term: &str,
    context: &str,
) -> Option<String> {
    use serde_json::json;

    let prompt = format!(
        "Given this sentence: \"{context}\"\n\
         The term \"{term}\" is a proper noun, technical term, or brand name.\n\
         Draft a concise 1-sentence meaning for \"{term}\" based on the context.\n\
         Return ONLY the meaning text, nothing else."
    );

    let body = json!({
        "model": "llama-3.1-8b-instant",
        "temperature": 0.1,
        "max_tokens": 100,
        "messages": [
            { "role": "user", "content": prompt }
        ]
    });

    let resp = http
        .post("https://api.groq.com/openai/v1/chat/completions")
        .header("Authorization", format!("Bearer {groq_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let json: serde_json::Value = resp.json().await.ok()?;
    let meaning = json
        .get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()?
        .trim()
        .trim_matches('"')
        .to_string();

    if meaning.is_empty() {
        None
    } else {
        info!("[classify-det] groq meaning for {term:?}: {meaning:?}");
        Some(meaning)
    }
}

// ── Deterministic edit pair extraction (used by Step 5) ─────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeterministicEditPair {
    variant_form: String,
    correct_form: String,
    edit_type: String,
    left_context: Vec<String>,
    right_context: Vec<String>,
}

fn deterministic_stt_edit_pair(
    change: &AnalyzedChange,
    hunks: &[edit_diff::Hunk],
    user_kept: &str,
) -> Option<DeterministicEditPair> {
    let corrected_hint = clean_surface(&change.corrected);
    if corrected_hint.is_empty() {
        return None;
    }

    for hunk in hunks {
        let kept_window = hunk.kept_window.trim();
        if kept_window.is_empty() {
            continue;
        }
        let Some(correct_form) = corrected_from_hunk(kept_window, &corrected_hint) else {
            continue;
        };

        let source_window = if !hunk.transcript_window.trim().is_empty() {
            hunk.transcript_window.trim()
        } else {
            hunk.polish_window.trim()
        };
        let edit_type = if source_window.is_empty() {
            "insert"
        } else {
            "replace"
        };
        let variant_form = if edit_type == "insert" {
            String::new()
        } else {
            source_from_hunk(source_window, &change.original)?
        };
        if edit_type == "replace" {
            let source_tokens = token_surfaces(&variant_form);
            if source_tokens.len() != 1 {
                continue;
            }
            let source_norm = tier2_edit_policy::normalize_token(&variant_form);
            let correct_norm = tier2_edit_policy::normalize_token(&correct_form);
            if source_norm.is_empty() || source_norm == correct_norm {
                continue;
            }
        }

        let (left_context, right_context) = context_around_kept_term(user_kept, &correct_form, 3);
        return Some(DeterministicEditPair {
            variant_form,
            correct_form,
            edit_type: edit_type.to_string(),
            left_context,
            right_context,
        });
    }

    None
}

fn preferred_corrected_surface(diff_surface: &str, analyzer_surface: &str) -> String {
    let diff_surface = clean_surface(diff_surface);
    let analyzer_surface = clean_surface(analyzer_surface);
    if diff_surface.is_empty() || analyzer_surface.is_empty() {
        return canonicalize_corrected_surface(diff_surface);
    }
    let preferred = if tier2_edit_policy::normalize_token(&diff_surface)
        != tier2_edit_policy::normalize_token(&analyzer_surface)
    {
        diff_surface
    } else if surface_shape_rank(&analyzer_surface) > surface_shape_rank(&diff_surface) {
        analyzer_surface
    } else {
        diff_surface
    };
    canonicalize_corrected_surface(preferred)
}

fn surface_shape_rank(surface: &str) -> u8 {
    match vocabulary::classify_term_type(surface) {
        "acronym" | "brand" | "proper_noun" | "code_identifier" => 2,
        _ if surface.chars().any(|c| c.is_ascii_uppercase()) => 1,
        _ => 0,
    }
}

fn canonicalize_corrected_surface(surface: String) -> String {
    canonical_developer_term(&surface)
        .map(str::to_string)
        .unwrap_or(surface)
}

fn canonical_developer_term(surface: &str) -> Option<&'static str> {
    let norm = tier2_edit_policy::normalize_token(surface);
    match norm.as_str() {
        "docker" => Some("Docker"),
        "emiac" => Some("EMIAC"),
        "github" => Some("GitHub"),
        "gitlab" => Some("GitLab"),
        "graphql" => Some("GraphQL"),
        "javascript" => Some("JavaScript"),
        "jwt" => Some("JWT"),
        "kubectl" => Some("kubectl"),
        "kubernetes" => Some("Kubernetes"),
        "localhost" => Some("localhost"),
        "macobs" => Some("Macobs"),
        "oauth" => Some("OAuth"),
        "postgresql" => Some("PostgreSQL"),
        "prisma" => Some("Prisma"),
        "redis" => Some("Redis"),
        "sqlite" => Some("SQLite"),
        "supabase" => Some("Supabase"),
        "tauri" => Some("Tauri"),
        "typescript" => Some("TypeScript"),
        "vercel" => Some("Vercel"),
        "vite" => Some("Vite"),
        "websocket" => Some("WebSocket"),
        _ => None,
    }
}

fn corrected_from_hunk(kept_window: &str, corrected_hint: &str) -> Option<String> {
    let kept_tokens = token_surfaces(kept_window);
    if kept_tokens.len() == 1 {
        return kept_tokens.into_iter().next();
    }
    contains_normalized_token(kept_window, corrected_hint).then(|| corrected_hint.to_string())
}

fn source_from_hunk(source_window: &str, original_hint: &str) -> Option<String> {
    let source_tokens = token_surfaces(source_window);
    if source_tokens.len() == 1 {
        return source_tokens.into_iter().next();
    }
    let original_hint = clean_surface(original_hint);
    if !original_hint.is_empty() && contains_normalized_token(source_window, &original_hint) {
        return Some(original_hint);
    }
    None
}

fn context_around_kept_term(text: &str, term: &str, radius: usize) -> (Vec<String>, Vec<String>) {
    let term_tokens = token_surfaces(term);
    let term_norms = term_tokens
        .iter()
        .map(|token| tier2_edit_policy::normalize_token(token))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if term_norms.is_empty() {
        return (vec![], vec![]);
    }
    let tokens = token_surfaces(text);
    let norms = tokens
        .iter()
        .map(|token| tier2_edit_policy::normalize_token(token))
        .collect::<Vec<_>>();
    let Some(start) = norms
        .windows(term_norms.len())
        .position(|window| window == term_norms.as_slice())
    else {
        return (vec![], vec![]);
    };
    let end = start + term_norms.len();
    let left_start = start.saturating_sub(radius);
    let left = tokens[left_start..start].to_vec();
    let right = tokens[end..tokens.len().min(end + radius)].to_vec();
    (left, right)
}

fn contains_normalized_token(text: &str, needle: &str) -> bool {
    let needle = tier2_edit_policy::normalize_token(needle);
    !needle.is_empty()
        && token_surfaces(text)
            .iter()
            .any(|token| tier2_edit_policy::normalize_token(token) == needle)
}

fn token_surfaces(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(clean_surface)
        .filter(|token| !token.is_empty())
        .collect()
}

fn clean_surface(text: &str) -> String {
    text.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .trim()
        .to_string()
}

fn unsafe_stt_source_reason(
    pool: &crate::store::DbPool,
    user_id: &str,
    source: &str,
) -> Option<String> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    if promotion_gate::is_common_word(source) {
        return Some("source is a common word".to_string());
    }
    existing_protected_term_in_text(pool, user_id, source)
        .map(|term| format!("source already contains protected term {term:?}"))
}

fn existing_protected_term_in_text(
    pool: &crate::store::DbPool,
    user_id: &str,
    text: &str,
) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(term) = protected_vocab_lookup(pool, user_id, trimmed) {
        return Some(term);
    }
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter(|part| !part.trim().is_empty())
        .find_map(|part| protected_vocab_lookup(pool, user_id, part))
}

fn protected_vocab_lookup(
    pool: &crate::store::DbPool,
    user_id: &str,
    term: &str,
) -> Option<String> {
    let existing = vocabulary::find_by_term_ci(pool, user_id, term)?;
    let term_type = existing
        .term_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| vocabulary::classify_term_type(&existing.term).to_string());
    let source = existing.source.trim();
    let protected = source == "manual"
        || source == "starred"
        || matches!(
            term_type.as_str(),
            "brand" | "acronym" | "proper_noun" | "code_identifier"
        );
    protected.then_some(existing.term)
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
        ambiguous_terms: vec![],
        negative_terms: vec![],
        review_candidates: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r2d2_sqlite::SqliteConnectionManager;

    fn mem_pool() -> crate::store::DbPool {
        let mgr = SqliteConnectionManager::memory();
        let pool = r2d2::Pool::builder().max_size(1).build(mgr).unwrap();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "CREATE TABLE local_user (id TEXT PRIMARY KEY);
             INSERT INTO local_user(id) VALUES ('u1');
             CREATE TABLE vocabulary (
                 user_id                 TEXT NOT NULL REFERENCES local_user(id),
                 term                    TEXT NOT NULL,
                 weight                  REAL NOT NULL DEFAULT 1.0,
                 use_count               INTEGER NOT NULL DEFAULT 1,
                 last_used               INTEGER NOT NULL,
                 source                  TEXT NOT NULL DEFAULT 'auto',
                 language                TEXT,
                 example_context         TEXT,
                 term_type               TEXT,
                 meaning                 TEXT,
                 meaning_updated_at      INTEGER,
                 examples_since_meaning  INTEGER NOT NULL DEFAULT 0,
                 UNIQUE(user_id, term)
             );",
        )
        .unwrap();
        pool
    }

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

    #[test]
    fn protected_vocab_source_is_not_safe_for_stt_learning() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "Macobs", 1.0, "auto"));
        assert_eq!(
            unsafe_stt_source_reason(&pool, "u1", "Macobs")
                .as_deref()
                .unwrap_or_default(),
            "source already contains protected term \"Macobs\""
        );
        assert!(unsafe_stt_source_reason(&pool, "u1", "Macobs mein").is_some());
        assert!(unsafe_stt_source_reason(&pool, "u1", "bimmicop").is_none());
    }

    #[test]
    fn common_word_source_is_not_safe_for_stt_learning() {
        let pool = mem_pool();
        assert_eq!(
            unsafe_stt_source_reason(&pool, "u1", "main").as_deref(),
            Some("source is a common word")
        );
    }

    fn stt_change(original: &str, corrected: &str) -> AnalyzedChange {
        AnalyzedChange {
            original: original.to_string(),
            corrected: corrected.to_string(),
            reason: ChangeReason::SttError,
            meaning: None,
            context_example: None,
            should_learn: true,
            confidence: 0.9,
            skip_reason: None,
            format_rule: None,
        }
    }

    #[test]
    fn deterministic_pair_accepts_real_diff_span() {
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff("macops ka data bhejo", "macops ka data bhejo", kept);
        let pair = deterministic_stt_edit_pair(&stt_change("macops", "Macobs"), &hunks, kept)
            .expect("expected deterministic pair");

        assert_eq!(pair.variant_form, "macops");
        assert_eq!(pair.correct_form, "Macobs");
        assert_eq!(pair.edit_type, "replace");
        assert_eq!(pair.right_context, vec!["ka", "data", "bhejo"]);
    }

    #[test]
    fn deterministic_pair_uses_real_hunk_not_analyzer_invention() {
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff("macops ka data bhejo", "macops ka data bhejo", kept);
        let pair = deterministic_stt_edit_pair(&stt_change("Macobs", "EMIAC"), &hunks, kept);

        let pair = pair.expect("real one-token hunk should still be usable");
        assert_eq!(pair.variant_form, "macops");
        assert_eq!(pair.correct_form, "Macobs");
    }

    #[test]
    fn deterministic_pair_records_insert_candidate() {
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff("ka data bhejo", "ka data bhejo", kept);
        let pair = deterministic_stt_edit_pair(&stt_change("", "Macobs"), &hunks, kept)
            .expect("expected insert pair");

        assert_eq!(pair.variant_form, "");
        assert_eq!(pair.correct_form, "Macobs");
        assert_eq!(pair.edit_type, "insert");
        assert_eq!(pair.right_context, vec!["ka", "data", "bhejo"]);
    }

    #[test]
    fn deterministic_pair_rejects_case_only_change() {
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff("macobs ka data bhejo", "macobs ka data bhejo", kept);
        let pair = deterministic_stt_edit_pair(&stt_change("macobs", "Macobs"), &hunks, kept);

        assert!(pair.is_none());
    }

    #[test]
    fn preferred_corrected_surface_keeps_analyzer_canonical_casing() {
        assert_eq!(
            preferred_corrected_surface("kubernetes", "Kubernetes"),
            "Kubernetes"
        );
        assert_eq!(preferred_corrected_surface("macobs", "Macobs"), "Macobs");
        assert_eq!(preferred_corrected_surface("jwt", "JWT"), "JWT");
    }

    #[test]
    fn preferred_corrected_surface_rejects_unrelated_analyzer_surface() {
        assert_eq!(
            preferred_corrected_surface("kubernetes", "Docker"),
            "Kubernetes"
        );
    }

    #[test]
    fn known_developer_terms_are_canonicalized_without_existing_vocab() {
        let cases = [
            ("docker", "Docker"),
            ("emiac", "EMIAC"),
            ("github", "GitHub"),
            ("graphql", "GraphQL"),
            ("javascript", "JavaScript"),
            ("jwt", "JWT"),
            ("kubernetes", "Kubernetes"),
            ("macobs", "Macobs"),
            ("oauth", "OAuth"),
            ("postgresql", "PostgreSQL"),
            ("sqlite", "SQLite"),
            ("typescript", "TypeScript"),
            ("websocket", "WebSocket"),
        ];
        for (surface, canonical) in cases {
            assert_eq!(
                preferred_corrected_surface(surface, surface),
                canonical,
                "{surface} should canonicalize"
            );
        }
    }
}
