//! POST /v1/classify-edit  —  Learning Pipeline v3
//!
//! Deterministic edit classification with a narrow LLM fallback only for
//! complex edit interpretation and meaning generation. Replaces the v2
//! LLM-driven pipeline:
//!
//!   1. **Capture gate** (cheap): reject stale / clipboard / app-switched edits
//!   2. **Branch** — no-edit (reward active vocab), full deletion, or stale
//!   3. **Demotion** — unconditional negative signal for removed terms
//!   4. **Deterministic classifier** — classify hunks from diff without LLM
//!   5. **Complex edit interpreter** — DeepSeek may propose spans, but only real
//!      transcript/output/kept spans survive verification
//!   6. **Meaning generation** — background call ONLY for new STT correction terms
//!   7. **Save** — persist learnable changes by reason type

use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::{
    AppState,
    llm::{
        alias_safety,
        analyzer::{self, AnalyzedChange, ChangeReason},
        edit_diff, promotion_gate,
    },
    store::{
        edit_review_sessions, email_memory, history, prefs::get_prefs, stt_replacements,
        tier2_edit_policy, users, vectors, vocabulary,
    },
};

#[derive(Deserialize)]
pub struct ClassifyBody {
    pub recording_id: String,
    pub ai_output: String,
    pub user_kept: String,
    /// How the desktop captured the edit text. Only atomic, field-owned capture
    /// methods (`ax` and `keystroke_verified`) may reach learning. Missing
    /// values default to `ax` for backward compatibility; unknown values fail closed.
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
    /// Desktop runtime session id for correlating with control-plane `runtime_sessions`.
    #[serde(default)]
    pub client_run_id: Option<String>,
    /// The text already in the focused field BEFORE our paste (the pre-dictation
    /// baseline). When the user dictated into a field that already had content,
    /// the desktop reads the WHOLE field as `user_kept`; this lets the classifier
    /// strip the pre-existing prefix/suffix and diff only OUR output + the user's
    /// edits to it. Empty/None → field was empty, `user_kept` is used as-is.
    #[serde(default)]
    pub prior_text: Option<String>,
    #[serde(default)]
    pub edit_trace_json: Option<serde_json::Value>,
}

/// Strip the pre-existing field text so the edit diff sees only our output.
///
/// The field after the user's edit is `prior_prefix + edited_output + prior_suffix`
/// where `prior_prefix + prior_suffix == prior_text` (the caret split the baseline
/// when we pasted). We recover `edited_output` by matching the baseline's prefix and
/// suffix against `user_kept`. If the baseline can't be cleanly located (e.g. the
/// user also edited the surrounding text), we fall back to the full field — never
/// worse than today's behaviour.
fn scope_to_our_output(user_kept: &str, prior_text: Option<&str>) -> String {
    let prior = match prior_text {
        Some(p) if !p.is_empty() => p,
        _ => return user_kept.to_string(),
    };
    let kept: Vec<char> = user_kept.chars().collect();
    let prior_c: Vec<char> = prior.chars().collect();

    // A = common prefix of (prior, kept) — the pre-existing text before our paste.
    let mut a = 0usize;
    while a < prior_c.len() && a < kept.len() && prior_c[a] == kept[a] {
        a += 1;
    }
    // B = the rest of the baseline; the field must still end with it.
    let b_chars = &prior_c[a..];
    let b = b_chars.len();
    if a + b <= kept.len() && kept[kept.len() - b..] == *b_chars {
        return kept[a..kept.len() - b].iter().collect();
    }
    // Baseline not cleanly present (surrounding text was also edited) — keep full.
    user_kept.to_string()
}

fn default_capture_method() -> String {
    "ax".to_string()
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn post_runtime_memory_dirty(state: AppState) {
    tokio::spawn(async move {
        let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
            return;
        };
        let Some(token) = user.cloud_token.filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let base_url = user
            .enterprise_server_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
        let url = format!("{}/v1/runtime/memory/dirty", base_url.trim_end_matches('/'));
        let _ = state
            .http_client
            .post(url)
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;
    });
}

fn post_runtime_client_event(
    state: AppState,
    event_type: &'static str,
    recording_id: String,
    classification: String,
    ai_output_hash: String,
    user_kept_hash: String,
    payload: serde_json::Value,
) {
    tokio::spawn(async move {
        let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
            return;
        };
        let Some(token) = user.cloud_token.filter(|t| !t.trim().is_empty()) else {
            return;
        };
        let base_url = user
            .enterprise_server_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());
        let url = format!(
            "{}/v1/runtime/client-events",
            base_url.trim_end_matches('/')
        );
        let body = serde_json::json!({
            "event_type": event_type,
            "recording_id": recording_id,
            "classification": classification,
            "input_hash": ai_output_hash,
            "corrected_hash": user_kept_hash,
            "payload": payload,
        });
        match state
            .http_client
            .post(url)
            .bearer_auth(token)
            .json(&body)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                info!("[classify] runtime client event uploaded");
            }
            Ok(resp) => {
                warn!(
                    "[classify] runtime client event upload failed: {}",
                    resp.status()
                );
            }
            Err(e) => {
                warn!("[classify] runtime client event upload failed: {e}");
            }
        }
    });
}

/// Maximum elapsed-since-paste before we treat the edit as unrelated to
/// our paste. Desktop edit-watch intentionally gives users a generous reading
/// window before the first edit and a second quiet window after the last edit,
/// so this must stay wider than the desktop watcher's hard cap.
const CAPTURE_STALE_MS: u64 = 180_000;

fn capture_is_stale(time_since_paste_ms: u64) -> bool {
    time_since_paste_ms > CAPTURE_STALE_MS
}

/// Stricter subset: captures whose source is an *atomic* read of a specific
/// text element. An AX read returning a value means it came from the targeted
/// element at that moment; a focus change after the read doesn't invalidate it.
fn is_high_confidence_capture(capture_method: &str) -> bool {
    matches!(capture_method, "ax" | "keystroke_verified")
}

fn is_surrounding_continuation(ai_output: &str, user_kept: &str) -> bool {
    let output = ai_output.trim();
    let kept = user_kept.trim();
    if output.is_empty() || kept == output || kept.len() <= output.len() {
        return false;
    }
    let Some(offset) = kept.find(output) else {
        return false;
    };
    let end = offset + output.len();
    !kept[..offset].trim().is_empty() || !kept[end..].trim().is_empty()
}

#[derive(Serialize)]
pub struct ClassifyResponse {
    pub class: String,
    pub reason: String,
    pub pending_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_session_id: Option<String>,
    pub learned: bool,
    pub notify: bool,
    pub promoted_count: usize,
    pub is_repeat: bool,
    pub promoted_terms: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub learned_emails: Vec<String>,
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
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ReviewCandidate {
    pub original: String,
    pub corrected: String,
    pub term_type: String,
    pub learnable: bool,
    pub tag: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
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
    let learning_enabled = get_prefs(&state.pool, &state.default_user_id)
        .map(|p| p.learning_enabled)
        .unwrap_or(true);
    crate::legacy_learning::with_legacy_write_scope(
        learning_enabled,
        classify_inner(state, body, learning_enabled),
    )
    .await
}

async fn classify_inner(
    state: AppState,
    mut body: ClassifyBody,
    _learning_enabled: bool,
) -> (StatusCode, Json<ClassifyResponse>) {
    let audit_only = crate::legacy_learning::audit_only_legacy_mutations();
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
    if audit_only {
        info!(
            "[classify] learning disabled — audit-only for {}",
            body.recording_id,
        );
    }
    let output_language = prefs
        .as_ref()
        .map(|p| p.output_language.clone())
        .unwrap_or_else(|| "hinglish".into());
    let groq_key = prefs
        .as_ref()
        .and_then(|p| p.groq_api_key.clone())
        .or_else(|| std::env::var("GROQ_API_KEY").ok())
        .or_else(|| std::env::var("GATEWAY_API_KEY").ok())
        .unwrap_or_default();
    let deepseek_key = crate::llm::deepseek::learning_api_key();

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
    if !is_high_confidence_capture(&body.capture_method) {
        info!(
            "[classify] capture_error: untrusted capture method {:?} for {}",
            body.capture_method, body.recording_id,
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "no_edit",
                "capture_error: capture source cannot prove field ownership",
            )),
        );
    }

    // ── Step 2: Branch — no edit / full deletion / stale ─────────────────────

    // Stale capture: outside the trusted desktop watcher lifecycle.
    if capture_is_stale(body.time_since_paste_ms) {
        info!(
            "[classify] stale capture ({}ms after paste) for {}",
            body.time_since_paste_ms, body.recording_id,
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "stale",
                "edit arrived after the trusted capture window",
            )),
        );
    }

    // ── Scope to OUR output ──────────────────────────────────────────────────
    // When the field already had text, the desktop reads the whole field. Strip
    // the pre-existing prefix/suffix so every downstream step (no-edit check,
    // diff, alias revert, email memory) only sees what we typed + the user's
    // edits to it — never the surrounding context that was already there.
    {
        let scoped = scope_to_our_output(&body.user_kept, body.prior_text.as_deref());
        if scoped != body.user_kept {
            info!(
                "[classify] scoped user_kept to our output region for {}: {} -> {} chars",
                body.recording_id,
                body.user_kept.chars().count(),
                scoped.chars().count(),
            );
            body.user_kept = scoped;
        }
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
        let rewarded = if audit_only {
            0
        } else {
            vocabulary::reward_active_terms(
                &state.pool,
                &state.default_user_id,
                &body.ai_output,
                0.1,
            )
        };
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

    if is_surrounding_continuation(&body.ai_output, &body.user_kept) {
        info!(
            "[classify] continuation excluded for {} — original output survived verbatim",
            body.recording_id,
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "no_edit",
                "continuation: text was added around the dictated output",
            )),
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

    let mut analyzer_changes = det_changes;
    let protected_insert_changes = protected_insert_changes_from_hunks(
        &edit_hunks,
        &body.user_kept,
        &state.pool,
        &state.default_user_id,
    );
    if !protected_insert_changes.is_empty() {
        info!(
            "[classify] protected insert extractor added {} change(s)",
            protected_insert_changes.len()
        );
        merge_llm_changes(&mut analyzer_changes, protected_insert_changes);
    }
    if needs_complex_edit_interpreter(&edit_hunks, &analyzer_changes) {
        let llm_changes = interpret_complex_edit_with_llm(
            &state.http_client,
            &deepseek_key,
            &transcript,
            &body.ai_output,
            &body.user_kept,
            &edit_hunks,
            &state.pool,
            &state.default_user_id,
        )
        .await;
        if !llm_changes.is_empty() {
            info!(
                "[classify] complex edit interpreter added {} verified change(s)",
                llm_changes.len()
            );
            merge_llm_changes(&mut analyzer_changes, llm_changes);
        }
    }

    // Collapse empty-original duplicates: when a substitution (e.g. max→EMIAC)
    // and a bare protected-insert of the same corrected (""→EMIAC) both fire,
    // keep only the substitution. The bare insert is what made the card show
    // "EMIAC — was —" next to the real swap.
    dedup_empty_original_by_corrected(&mut analyzer_changes);

    // No synchronous meaning LLM call — meaning is generated iteratively in the
    // background by spawn_vocab_embedding → spawn_meaning_refresh (GPT-5.4-mini).
    // Each sighting adds context; the meaning deepens over time.
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
        let protected_terms =
            protected_terms_in_span(&state.pool, &state.default_user_id, &change.corrected);
        let covered_by_protected_insert = protected_terms.iter().any(|term| {
            analyzer_changes.iter().any(|candidate| {
                candidate.reason == ChangeReason::SttError
                    && candidate.should_learn
                    && candidate.original.trim().is_empty()
                    && tier2_edit_policy::normalize_token(&candidate.corrected)
                        == tier2_edit_policy::normalize_token(term)
            })
        });
        if covered_by_protected_insert {
            info!(
                "[classify] ambiguous skipped — protected insert already covers {:?}",
                change.corrected
            );
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

    // One fail-closed decision owns every mutation below. A changed string is
    // not enough: it must come from an owned capture and produce a verified
    // deterministic change. Destructive alias/policy feedback additionally
    // requires explicit correction evidence, not a style rewrite.
    let is_correlated_edit = !edit_hunks.is_empty() && !analyzer_output.changes.is_empty();
    if !is_correlated_edit {
        info!(
            "[classify] uncorrelated edit excluded for {} — hunks={} changes={}",
            body.recording_id,
            edit_hunks.len(),
            analyzer_output.changes.len(),
        );
        return (
            StatusCode::OK,
            Json(empty_response(
                "no_edit",
                "correlation_error: no verified owned change",
            )),
        );
    }
    if audit_only {
        info!(
            "[classify] audit-only: {} verified change(s), all mutations skipped for {}",
            analyzer_output.changes.len(),
            body.recording_id,
        );
        let mut response = empty_response(
            &analyzer_output.overall_class,
            "learning disabled: verified changes were not persisted",
        );
        response.changes = analyzer_output.changes;
        return (StatusCode::OK, Json(response));
    }
    let has_correction_evidence = analyzer_output.changes.iter().any(|change| {
        change.should_learn
            && matches!(
                change.reason,
                ChangeReason::SttError | ChangeReason::PolishError | ChangeReason::FormatPreference
            )
    });
    let correction_pairs: std::collections::HashSet<(String, String)> = analyzer_output
        .changes
        .iter()
        .filter(|change| {
            change.should_learn
                && matches!(
                    change.reason,
                    ChangeReason::SttError
                        | ChangeReason::PolishError
                        | ChangeReason::FormatPreference
                )
        })
        .map(|change| {
            (
                tier2_edit_policy::normalize_token(&change.original),
                tier2_edit_policy::normalize_token(&change.corrected),
            )
        })
        .collect();
    let all_changes_are_corrections = !analyzer_output.changes.is_empty()
        && analyzer_output.changes.iter().all(|change| {
            change.should_learn
                && matches!(
                    change.reason,
                    ChangeReason::SttError
                        | ChangeReason::PolishError
                        | ChangeReason::FormatPreference
                )
        });

    let mut learned_emails = Vec::new();
    let mut negative_terms: Vec<NegativeTerm> = Vec::new();
    let mut policy_touched = false;

    history::apply_edit_feedback(&state.pool, &body.recording_id, &body.user_kept);

    if has_correction_evidence {
        let edit_event_id = vectors::insert_edit_event(
            &state.pool,
            &rec.user_id,
            Some(&rec.id),
            &transcript,
            &body.ai_output,
            &body.user_kept,
            rec.target_app.as_deref(),
        );
        if let Some(ref id) = edit_event_id {
            info!(
                "[classify] correlated edit_event {} created for recording {}",
                id, rec.id
            );
        } else {
            warn!(
                "[classify] failed to insert correlated edit_event for {}",
                body.recording_id
            );
        }

        learned_emails = email_memory::upsert_many_from_text(
            &state.pool,
            &state.default_user_id,
            &body.user_kept,
            Some(&body.user_kept),
        );
        if !learned_emails.is_empty() {
            info!(
                "[classify] learned {} local email memory item(s) for {}",
                learned_emails.len(),
                body.recording_id,
            );
        }

        let reverted =
            run_alias_revert_pass(&state, &body.ai_output, &body.user_kept, &correction_pairs);
        if !reverted.is_empty() {
            info!(
                "[classify] reverted {} wrong alias(es) after correlated correction",
                reverted.len()
            );
            for reverted_alias in &reverted {
                negative_terms.push(NegativeTerm {
                    term: reverted_alias.replaced_with.clone(),
                    wrong_replacement: reverted_alias.term.clone(),
                    correction_count: 1,
                });
            }
        }

        if all_changes_are_corrections {
            let policy_feedback = tier2_edit_policy::mark_removed_feedback(
                &state.pool,
                &state.default_user_id,
                &body.recording_id,
                &body.ai_output,
                &body.user_kept,
            );
            for (replaced_with, term) in &policy_feedback.penalized_pairs {
                if negative_terms.iter().any(|negative| {
                    negative.term == *term && negative.wrong_replacement == *replaced_with
                }) {
                    continue;
                }
                info!(
                    "[classify] policy-revert: {:?} -> {:?} blocked from future corrections",
                    replaced_with, term,
                );
                negative_terms.push(NegativeTerm {
                    term: term.clone(),
                    wrong_replacement: replaced_with.clone(),
                    correction_count: 1,
                });
            }
            policy_touched = policy_feedback.marked_kept > 0 || policy_feedback.penalized > 0;
            if policy_touched {
                info!(
                    "[classify] tier2 edit-policy feedback: kept_marked={} penalized={} for {}",
                    policy_feedback.marked_kept, policy_feedback.penalized, body.recording_id,
                );
            }
        }
    }

    // ── Step 5: Prepare learnable candidates ────────────────────────────────
    // STT learning is human-in-the-loop: classify proposes candidates, and
    // `/v1/confirm-batch` is the only path that writes approved aliases/terms.
    let stt_change_count = analyzer_output
        .changes
        .iter()
        .filter(|c| matches!(c.reason, ChangeReason::SttError) && c.should_learn)
        .count();

    let promoted_count = 0_usize;
    let promoted_terms: Vec<String> = Vec::new();
    let queued_terms: Vec<QueuedTerm> = Vec::new();
    let mut review_candidates: Vec<ReviewCandidate> = Vec::new();
    let mut safety_blocked_aliases: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    let mut has_repeat = false;
    let mut learned = false;
    let server_memory_terms: Vec<serde_json::Value> = Vec::new();
    let server_memory_aliases: Vec<serde_json::Value> = Vec::new();

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
        }
        .and_then(|pair| {
            refine_stt_pair_for_learning(pair, &state.pool, &state.default_user_id, &body.user_kept)
        });
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

        // A non-learnable analyzer result must never mutate vocabulary. STT
        // corrections to an already protected term still flow through the
        // explicit review path below.
        if !change.should_learn && !matches!(change.reason, ChangeReason::SttError) {
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
                if promotion_gate::is_common_word(corrected)
                    || crate::tier2::is_in_dictionary(corrected)
                {
                    info!("[classify] STT_ERROR skipped — common/dictionary word: {corrected:?}");
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
                    .unwrap_or_else(|| vocabulary::classify_term_type(canonical_for_policy))
                    .to_string();
                let term_type = term_type.as_str();
                if existing_protected.is_none() && matches!(term_type, "phrase" | "other") {
                    // ── Add-on: "Ask to learn" ──────────────────────────────
                    // This term is not an obvious proper noun, so we NEVER
                    // auto-learn it. But it has already cleared every junk gate
                    // above (common-word, dictionary, numeric, in-kept, script),
                    // so instead of dropping it silently we offer the user a
                    // Learn / Skip choice when it looks name-like: a single
                    // unknown word the user kept (e.g. a lowercase name), or a
                    // multi-word span with a real name anchor (e.g. "Emiac tech").
                    // Cheap source-safety only (no LLM): skip the offer if the
                    // heard form is itself a common / unsafe alias source.
                    let single = corrected.split_whitespace().count() <= 1;
                    let looks_offerable = if single {
                        !clean_surface(corrected).is_empty()
                    } else {
                        name_like_span(corrected)
                    };
                    let source_safe = original.trim().is_empty()
                        || (!promotion_gate::is_common_word(original)
                            && unsafe_stt_source_reason(
                                &state.pool,
                                &state.default_user_id,
                                original,
                            )
                            .is_none());
                    let alias_offer_safe = if !original.trim().is_empty()
                        && deterministic_hitl_alias_safe(
                            &state.pool,
                            &state.default_user_id,
                            original,
                            corrected,
                            term_type,
                        ) {
                        info!(
                            "[classify] local_ask accepted by deterministic HITL gate: {:?} -> {:?}",
                            original, corrected
                        );
                        true
                    } else if !original.trim().is_empty() {
                        let safety = alias_safety::judge_alias_source(
                            &state.http_client,
                            &state.pool,
                            &state.default_user_id,
                            &groq_key,
                            original,
                            corrected,
                            Some(&body.user_kept),
                        )
                        .await;
                        if !safety.allows_learning() {
                            let original_norm = tier2_edit_policy::normalize_token(original);
                            let corrected_norm = tier2_edit_policy::normalize_token(corrected);
                            if !original_norm.is_empty() && !corrected_norm.is_empty() {
                                safety_blocked_aliases.insert((original_norm, corrected_norm));
                            }
                            info!(
                                "[classify] local_ask suppressed by safety gate: {:?} -> {:?} verdict={} reason={}",
                                original,
                                corrected,
                                safety.verdict.as_str(),
                                safety.reason
                            );
                            false
                        } else {
                            true
                        }
                    } else {
                        true
                    };

                    if looks_offerable && source_safe && alias_offer_safe {
                        review_candidates.push(ReviewCandidate {
                            original: original.to_string(),
                            corrected: corrected.to_string(),
                            term_type: term_type.to_string(),
                            learnable: true,
                            tag: "local_ask".to_string(),
                            context: change
                                .context_example
                                .clone()
                                .or_else(|| surrounding_sentence(&body.user_kept, corrected)),
                        });
                        info!(
                            "[classify] offering Learn/Skip for name-like {corrected:?} (type={term_type})"
                        );
                    } else {
                        info!(
                            "[classify] STT_ERROR skipped — not a proper noun (type={term_type}): {corrected:?}"
                        );
                    }
                    continue;
                }
                if !original.trim().is_empty()
                    && deterministic_hitl_alias_safe(
                        &state.pool,
                        &state.default_user_id,
                        original,
                        canonical_for_policy,
                        term_type,
                    )
                {
                    info!(
                        "[classify] STT_ERROR alias accepted by deterministic HITL gate: {:?} -> {:?}",
                        original, canonical_for_policy
                    );
                } else if !original.trim().is_empty() {
                    let safety = alias_safety::judge_alias_source(
                        &state.http_client,
                        &state.pool,
                        &state.default_user_id,
                        &groq_key,
                        original,
                        canonical_for_policy,
                        Some(&body.user_kept),
                    )
                    .await;
                    if !safety.allows_learning() {
                        let original_norm = tier2_edit_policy::normalize_token(original);
                        let corrected_norm =
                            tier2_edit_policy::normalize_token(canonical_for_policy);
                        if !original_norm.is_empty() && !corrected_norm.is_empty() {
                            safety_blocked_aliases.insert((original_norm, corrected_norm));
                        }
                        info!(
                            "[classify] STT_ERROR alias suppressed by safety gate: {:?} -> {:?} verdict={} reason={} — not offering review candidate",
                            original,
                            canonical_for_policy,
                            safety.verdict.as_str(),
                            safety.reason
                        );
                        continue;
                    }
                }
                if let Some(reason) =
                    unsafe_stt_source_reason(&state.pool, &state.default_user_id, original)
                {
                    info!("[classify] STT_ERROR skipped — unsafe source {original:?}: {reason}");
                    continue;
                }

                let tag = if existing_term.is_some() {
                    has_repeat = true;
                    "existing_term_alias"
                } else {
                    "stt"
                };
                push_unique_review_candidate(
                    &mut review_candidates,
                    ReviewCandidate {
                        original: original.to_string(),
                        corrected: canonical_for_policy.to_string(),
                        term_type: term_type.to_string(),
                        learnable: true,
                        tag: tag.to_string(),
                        context: change.context_example.clone().or_else(|| {
                            surrounding_sentence(&body.user_kept, canonical_for_policy)
                        }),
                    },
                );
                info!(
                    "[classify] HITL review candidate: {:?} -> {:?} (type={term_type}, tag={tag}, total_stt_changes={}, in_vocab={})",
                    original,
                    canonical_for_policy,
                    stt_change_count,
                    existing_term.is_some(),
                );
                continue;
            }

            ChangeReason::PolishError => {
                let wrong = original.to_ascii_lowercase();
                if !wrong.is_empty()
                    && wrong != corrected.to_ascii_lowercase()
                    && !unsafe_prompt_correction_source(
                        &state.pool,
                        &state.default_user_id,
                        &wrong,
                        corrected,
                    )
                {
                    push_unique_review_candidate(
                        &mut review_candidates,
                        ReviewCandidate {
                            original: wrong,
                            corrected: corrected.to_ascii_lowercase(),
                            term_type: "writing_preference".to_string(),
                            learnable: true,
                            tag: "polish_error".to_string(),
                            context: change
                                .context_example
                                .clone()
                                .or_else(|| surrounding_sentence(&body.user_kept, corrected)),
                        },
                    );
                }
            }

            ChangeReason::FormatPreference => {
                let wrong = original.to_ascii_lowercase();
                if !wrong.is_empty()
                    && wrong != corrected.to_ascii_lowercase()
                    && !unsafe_prompt_correction_source(
                        &state.pool,
                        &state.default_user_id,
                        &wrong,
                        corrected,
                    )
                {
                    push_unique_review_candidate(
                        &mut review_candidates,
                        ReviewCandidate {
                            original: wrong,
                            corrected: corrected.to_string(),
                            term_type: "format_preference".to_string(),
                            learnable: true,
                            tag: "format_preference".to_string(),
                            context: change
                                .context_example
                                .clone()
                                .or_else(|| surrounding_sentence(&body.user_kept, corrected)),
                        },
                    );
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
                        context: change
                            .context_example
                            .clone()
                            .or_else(|| surrounding_sentence(&body.user_kept, corrected)),
                    });
                }
            }

            ChangeReason::StylePreference => {
                // Not learnable — intentional no-op.
            }
        }
    }

    let local_review_candidates = local_review_candidates_from_analyzer(
        &analyzer_output.changes,
        &state.pool,
        &state.default_user_id,
        &body.user_kept,
        &output_language,
    );
    let local_before = review_candidates.len();
    for candidate in local_review_candidates {
        let pair = (
            tier2_edit_policy::normalize_token(&candidate.original),
            tier2_edit_policy::normalize_token(&candidate.corrected),
        );
        let safety_blocked =
            safety_blocked_aliases
                .iter()
                .any(|(blocked_original, blocked_corrected)| {
                    pair.1 == *blocked_corrected
                        && !pair.0.is_empty()
                        && !blocked_original.is_empty()
                        && safety_block_should_suppress_candidate(&pair.0, blocked_original)
                });
        if safety_blocked {
            info!(
                "[classify] local review candidate suppressed by prior safety block: {:?} -> {:?}",
                candidate.original, candidate.corrected
            );
            continue;
        }
        push_unique_review_candidate(&mut review_candidates, candidate);
    }
    if review_candidates.len() != local_before {
        info!(
            "[classify] local review fallback added {} candidate(s)",
            review_candidates.len().saturating_sub(local_before)
        );
    }

    // ── Add-on: de-dup + cap the new "Ask to learn" cards ───────────────────
    // A noisy recording must never fan out into a wall of choices. De-dup all
    // review candidates by (original, corrected) and cap the new "local_ask"
    // cards at 8 per recording. Existing tags keep their order and behavior; the
    // cap only ever removes surplus local_ask offers, never an auto-learn or an
    // existing review card.
    {
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut ask_count = 0usize;
        review_candidates.retain(|candidate| {
            if !seen.insert((candidate.original.clone(), candidate.corrected.clone())) {
                return false;
            }
            if candidate.tag == "local_ask" {
                ask_count += 1;
                if ask_count > 8 {
                    return false;
                }
            }
            true
        });
    }

    let has_negatives = !negative_terms.is_empty();
    if !review_candidates.is_empty() {
        let local_count = review_candidates.len();
        review_candidates =
            sanitize_review_candidates(review_candidates, &body.user_kept, &output_language);
        info!(
            "[classify] local review candidates: {local_count} -> {}",
            review_candidates.len()
        );
    }
    let has_review = !review_candidates.is_empty();

    // Invalidate after any corrections, stt_replacements, or Tier 2 policy writes.
    if learned || policy_touched || has_negatives || !learned_emails.is_empty() {
        crate::invalidate_lexicon_cache(&state.lexicon_cache).await;
    }

    // Only retrain if something was actually committed (not pending review).
    // When review candidates exist, retrain happens after confirm-batch.
    if (learned || has_negatives) && !has_review {
        schedule_onnx_retrain(state.clone());
    }

    if !learned_emails.is_empty() {
        learned = true;
    }
    let notify = (learned && (promoted_count > 0 || policy_touched)) || !learned_emails.is_empty();

    let change_count = analyzer_output.changes.len();
    info!(
        "[classify] {} overall={} changes={} promoted={} notify={} learned={} negatives={} review={}",
        body.recording_id,
        analyzer_output.overall_class,
        change_count,
        promoted_count,
        notify,
        learned,
        negative_terms.len(),
        review_candidates.len(),
    );

    if learned || notify || has_negatives || has_review || !queued_terms.is_empty() {
        post_runtime_client_event(
            state.clone(),
            "classify_edit_result",
            body.recording_id.clone(),
            analyzer_output.overall_class.clone(),
            hash_text(&body.ai_output),
            hash_text(&body.user_kept),
            serde_json::json!({
                "learned": learned,
                "notify": notify,
                "promoted_count": promoted_count,
                "promoted_term_count": promoted_terms.len(),
                "learned_email_count": learned_emails.len(),
                "queued_term_count": queued_terms.len(),
                "negative_count": negative_terms.len(),
                "review_candidate_count": review_candidates.len(),
                "change_count": change_count,
                "capture_method": body.capture_method,
                "memory": {
                    "accepted_terms": server_memory_terms,
                    "accepted_aliases": server_memory_aliases,
                },
            }),
        );
    }
    if promoted_count > 0 || policy_touched {
        post_runtime_memory_dirty(state.clone());
    }

    let review_json: Vec<serde_json::Value> = review_candidates
        .iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .collect();
    let changes_json: Vec<serde_json::Value> = analyzer_output
        .changes
        .iter()
        .filter_map(|change| serde_json::to_value(change).ok())
        .collect();
    let review_session_id = if review_candidates.is_empty() {
        None
    } else {
        let id = edit_review_sessions::insert(
            &state.pool,
            &state.default_user_id,
            &body.recording_id,
            &body.ai_output,
            &body.user_kept,
            &review_json,
            &changes_json,
        );
        if id.is_none() {
            warn!(
                "[classify] failed to persist review session for {}",
                body.recording_id
            );
        }
        id
    };
    crate::observability::schedule_classify_observability(
        &state,
        crate::observability::ClassifyObservabilityInput {
            recording_id: &body.recording_id,
            ai_output: &body.ai_output,
            user_kept: &body.user_kept,
            capture_method: &body.capture_method,
            overall_class: &analyzer_output.overall_class,
            changes: &analyzer_output.changes,
            review_candidates: &review_json,
            promoted_terms: &promoted_terms,
            edit_trace_json: body.edit_trace_json.as_ref(),
        },
    );

    (
        StatusCode::OK,
        Json(ClassifyResponse {
            class: analyzer_output.overall_class,
            reason: format!("analyzer identified {} change(s)", change_count),
            pending_id: None,
            review_session_id,
            learned,
            notify,
            promoted_count,
            is_repeat: has_repeat,
            promoted_terms,
            learned_emails,
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
fn run_alias_revert_pass(
    state: &AppState,
    polish: &str,
    user_kept: &str,
    allowed_pairs: &std::collections::HashSet<(String, String)>,
) -> Vec<RevertedAlias> {
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
        let pair = (
            tier2_edit_policy::normalize_token(&v.term),
            tier2_edit_policy::normalize_token(&replaced_with),
        );
        if !allowed_pairs.contains(&pair) {
            info!(
                "[revert] skipped uncorrelated alias removal {:?} -> {:?}",
                replaced_with, v.term,
            );
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
    if std::env::var("AIRNOTE_DISABLE_ONNX_RETRAIN")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        info!("[retrain] skipped — AIRNOTE_DISABLE_ONNX_RETRAIN is set");
        return;
    }

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
            // Windows has no `python3` on PATH; try the `py` launcher and
            // `python` first there. Skip a candidate only when it's truly
            // absent (NotFound) so a real script error still surfaces.
            let python_candidates: &[&str] = if cfg!(windows) {
                &["py", "python", "python3"]
            } else {
                &["python3", "python"]
            };
            let mut output = Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no python interpreter found in PATH",
            ));
            for cmd in python_candidates {
                let attempt = std::process::Command::new(cmd)
                    .arg(&script_path)
                    .arg("--db")
                    .arg(db.to_str().unwrap_or_default())
                    .arg("--user-id")
                    .arg(&user)
                    .arg("--micro")
                    .output();
                if matches!(&attempt, Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
                    continue;
                }
                output = attempt;
                break;
            }
            match output {
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

// ── Complex edit interpreter ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LlmEditInterpreterResponse {
    #[serde(default)]
    edits: Vec<LlmEditCandidate>,
}

#[derive(Debug, Deserialize)]
struct LlmEditCandidate {
    #[serde(default)]
    source_span: String,
    #[serde(default)]
    corrected_span: String,
    #[serde(default)]
    edit_type: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    should_learn: bool,
    #[serde(default)]
    confidence: f64,
}

fn needs_complex_edit_interpreter(
    hunks: &[edit_diff::Hunk],
    deterministic_changes: &[AnalyzedChange],
) -> bool {
    if hunks.is_empty() {
        return false;
    }

    let has_learnable_stt = deterministic_changes
        .iter()
        .any(|change| change.should_learn && matches!(change.reason, ChangeReason::SttError));

    let has_complex_hunk = hunks.iter().any(|hunk| {
        let kept_tokens = token_surfaces(&hunk.kept_window);
        let polish_tokens = token_surfaces(&hunk.polish_window);

        if kept_tokens.is_empty() {
            return false;
        }
        if polish_tokens.is_empty() {
            return true;
        }
        kept_tokens.len() > 4 || polish_tokens.len() > 4 || kept_tokens.len() != polish_tokens.len()
    });

    has_complex_hunk
        || !has_learnable_stt
            && deterministic_changes.iter().any(|change| {
                matches!(
                    change.reason,
                    ChangeReason::StylePreference | ChangeReason::StructuralRewrite
                ) && !change.should_learn
            })
}

async fn interpret_complex_edit_with_llm(
    http: &reqwest::Client,
    deepseek_key: &str,
    transcript: &str,
    polished: &str,
    user_kept: &str,
    hunks: &[edit_diff::Hunk],
    pool: &crate::store::DbPool,
    user_id: &str,
) -> Vec<AnalyzedChange> {
    if deepseek_key.trim().is_empty() {
        info!("[classify-llm] skipped complex edit interpreter — no DEEPSEEK_API_KEY");
        return Vec::new();
    }

    let prompt = build_complex_edit_prompt(transcript, polished, user_kept, hunks);
    let system_prompt = "You are a conservative edit interpreter for a speech dictation learning pipeline. \
You only identify concrete spans that the user explicitly changed. \
Return strict JSON only. Do not learn ordinary Hinglish/English/Hindi words. \
For aliases, prefer exact corrected domain terms, brands, acronyms, product names, and code identifiers.";
    let (payload, latency_ms, model) =
        match crate::llm::deepseek::chat_json::<LlmEditInterpreterResponse>(
            http,
            deepseek_key,
            system_prompt,
            &prompt,
            700,
            std::time::Duration::from_secs(8),
            "complex-edit-interpreter",
        )
        .await
        {
            Ok(result) => result,
            Err(err) => {
                crate::llm::deepseek::log_fail_closed("complex-edit-interpreter", &err);
                return Vec::new();
            }
        };

    let verified = payload
        .edits
        .iter()
        .filter_map(|candidate| {
            verified_llm_candidate_to_change(
                candidate, transcript, polished, user_kept, hunks, pool, user_id,
            )
        })
        .collect::<Vec<_>>();
    info!(
        "[classify-llm] deepseek complex edit model={} latency_ms={} raw_candidates={} verified_candidates={}",
        model,
        latency_ms,
        payload.edits.len(),
        verified.len(),
    );
    verified
}

fn build_complex_edit_prompt(
    transcript: &str,
    polished: &str,
    user_kept: &str,
    hunks: &[edit_diff::Hunk],
) -> String {
    let hunks_json = serde_json::to_string(hunks).unwrap_or_else(|_| "[]".to_string());
    format!(
        "Task: identify only concrete STT correction spans from this user edit.\n\
         Do not rewrite. Do not explain. Do not invent terms.\n\n\
         RAW_TRANSCRIPT:\n{transcript}\n\n\
         AIRNOTE_OUTPUT:\n{polished}\n\n\
         USER_FINAL:\n{user_kept}\n\n\
         DETERMINISTIC_DIFF_HUNKS_JSON:\n{hunks_json}\n\n\
         Return JSON exactly in this shape:\n\
         {{\"edits\":[{{\"source_span\":\"text from RAW_TRANSCRIPT or AIRNOTE_OUTPUT, empty only for missing-word insertions\",\
         \"corrected_span\":\"text from USER_FINAL\",\
         \"edit_type\":\"replace|insert\",\
         \"reason\":\"stt_error|style_preference|structural_rewrite\",\
         \"should_learn\":true,\
         \"confidence\":0.0}}]}}\n\n\
         Rules:\n\
         - corrected_span must appear literally in USER_FINAL.\n\
         - For replace, source_span must appear literally in RAW_TRANSCRIPT or AIRNOTE_OUTPUT.\n\
         - For insert, source_span must be empty and corrected_span must be a protected name, brand, acronym, or code identifier that was likely skipped by STT.\n\
         - If one inserted phrase contains multiple protected terms, return one edit per term, e.g. inserted \"n8n EMIAC\" becomes corrected_span \"n8n\" and corrected_span \"EMIAC\".\n\
         - Do not mark grammar, tone, wording, or full sentence rewrites as learnable.\n\
         - Do not learn common Hindi/Hinglish/English words like kaisa, laga, main, mein, hai, time, can, go.\n\
         - If uncertain, return {{\"edits\":[]}}."
    )
}

fn verified_llm_candidate_to_change(
    candidate: &LlmEditCandidate,
    transcript: &str,
    polished: &str,
    user_kept: &str,
    hunks: &[edit_diff::Hunk],
    pool: &crate::store::DbPool,
    user_id: &str,
) -> Option<AnalyzedChange> {
    if !candidate.should_learn {
        return None;
    }
    let corrected = clean_surface(&candidate.corrected_span);
    if corrected.is_empty() || token_surfaces(&corrected).len() > 4 {
        return None;
    }
    if promotion_gate::is_common_word(&corrected) {
        return None;
    }
    if !contains_normalized_phrase(user_kept, &corrected) {
        return None;
    }

    let edit_type = candidate.edit_type.trim().to_ascii_lowercase();
    let parsed_reason =
        parse_llm_change_reason(&candidate.reason).unwrap_or(ChangeReason::StructuralRewrite);
    let reason = if edit_type == "insert"
        && matches!(
            parsed_reason,
            ChangeReason::SttError | ChangeReason::StructuralRewrite
        ) {
        ChangeReason::SttError
    } else {
        parsed_reason
    };
    if !matches!(reason, ChangeReason::SttError) {
        return None;
    }
    let source = clean_surface(&candidate.source_span);
    let min_confidence = if edit_type == "insert" { 0.86 } else { 0.78 };
    if candidate.confidence < min_confidence {
        return None;
    }

    let original = if edit_type == "insert" {
        if !source.is_empty() {
            return None;
        }
        if !is_strong_insert_target(pool, user_id, &corrected) {
            return None;
        }
        String::new()
    } else if edit_type == "replace" {
        if source.is_empty()
            || !candidate_source_supported(&source, transcript, polished, hunks)
            || tier2_edit_policy::normalize_token(&source)
                == tier2_edit_policy::normalize_token(&corrected)
        {
            return None;
        }
        source
    } else {
        return None;
    };

    info!(
        "[classify-llm] verified complex edit: {:?} -> {:?} ({edit_type}, conf={:.2})",
        original, corrected, candidate.confidence,
    );
    Some(AnalyzedChange {
        original,
        corrected: canonicalize_corrected_surface(corrected),
        reason,
        meaning: None,
        context_example: surrounding_sentence(user_kept, &candidate.corrected_span)
            .or_else(|| surrounding_sentence(user_kept, &clean_surface(&candidate.corrected_span))),
        should_learn: true,
        confidence: candidate.confidence.clamp(0.0, 1.0),
        skip_reason: None,
        format_rule: None,
    })
}

fn parse_llm_change_reason(raw: &str) -> Option<ChangeReason> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "stt_error" | "stt" => Some(ChangeReason::SttError),
        "polish_error" | "polish" => Some(ChangeReason::PolishError),
        "format_preference" | "format" => Some(ChangeReason::FormatPreference),
        "style_preference" | "style" => Some(ChangeReason::StylePreference),
        "structural_rewrite" | "structural" | "rewrite" => Some(ChangeReason::StructuralRewrite),
        _ => None,
    }
}

fn is_strong_insert_target(pool: &crate::store::DbPool, user_id: &str, corrected: &str) -> bool {
    if protected_vocab_lookup(pool, user_id, corrected).is_some()
        || canonical_developer_term(corrected).is_some()
    {
        return true;
    }
    if promotion_gate::is_common_word(corrected) || crate::tier2::is_in_dictionary(corrected) {
        return false;
    }
    matches!(
        vocabulary::classify_term_type(corrected),
        "brand" | "acronym" | "proper_noun" | "code_identifier"
    )
}

fn protected_term_type(term_type: &str) -> bool {
    matches!(
        term_type,
        "brand" | "acronym" | "proper_noun" | "code_identifier"
    )
}

fn deterministic_hitl_alias_safe(
    pool: &crate::store::DbPool,
    user_id: &str,
    original: &str,
    corrected: &str,
    term_type: &str,
) -> bool {
    let original = original.trim();
    let corrected = corrected.trim();
    if original.is_empty()
        || corrected.is_empty()
        || tier2_edit_policy::normalize_token(original)
            == tier2_edit_policy::normalize_token(corrected)
    {
        return false;
    }
    if unsafe_stt_source_reason(pool, user_id, original).is_some() {
        return false;
    }
    protected_vocab_lookup(pool, user_id, corrected).is_some()
        || canonical_developer_term(corrected).is_some()
        || protected_term_type(term_type)
}

fn local_review_candidates_from_analyzer(
    changes: &[AnalyzedChange],
    pool: &crate::store::DbPool,
    user_id: &str,
    user_kept: &str,
    output_language: &str,
) -> Vec<ReviewCandidate> {
    let mut candidates = Vec::new();
    for change in changes {
        if let Some(candidate) =
            local_review_candidate_from_change(change, pool, user_id, user_kept, output_language)
        {
            push_unique_review_candidate(&mut candidates, candidate);
        }
    }
    candidates
}

fn local_review_candidate_from_change(
    change: &AnalyzedChange,
    pool: &crate::store::DbPool,
    user_id: &str,
    user_kept: &str,
    output_language: &str,
) -> Option<ReviewCandidate> {
    if !matches!(change.reason, ChangeReason::SttError) || !change.should_learn {
        return None;
    }

    let corrected_hint = clean_surface(&change.corrected);
    if corrected_hint.is_empty() || token_surfaces(&corrected_hint).len() > 4 {
        return None;
    }
    let corrected = canonicalize_corrected_surface(corrected_hint);
    if corrected.is_empty()
        || promotion_gate::is_common_word(&corrected)
        || crate::tier2::is_in_dictionary(&corrected)
        || promotion_gate::is_numeric_junk(&corrected)
        || !promotion_gate::appears_in_user_kept(&corrected, user_kept)
        || !promotion_gate::script_matches(&corrected, output_language)
    {
        return None;
    }

    let existing_protected = protected_vocab_lookup(pool, user_id, &corrected)
        .or_else(|| canonical_developer_term(&corrected).map(str::to_string));
    let existing_term = existing_protected
        .as_ref()
        .and_then(|canonical| vocabulary::find_by_term_ci(pool, user_id, canonical));
    let canonical_for_policy = existing_term
        .as_ref()
        .map(|existing| existing.term.as_str())
        .or(existing_protected.as_deref())
        .unwrap_or(&corrected);
    let term_type = existing_term
        .as_ref()
        .and_then(|existing| existing.term_type.as_deref())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| vocabulary::classify_term_type(canonical_for_policy))
        .to_string();

    let strong_target = existing_protected.is_some()
        || matches!(
            term_type.as_str(),
            "brand" | "acronym" | "proper_noun" | "code_identifier"
        );
    if !strong_target {
        return None;
    }

    let original = clean_surface(&change.original);
    let source_tokens = token_surfaces(&original);
    let corrected_tokens = token_surfaces(&corrected);
    let is_token_collapse = source_tokens.len() > 1 && corrected_tokens.len() == 1;
    let is_single_common_source = source_tokens.len() == 1
        && (promotion_gate::is_common_word(&original)
            || alias_safety::is_common_alias_source(&original)
            || crate::tier2::is_in_dictionary(&alias_safety::normalize_source(&original)));
    if is_single_common_source {
        return None;
    }

    Some(ReviewCandidate {
        original,
        corrected: canonical_for_policy.to_string(),
        term_type,
        learnable: true,
        tag: if is_token_collapse {
            "local_token_collapse".to_string()
        } else {
            "local_deterministic".to_string()
        },
        context: change
            .context_example
            .clone()
            .or_else(|| surrounding_sentence(user_kept, canonical_for_policy)),
    })
}

fn merge_review_candidates(
    candidates: &mut Vec<ReviewCandidate>,
    additions: Vec<ReviewCandidate>,
) -> usize {
    let before = candidates.len();
    for candidate in additions {
        push_unique_review_candidate(candidates, candidate);
    }
    candidates.len().saturating_sub(before)
}

fn push_unique_review_candidate(candidates: &mut Vec<ReviewCandidate>, candidate: ReviewCandidate) {
    let original_norm = tier2_edit_policy::normalize_token(&candidate.original);
    let corrected_norm = tier2_edit_policy::normalize_token(&candidate.corrected);
    let original_surface_norm = review_candidate_span_norm(&candidate.original);
    let corrected_surface_norm = review_candidate_span_norm(&candidate.corrected);
    if corrected_norm.is_empty()
        || (!original_surface_norm.is_empty() && original_surface_norm == corrected_surface_norm)
        || candidates.iter().any(|existing| {
            tier2_edit_policy::normalize_token(&existing.original) == original_norm
                && tier2_edit_policy::normalize_token(&existing.corrected) == corrected_norm
        })
    {
        return;
    }
    candidates.push(candidate);
}

enum ReviewCandidateContextTrim {
    Unchanged,
    Drop,
    Trim(ReviewCandidate),
}

fn trim_unchanged_review_candidate_context(
    candidate: &ReviewCandidate,
) -> ReviewCandidateContextTrim {
    let original_tokens = token_surfaces(&candidate.original);
    let corrected_tokens = token_surfaces(&candidate.corrected);
    if original_tokens.len() <= 1 || original_tokens.len() != corrected_tokens.len() {
        return ReviewCandidateContextTrim::Unchanged;
    }

    let original_norms = original_tokens
        .iter()
        .map(|token| review_candidate_span_norm(token))
        .collect::<Vec<_>>();
    let corrected_norms = corrected_tokens
        .iter()
        .map(|token| review_candidate_span_norm(token))
        .collect::<Vec<_>>();

    let mut start = 0usize;
    while start < original_norms.len() && original_norms[start] == corrected_norms[start] {
        start += 1;
    }

    let mut end = original_norms.len();
    while end > start && original_norms[end - 1] == corrected_norms[end - 1] {
        end -= 1;
    }

    if start == 0 && end == original_norms.len() {
        return ReviewCandidateContextTrim::Unchanged;
    }
    if start >= end {
        return ReviewCandidateContextTrim::Drop;
    }

    let original = original_tokens[start..end].join(" ");
    let corrected = corrected_tokens[start..end].join(" ");
    let corrected_norm = review_candidate_span_norm(&corrected);
    if corrected_norm.is_empty()
        || promotion_gate::is_common_word(&corrected)
        || alias_safety::is_common_alias_source(&corrected)
    {
        return ReviewCandidateContextTrim::Drop;
    }
    if !matches!(
        vocabulary::classify_term_type(&corrected),
        "brand" | "acronym" | "proper_noun" | "code_identifier"
    ) {
        return ReviewCandidateContextTrim::Drop;
    }

    let mut trimmed = candidate.clone();
    trimmed.original = original;
    trimmed.corrected = canonicalize_corrected_surface(corrected);
    trimmed.term_type = vocabulary::classify_term_type(&trimmed.corrected).to_string();
    if !trimmed.tag.contains("trimmed") {
        trimmed.tag = format!("{}_trimmed", trimmed.tag);
    }
    ReviewCandidateContextTrim::Trim(trimmed)
}

fn review_candidate_span_norm(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn sanitize_review_candidates(
    candidates: Vec<ReviewCandidate>,
    user_kept: &str,
    output_language: &str,
) -> Vec<ReviewCandidate> {
    let mut sanitized = Vec::new();
    for candidate in candidates {
        let candidate = match trim_unchanged_review_candidate_context(&candidate) {
            ReviewCandidateContextTrim::Unchanged => candidate,
            ReviewCandidateContextTrim::Drop => continue,
            ReviewCandidateContextTrim::Trim(trimmed) => trimmed,
        };
        let corrected = clean_surface(&candidate.corrected);
        if corrected.is_empty()
            || !promotion_gate::appears_in_user_kept(&corrected, user_kept)
            || !promotion_gate::script_matches(&corrected, output_language)
        {
            continue;
        }
        push_unique_review_candidate(&mut sanitized, candidate);
    }
    remove_covered_weak_partial_candidates(sanitized)
}

fn remove_covered_weak_partial_candidates(
    candidates: Vec<ReviewCandidate>,
) -> Vec<ReviewCandidate> {
    candidates
        .iter()
        .enumerate()
        .filter_map(|(idx, candidate)| {
            if covered_by_stronger_same_target(idx, candidate, &candidates) {
                info!(
                    "[classify] review candidate suppressed — weak partial source {:?} covered by full-span alias for {:?}",
                    candidate.original, candidate.corrected
                );
                None
            } else {
                Some(candidate.clone())
            }
        })
        .collect()
}

fn covered_by_stronger_same_target(
    idx: usize,
    candidate: &ReviewCandidate,
    candidates: &[ReviewCandidate],
) -> bool {
    let corrected_norm = review_candidate_span_norm(&candidate.corrected);
    let original_norm = review_candidate_span_norm(&candidate.original);
    let original_tokens: Vec<&str> = original_norm.split_whitespace().collect();
    if corrected_norm.is_empty()
        || original_tokens.len() != 1
        || !weak_single_token_alias_source(&candidate.original)
    {
        return false;
    }

    candidates.iter().enumerate().any(|(other_idx, other)| {
        if other_idx == idx {
            return false;
        }
        if review_candidate_span_norm(&other.corrected) != corrected_norm {
            return false;
        }
        let other_norm = review_candidate_span_norm(&other.original);
        let other_tokens: Vec<&str> = other_norm.split_whitespace().collect();
        other_tokens.len() > 1
            && (contains_token_sequence(&other_tokens, &original_tokens)
                || other_tokens.join("").contains(&original_tokens.join("")))
    })
}

fn weak_single_token_alias_source(source: &str) -> bool {
    let source_norm = alias_safety::normalize_source(source);
    let tokens: Vec<&str> = source_norm.split_whitespace().collect();
    if tokens.len() != 1 {
        return false;
    }
    let token = tokens[0];
    alias_safety::is_common_alias_source(source)
        || promotion_gate::is_common_word(source)
        || crate::tier2::is_in_dictionary(token)
        || lowercase_plain_fragment(source, token)
}

fn safety_block_should_suppress_candidate(
    candidate_original_norm: &str,
    blocked_original_norm: &str,
) -> bool {
    let candidate = candidate_original_norm.trim();
    let blocked = blocked_original_norm.trim();
    if candidate.is_empty() || blocked.is_empty() {
        return false;
    }
    if candidate == blocked {
        return true;
    }

    let candidate_tokens: Vec<&str> = candidate.split_whitespace().collect();
    let blocked_tokens: Vec<&str> = blocked.split_whitespace().collect();
    if candidate_tokens.is_empty() || blocked_tokens.is_empty() {
        return false;
    }

    if candidate_tokens.len() <= blocked_tokens.len() {
        return contains_token_sequence(&blocked_tokens, &candidate_tokens)
            || blocked_tokens.join("").contains(&candidate_tokens.join(""));
    }

    blocked_tokens.len() > 1
        && (contains_token_sequence(&candidate_tokens, &blocked_tokens)
            || candidate_tokens.join("").contains(&blocked_tokens.join("")))
}

fn lowercase_plain_fragment(raw: &str, norm_token: &str) -> bool {
    if norm_token.len() < 3 || norm_token.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let raw_trimmed = raw.trim();
    !raw_trimmed.chars().any(|c| c.is_ascii_uppercase())
        && !raw_trimmed.contains('_')
        && !raw_trimmed.contains('-')
}

fn contains_token_sequence(haystack: &[&str], needle: &[&str]) -> bool {
    !needle.is_empty()
        && needle.len() <= haystack.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn protected_insert_changes_from_hunks(
    hunks: &[edit_diff::Hunk],
    user_kept: &str,
    pool: &crate::store::DbPool,
    user_id: &str,
) -> Vec<AnalyzedChange> {
    let mut changes = Vec::new();
    for hunk in hunks {
        if hunk.kept_window.trim().is_empty() {
            continue;
        }
        let protected_terms = protected_terms_in_span(pool, user_id, &hunk.kept_window);
        if protected_terms.is_empty() {
            continue;
        }
        let source_surface = clean_surface(&hunk.polish_window);
        let source_is_missing = source_surface.is_empty();
        let source_is_unsafe = !source_is_missing
            && unsafe_stt_source_reason(pool, user_id, &source_surface).is_some();
        if !source_is_missing && !source_is_unsafe {
            continue;
        }

        for corrected in protected_terms {
            if changes.iter().any(|change: &AnalyzedChange| {
                tier2_edit_policy::normalize_token(&change.corrected)
                    == tier2_edit_policy::normalize_token(&corrected)
            }) {
                continue;
            }
            changes.push(AnalyzedChange {
                original: String::new(),
                corrected: corrected.clone(),
                reason: ChangeReason::SttError,
                meaning: None,
                context_example: surrounding_sentence(user_kept, &corrected),
                should_learn: true,
                confidence: 0.92,
                skip_reason: None,
                format_rule: None,
            });
        }
    }
    changes
}

fn candidate_source_supported(
    source: &str,
    transcript: &str,
    polished: &str,
    hunks: &[edit_diff::Hunk],
) -> bool {
    contains_normalized_phrase(transcript, source)
        || contains_normalized_phrase(polished, source)
        || hunks.iter().any(|hunk| {
            contains_normalized_phrase(&hunk.transcript_window, source)
                || contains_normalized_phrase(&hunk.polish_window, source)
        })
}

fn contains_normalized_phrase(text: &str, needle: &str) -> bool {
    let needle_tokens = normalized_phrase_tokens(needle);
    if needle_tokens.is_empty() {
        return false;
    }
    let text_tokens = normalized_phrase_tokens(text);
    text_tokens
        .windows(needle_tokens.len())
        .any(|window| window == needle_tokens.as_slice())
}

fn normalized_phrase_tokens(text: &str) -> Vec<String> {
    token_surfaces(text)
        .into_iter()
        .map(|token| tier2_edit_policy::normalize_token(&token))
        .filter(|token| !token.is_empty())
        .collect()
}

/// Drop empty-original changes whose `corrected` is already covered by a change
/// that DOES carry a real original. A substitution (`max → EMIAC`) is strictly
/// more informative than the bare insert (`"" → EMIAC`) the protected-term
/// extractor emits for the same term, and emitting both makes the review card
/// show a phantom `was "—"` swap. Pure inserts (no same-corrected substitution)
/// are left untouched.
fn dedup_empty_original_by_corrected(changes: &mut Vec<AnalyzedChange>) {
    use std::collections::HashSet;
    // `corrected` surfaces that already have a change carrying a real original.
    let with_original: HashSet<String> = changes
        .iter()
        .filter(|c| !c.original.trim().is_empty())
        .map(|c| tier2_edit_policy::normalize_token(&c.corrected))
        .filter(|s| !s.is_empty())
        .collect();
    changes.retain(|c| {
        if !c.original.trim().is_empty() {
            return true;
        }
        let corrected = tier2_edit_policy::normalize_token(&c.corrected);
        !(!corrected.is_empty() && with_original.contains(&corrected))
    });
}

fn merge_llm_changes(changes: &mut Vec<AnalyzedChange>, llm_changes: Vec<AnalyzedChange>) {
    for llm_change in llm_changes {
        let duplicate_idx = changes.iter().position(|existing| {
            tier2_edit_policy::normalize_token(&existing.original)
                == tier2_edit_policy::normalize_token(&llm_change.original)
                && tier2_edit_policy::normalize_token(&existing.corrected)
                    == tier2_edit_policy::normalize_token(&llm_change.corrected)
        });
        if let Some(idx) = duplicate_idx {
            if !changes[idx].should_learn && llm_change.should_learn {
                changes[idx] = llm_change;
            }
        } else {
            changes.push(llm_change);
        }
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

        if let Some(split_changes) =
            split_known_term_hunk(&polish_tokens, &kept_tokens, user_kept, pool, user_id)
        {
            changes.extend(split_changes);
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
            // Check if original is a real word (236K dictionary + Hindi)
            let original_is_common = crate::tier2::is_in_dictionary(&original_surface)
                || promotion_gate::is_common_word(&original_surface);
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

fn split_known_term_hunk(
    polish_tokens: &[String],
    kept_tokens: &[String],
    user_kept: &str,
    pool: &crate::store::DbPool,
    user_id: &str,
) -> Option<Vec<AnalyzedChange>> {
    if kept_tokens.is_empty()
        || polish_tokens.len() <= kept_tokens.len()
        || kept_tokens.len() > 3
        || polish_tokens.len() > 8
    {
        return None;
    }
    let kept_canonicals = kept_tokens
        .iter()
        .map(|token| canonicalize_corrected_surface(token.clone()))
        .collect::<Vec<_>>();
    if !kept_canonicals
        .iter()
        .all(|term| is_strong_insert_target(pool, user_id, term))
    {
        return None;
    }

    let groups = split_source_tokens_evenly(polish_tokens, kept_canonicals.len())?;
    let mut changes = Vec::new();
    for (source_tokens, corrected) in groups.into_iter().zip(kept_canonicals.into_iter()) {
        let original = source_tokens.join(" ");
        if original.trim().is_empty()
            || tier2_edit_policy::normalize_token(&original)
                == tier2_edit_policy::normalize_token(&corrected)
        {
            return None;
        }
        changes.push(AnalyzedChange {
            original,
            corrected: corrected.clone(),
            reason: ChangeReason::SttError,
            meaning: None,
            context_example: surrounding_sentence(user_kept, &corrected),
            should_learn: true,
            confidence: 0.9,
            skip_reason: None,
            format_rule: None,
        });
    }
    Some(changes)
}

fn split_source_tokens_evenly(tokens: &[String], group_count: usize) -> Option<Vec<Vec<String>>> {
    if group_count == 0 || tokens.len() < group_count {
        return None;
    }
    let mut groups = Vec::with_capacity(group_count);
    let mut start = 0usize;
    for idx in 0..group_count {
        let remaining_tokens = tokens.len() - start;
        let remaining_groups = group_count - idx;
        let group_len = remaining_tokens.div_ceil(remaining_groups);
        let end = (start + group_len).min(tokens.len());
        if start >= end {
            return None;
        }
        groups.push(tokens[start..end].to_vec());
        start = end;
    }
    (start == tokens.len()).then_some(groups)
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
    let original_is_common =
        crate::tier2::is_in_dictionary(&original) || promotion_gate::is_common_word(&original);
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

// ── Deterministic edit pair extraction (used by Step 5) ─────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeterministicEditPair {
    variant_form: String,
    correct_form: String,
    edit_type: String,
    left_context: Vec<String>,
    right_context: Vec<String>,
}

fn refine_stt_pair_for_learning(
    mut pair: DeterministicEditPair,
    pool: &crate::store::DbPool,
    user_id: &str,
    user_kept: &str,
) -> Option<DeterministicEditPair> {
    let original_correct_form = pair.correct_form.clone();
    let exact_protected = protected_vocab_lookup(pool, user_id, &pair.correct_form)
        .or_else(|| canonical_developer_term(&pair.correct_form).map(str::to_string));
    if let Some(canonical) = exact_protected {
        pair.correct_form = canonical;
        let (left_context, right_context) =
            context_around_kept_term(user_kept, &pair.correct_form, 3);
        pair.left_context = left_context;
        pair.right_context = right_context;
        trim_common_source_edges(&mut pair);
        return Some(pair);
    }

    let protected_terms = protected_terms_in_span(pool, user_id, &pair.correct_form);
    if protected_terms.len() == 1 {
        pair.correct_form = protected_terms[0].clone();
        let (left_context, right_context) =
            context_around_kept_term(user_kept, &pair.correct_form, 3);
        pair.left_context = left_context;
        pair.right_context = right_context;
        trim_common_source_edges(&mut pair);

        // If the user edit span was "Macobs mein" but the source side is a
        // common/filler word, this is evidence that a protected term was added,
        // not evidence that the filler word should become an alias.
        if pair.edit_type == "replace"
            && (unsafe_stt_source_reason(pool, user_id, &pair.variant_form).is_some()
                || corrected_span_had_common_filler(&original_correct_form, &pair.correct_form))
        {
            pair.variant_form.clear();
            pair.edit_type = "insert".to_string();
        }
        return Some(pair);
    }

    if protected_terms.len() > 1 {
        // Multi-term spans must be split earlier by the hunk classifier or the
        // protected insert extractor. Keeping a broad phrase here would create
        // review cards like "Macobs mein" and noisy aliases.
        return None;
    }

    Some(pair)
}

fn trim_common_source_edges(pair: &mut DeterministicEditPair) {
    if pair.edit_type != "replace" {
        return;
    }
    let tokens = token_surfaces(&pair.variant_form);
    if tokens.len() <= 1 {
        return;
    }

    let mut start = 0usize;
    let mut end = tokens.len();
    while start < end && is_common_source_edge_token(&tokens[start]) {
        start += 1;
    }
    while end > start && is_common_source_edge_token(&tokens[end - 1]) {
        end -= 1;
    }

    if start == 0 && end == tokens.len() {
        return;
    }
    if start >= end {
        pair.variant_form.clear();
        pair.edit_type = "insert".to_string();
        return;
    }

    pair.variant_form = tokens[start..end].join(" ");
}

fn is_common_source_edge_token(token: &str) -> bool {
    promotion_gate::is_common_word(token) || alias_safety::is_common_alias_source(token)
}

fn corrected_span_had_common_filler(original_span: &str, narrowed_term: &str) -> bool {
    let narrowed_norm = tier2_edit_policy::normalize_token(narrowed_term);
    token_surfaces(original_span).into_iter().any(|token| {
        let norm = tier2_edit_policy::normalize_token(&token);
        !norm.is_empty()
            && norm != narrowed_norm
            && (promotion_gate::is_common_word(&token)
                || alias_safety::is_common_alias_source(&token)
                || crate::tier2::is_in_dictionary(&token))
    })
}

fn protected_terms_in_span(pool: &crate::store::DbPool, user_id: &str, span: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for term in vocabulary::top_terms(pool, user_id, 1000) {
        if !is_protected_vocab_item(&term) || !contains_normalized_phrase(span, &term.term) {
            continue;
        }
        push_unique_term(&mut terms, term.term);
    }
    for token in token_surfaces(span) {
        if let Some(canonical) = canonical_developer_term(&token) {
            push_unique_term(&mut terms, canonical.to_string());
        }
    }
    terms
}

fn is_protected_vocab_item(term: &vocabulary::VocabTerm) -> bool {
    let term_type = term
        .term_type
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| vocabulary::classify_term_type(&term.term));
    let source = term.source.trim();
    source == "manual"
        || source == "starred"
        || matches!(
            term_type,
            "brand" | "acronym" | "proper_noun" | "code_identifier" | "phrase"
        )
}

fn push_unique_term(terms: &mut Vec<String>, term: String) {
    let norm = tier2_edit_policy::normalize_token(&term);
    if norm.is_empty()
        || terms
            .iter()
            .any(|existing| tier2_edit_policy::normalize_token(existing) == norm)
    {
        return;
    }
    terms.push(term);
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

/// True when a single token *looks like* a protected name / brand / acronym /
/// code identifier worth OFFERING to the user as a learnable term. A local
/// mirror of the control-plane's `looks_like_protected_target`, built only from
/// existing primitives so the local "Ask to learn" choice agrees with the
/// server's notion of a name. Rejects empty / common-word / dictionary /
/// numeric tokens, then requires a proper-noun signal (initial capital, a short
/// all-caps run, or a digit/dot). Used only to gate the new review-card offers;
/// it never auto-learns anything.
fn is_name_like_term(raw: &str) -> bool {
    let t = clean_surface(raw);
    if t.is_empty()
        || promotion_gate::is_common_word(&t)
        || crate::tier2::is_in_dictionary(&t)
        || promotion_gate::is_numeric_junk(&t)
    {
        return false;
    }
    let first_upper = t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    let all_caps =
        !letters.is_empty() && letters.len() <= 8 && letters.iter().all(|c| c.is_uppercase());
    let has_digit_dot = t.chars().any(|c| c.is_ascii_digit() || c == '.');
    first_upper || all_caps || has_digit_dot
}

/// True when a (possibly multi-word) corrected span is worth offering as a
/// learnable term: 1..=4 words and at least one token is name-like (a real
/// name/brand anchor like "Emiac" in "Emiac tech"). Because `is_name_like_term`
/// already excludes common / dictionary / numeric tokens, a span made entirely
/// of ordinary words ("the market") has no anchor and stays silent, while a
/// genuine multi-word name is surfaced.
fn name_like_span(raw: &str) -> bool {
    let words: Vec<&str> = raw.split_whitespace().collect();
    !words.is_empty() && words.len() <= 4 && words.iter().any(|&w| is_name_like_term(w))
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
    if alias_safety::is_common_alias_source(source) || promotion_gate::is_common_word(source) {
        return Some("source is a common word".to_string());
    }
    let source_norm = alias_safety::normalize_source(source);
    if crate::tier2::is_in_dictionary(&source_norm) {
        return Some("source is a dictionary word".to_string());
    }
    existing_protected_term_in_text(pool, user_id, source)
        .map(|term| format!("source already contains protected term {term:?}"))
}

fn unsafe_prompt_correction_source(
    pool: &crate::store::DbPool,
    user_id: &str,
    source: &str,
    corrected: &str,
) -> bool {
    if !alias_safety::is_common_alias_source(source) && !promotion_gate::is_common_word(source) {
        return false;
    }
    protected_vocab_lookup(pool, user_id, corrected).is_some()
        || matches!(
            vocabulary::classify_term_type(corrected),
            "brand" | "acronym" | "proper_noun" | "code_identifier"
        )
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
        review_session_id: None,
        learned: false,
        notify: false,
        promoted_count: 0,
        is_repeat: false,
        promoted_terms: vec![],
        learned_emails: vec![],
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
        crate::legacy_learning::enable_debug_legacy_writes_for_tests();
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
    fn capture_stale_window_covers_the_desktop_edit_watcher() {
        assert!(!capture_is_stale(120_000));
        assert!(!capture_is_stale(CAPTURE_STALE_MS));
        assert!(capture_is_stale(CAPTURE_STALE_MS + 1));
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
    fn deterministic_classifier_collapses_spaced_source_to_code_token() {
        let pool = mem_pool();
        let hunks = edit_diff::diff(
            "Yah n 10 ko achchhe se sun nahin pa raha hai",
            "Yah n 10 ko achchhe se sun nahin pa raha hai",
            "Yah n8n ko achchhe se sun nahin pa raha hai",
        );
        let changes = deterministic_classify_hunks(
            &hunks,
            "Yah n 10 ko achchhe se sun nahin pa raha hai",
            "Yah n 10 ko achchhe se sun nahin pa raha hai",
            "Yah n8n ko achchhe se sun nahin pa raha hai",
            &pool,
            "u1",
        );

        assert!(changes.iter().any(|change| {
            change.original == "n 10"
                && change.corrected == "n8n"
                && change.reason == ChangeReason::SttError
                && change.should_learn
        }));
    }

    #[test]
    fn local_review_candidate_preserves_spaced_n10_to_n8n() {
        let pool = mem_pool();
        let changes = vec![stt_change("n 10", "n8n")];
        let candidates = local_review_candidates_from_analyzer(
            &changes,
            &pool,
            "u1",
            "Yah n8n ko achchhe se sun nahin pa raha hai",
            "hinglish",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "n 10");
        assert_eq!(candidates[0].corrected, "n8n");
        assert_eq!(candidates[0].term_type, "code_identifier");
        assert_eq!(candidates[0].tag, "local_token_collapse");
    }

    #[test]
    fn local_review_candidate_normalizes_uppercase_source_n10() {
        let pool = mem_pool();
        let lower = local_review_candidates_from_analyzer(
            &[stt_change("n 10", "n8n")],
            &pool,
            "u1",
            "n8n ka workflow",
            "hinglish",
        );
        let upper = local_review_candidates_from_analyzer(
            &[stt_change("N 10", "n8n")],
            &pool,
            "u1",
            "n8n ka workflow",
            "hinglish",
        );

        let mut merged = lower.clone();
        let added = merge_review_candidates(&mut merged, upper);

        assert_eq!(lower.len(), 1);
        assert_eq!(added, 0, "N 10 and n 10 should dedupe to one alias");
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn server_candidates_keep_local_spaced_code_identifier() {
        let pool = mem_pool();
        let local = local_review_candidates_from_analyzer(
            &[stt_change("N 10", "n8n")],
            &pool,
            "u1",
            "n8n aur Kafka ka use karenge",
            "hinglish",
        );
        let mut server = vec![ReviewCandidate {
            original: "kaaf ka".to_string(),
            corrected: "Kafka".to_string(),
            term_type: "brand".to_string(),
            learnable: true,
            tag: "server".to_string(),
            context: None,
        }];

        let added = merge_review_candidates(&mut server, local);

        assert_eq!(added, 1);
        assert!(
            server
                .iter()
                .any(|candidate| { candidate.original == "N 10" && candidate.corrected == "n8n" })
        );
        assert!(
            server
                .iter()
                .any(|candidate| candidate.corrected == "Kafka")
        );
    }

    #[test]
    fn server_candidate_drops_context_wrapped_common_word_edit() {
        let candidates = sanitize_review_candidates(
            vec![ReviewCandidate {
                original: "Lark wiki two".to_string(),
                corrected: "Lark wiki too".to_string(),
                term_type: "proper_noun".to_string(),
                learnable: true,
                tag: "server_llm".to_string(),
                context: None,
            }],
            "Lark wiki too",
            "english",
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn scope_to_our_output_strips_pre_existing_text() {
        // Prefix only.
        assert_eq!(
            scope_to_our_output("Hello team. Isko EMIAC kar do.", Some("Hello team. ")),
            "Isko EMIAC kar do."
        );
        // Suffix only.
        assert_eq!(
            scope_to_our_output("Isko EMIAC kar do. Thanks.", Some(" Thanks.")),
            "Isko EMIAC kar do."
        );
        // Both sides (caret split the baseline "A. " + " B.").
        assert_eq!(
            scope_to_our_output("A. our text here B.", Some("A.  B.")),
            "our text here"
        );
        // Empty / None baseline → unchanged.
        assert_eq!(
            scope_to_our_output("just our text", Some("")),
            "just our text"
        );
        assert_eq!(scope_to_our_output("just our text", None), "just our text");
        // Baseline not cleanly present (surrounding text also edited) → full field.
        assert_eq!(
            scope_to_our_output("totally different", Some("nope prefix")),
            "totally different"
        );
    }

    #[test]
    fn surrounding_additions_are_continuations_not_corrections() {
        assert!(is_surrounding_continuation(
            "Please review the proposal.",
            "Please review the proposal. I will send the link next.",
        ));
        assert!(is_surrounding_continuation(
            "Please review the proposal.",
            "Context: Please review the proposal.",
        ));
        assert!(!is_surrounding_continuation(
            "Please review the CQLite proposal.",
            "Please review the SQLite proposal.",
        ));
        assert!(!is_surrounding_continuation(
            "Please review the proposal.",
            "Please carefully review the proposal.",
        ));
    }

    #[test]
    fn server_candidate_trims_context_wrapped_brand_edit() {
        let candidate = ReviewCandidate {
            original: "please kaafka".to_string(),
            corrected: "please Kafka".to_string(),
            term_type: "proper_noun".to_string(),
            learnable: true,
            tag: "server_llm".to_string(),
            context: None,
        };
        assert!(!promotion_gate::is_common_word("Kafka"));
        assert!(!alias_safety::is_common_alias_source("Kafka"));
        assert_eq!(vocabulary::classify_term_type("Kafka"), "proper_noun");
        match trim_unchanged_review_candidate_context(&candidate) {
            ReviewCandidateContextTrim::Trim(trimmed) => {
                assert_eq!(trimmed.original, "kaafka");
                assert_eq!(trimmed.corrected, "Kafka");
            }
            ReviewCandidateContextTrim::Drop => panic!("brand candidate should not be dropped"),
            ReviewCandidateContextTrim::Unchanged => panic!("brand candidate should be trimmed"),
        }

        let candidates =
            sanitize_review_candidates(vec![candidate], "please Kafka status bhejo", "english");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "kaafka");
        assert_eq!(candidates[0].corrected, "Kafka");
        assert_eq!(candidates[0].term_type, "proper_noun");
        assert!(candidates[0].tag.contains("trimmed"));
    }

    #[test]
    fn sanitize_drops_weak_partial_when_full_phrase_exists() {
        let candidates = sanitize_review_candidates(
            vec![
                ReviewCandidate {
                    original: "grass".to_string(),
                    corrected: "Postgres".to_string(),
                    term_type: "proper_noun".to_string(),
                    learnable: true,
                    tag: "server_llm".to_string(),
                    context: None,
                },
                ReviewCandidate {
                    original: "Post grass".to_string(),
                    corrected: "Postgres".to_string(),
                    term_type: "proper_noun".to_string(),
                    learnable: true,
                    tag: "local_token_collapse".to_string(),
                    context: None,
                },
            ],
            "Postgres migration check karo",
            "english",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "Post grass");
        assert_eq!(candidates[0].corrected, "Postgres");
    }

    #[test]
    fn safety_blocked_single_token_does_not_poison_phrase_review_candidate() {
        assert!(safety_block_should_suppress_candidate("cop", "cop"));
        assert!(safety_block_should_suppress_candidate("cop", "main cop"));
        assert!(safety_block_should_suppress_candidate(
            "main cop app",
            "main cop"
        ));
        assert!(!safety_block_should_suppress_candidate("main cop", "cop"));
    }

    #[test]
    fn local_review_candidate_keeps_main_cop_to_existing_macobs() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "Macobs", 1.0, "manual"));
        let candidates = local_review_candidates_from_analyzer(
            &[stt_change("main cop", "Macobs")],
            &pool,
            "u1",
            "Macobs onboarding flow is broken",
            "english",
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].original, "main cop");
        assert_eq!(candidates[0].corrected, "Macobs");
        assert_eq!(candidates[0].tag, "local_token_collapse");
    }

    #[test]
    fn sanitize_keeps_capitalized_partial_alias() {
        let candidates = sanitize_review_candidates(
            vec![
                ReviewCandidate {
                    original: "Zuki".to_string(),
                    corrected: "ZooKeeper".to_string(),
                    term_type: "brand".to_string(),
                    learnable: true,
                    tag: "local_deterministic".to_string(),
                    context: None,
                },
                ReviewCandidate {
                    original: "Zuki par".to_string(),
                    corrected: "ZooKeeper".to_string(),
                    term_type: "brand".to_string(),
                    learnable: true,
                    tag: "local_token_collapse".to_string(),
                    context: None,
                },
            ],
            "ZooKeeper status check karo",
            "english",
        );

        assert_eq!(candidates.len(), 2);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.original == "Zuki")
        );
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.original == "Zuki par")
        );
    }

    fn llm_candidate(
        source_span: &str,
        corrected_span: &str,
        edit_type: &str,
        confidence: f64,
    ) -> LlmEditCandidate {
        LlmEditCandidate {
            source_span: source_span.to_string(),
            corrected_span: corrected_span.to_string(),
            edit_type: edit_type.to_string(),
            reason: "stt_error".to_string(),
            should_learn: true,
            confidence,
        }
    }

    #[test]
    fn complex_interpreter_needed_for_insertions() {
        let hunks = edit_diff::diff("ka data bhejo", "ka data bhejo", "Macobs ka data bhejo");
        let det = deterministic_classify_hunks(
            &hunks,
            "ka data bhejo",
            "ka data bhejo",
            "Macobs ka data bhejo",
            &mem_pool(),
            "u1",
        );
        assert!(needs_complex_edit_interpreter(&hunks, &det));
    }

    #[test]
    fn complex_interpreter_not_needed_for_simple_learnable_replace() {
        let pool = mem_pool();
        let hunks = edit_diff::diff(
            "mecobs ka data bhejo",
            "mecobs ka data bhejo",
            "Macobs ka data bhejo",
        );
        let det = deterministic_classify_hunks(
            &hunks,
            "mecobs ka data bhejo",
            "mecobs ka data bhejo",
            "Macobs ka data bhejo",
            &pool,
            "u1",
        );
        assert!(!needs_complex_edit_interpreter(&hunks, &det));
    }

    #[test]
    fn normalized_phrase_matching_handles_multi_word_spans() {
        assert!(contains_normalized_phrase("n a ten ka workflow", "n a ten"));
        assert!(contains_normalized_phrase("Macobs ka data bhejo", "macobs"));
        assert!(!contains_normalized_phrase("Macobs ka data bhejo", "EMIAC"));
    }

    #[test]
    fn llm_candidate_accepts_backed_replace() {
        let pool = mem_pool();
        let transcript = "bimmicop ka data bhejo";
        let polished = "bimmicop ka data bhejo";
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff(transcript, polished, kept);
        let change = verified_llm_candidate_to_change(
            &llm_candidate("bimmicop", "Macobs", "replace", 0.91),
            transcript,
            polished,
            kept,
            &hunks,
            &pool,
            "u1",
        )
        .expect("backed replace should verify");

        assert_eq!(change.original, "bimmicop");
        assert_eq!(change.corrected, "Macobs");
        assert_eq!(change.reason, ChangeReason::SttError);
    }

    #[test]
    fn llm_candidate_rejects_invented_corrected_span() {
        let pool = mem_pool();
        let transcript = "bimmicop ka data bhejo";
        let polished = "bimmicop ka data bhejo";
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff(transcript, polished, kept);
        let change = verified_llm_candidate_to_change(
            &llm_candidate("bimmicop", "EMIAC", "replace", 0.95),
            transcript,
            polished,
            kept,
            &hunks,
            &pool,
            "u1",
        );

        assert!(change.is_none());
    }

    #[test]
    fn llm_candidate_rejects_invented_source_span() {
        let pool = mem_pool();
        let transcript = "bimmicop ka data bhejo";
        let polished = "bimmicop ka data bhejo";
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff(transcript, polished, kept);
        let change = verified_llm_candidate_to_change(
            &llm_candidate("kaisa", "Macobs", "replace", 0.95),
            transcript,
            polished,
            kept,
            &hunks,
            &pool,
            "u1",
        );

        assert!(change.is_none());
    }

    #[test]
    fn llm_candidate_accepts_strong_missing_term_insert() {
        let pool = mem_pool();
        let transcript = "ka data bhejo";
        let polished = "ka data bhejo";
        let kept = "Macobs ka data bhejo";
        let hunks = edit_diff::diff(transcript, polished, kept);
        let change = verified_llm_candidate_to_change(
            &llm_candidate("", "Macobs", "insert", 0.92),
            transcript,
            polished,
            kept,
            &hunks,
            &pool,
            "u1",
        )
        .expect("strong missing protected term should verify");

        assert_eq!(change.original, "");
        assert_eq!(change.corrected, "Macobs");
        assert_eq!(change.reason, ChangeReason::SttError);
    }

    #[test]
    fn llm_candidate_rejects_weak_insert_target() {
        let pool = mem_pool();
        let transcript = "ka data bhejo";
        let polished = "ka data bhejo";
        let kept = "randomword ka data bhejo";
        let hunks = edit_diff::diff(transcript, polished, kept);
        let change = verified_llm_candidate_to_change(
            &llm_candidate("", "randomword", "insert", 0.95),
            transcript,
            polished,
            kept,
            &hunks,
            &pool,
            "u1",
        );

        assert!(change.is_none());
    }

    #[test]
    fn protected_insert_extractor_splits_inserted_terms() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "n8n", 1.0, "manual"));
        let kept = "n8n EMIAC ka proprietary model hai";
        let hunks = edit_diff::diff("ka proprietary model hai", "ka proprietary model hai", kept);
        let changes = protected_insert_changes_from_hunks(&hunks, kept, &pool, "u1");

        assert_eq!(changes.len(), 2);
        assert!(changes.iter().any(|change| {
            change.original.is_empty()
                && change.corrected == "n8n"
                && change.reason == ChangeReason::SttError
                && change.should_learn
        }));
        assert!(changes.iter().any(|change| {
            change.original.is_empty()
                && change.corrected == "EMIAC"
                && change.reason == ChangeReason::SttError
                && change.should_learn
        }));
    }

    #[test]
    fn protected_insert_extractor_strips_filler_from_common_source_hunk() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "Macobs", 1.0, "manual"));
        let kept = "Macobs mein data bhejo";
        let hunks = edit_diff::diff("mujhe data bhejo", "Mujhe data bhejo", kept);
        let changes = protected_insert_changes_from_hunks(&hunks, kept, &pool, "u1");

        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].original, "");
        assert_eq!(changes[0].corrected, "Macobs");
        assert_eq!(changes[0].reason, ChangeReason::SttError);
        assert!(changes[0].should_learn);
    }

    #[test]
    fn split_known_term_hunk_separates_compound_distortions() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "GraphQL", 1.0, "manual"));
        assert!(vocabulary::upsert(&pool, "u1", "Supabase", 1.0, "manual"));
        let changes = split_known_term_hunk(
            &[
                "graph".to_string(),
                "cute".to_string(),
                "super".to_string(),
                "base".to_string(),
            ],
            &["GraphQL".to_string(), "Supabase".to_string()],
            "GraphQL Supabase ka auth flow",
            &pool,
            "u1",
        )
        .expect("known terms should split");

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].original, "graph cute");
        assert_eq!(changes[0].corrected, "GraphQL");
        assert_eq!(changes[1].original, "super base");
        assert_eq!(changes[1].corrected, "Supabase");
    }

    #[test]
    fn refine_pair_strips_common_filler_from_corrected_span() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "Macobs", 1.0, "manual"));
        let pair = DeterministicEditPair {
            variant_form: "mujhe".to_string(),
            correct_form: "Macobs mein".to_string(),
            edit_type: "replace".to_string(),
            left_context: vec![],
            right_context: vec![],
        };

        let refined = refine_stt_pair_for_learning(pair, &pool, "u1", "Macobs mein data bhejo")
            .expect("protected term should survive");

        assert_eq!(refined.variant_form, "");
        assert_eq!(refined.correct_form, "Macobs");
        assert_eq!(refined.edit_type, "insert");
        assert_eq!(refined.right_context, vec!["mein", "data", "bhejo"]);
    }

    #[test]
    fn refine_pair_trims_common_edge_from_source_span() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "Macobs", 1.0, "manual"));
        let pair = DeterministicEditPair {
            variant_form: "main Gops".to_string(),
            correct_form: "Macobs".to_string(),
            edit_type: "replace".to_string(),
            left_context: vec![],
            right_context: vec![],
        };

        let refined =
            refine_stt_pair_for_learning(pair, &pool, "u1", "hello bhai Macobs ka IPO aa gaya kya")
                .expect("protected term should survive");

        assert_eq!(refined.variant_form, "Gops");
        assert_eq!(refined.correct_form, "Macobs");
        assert_eq!(refined.edit_type, "replace");
        assert_eq!(refined.left_context, vec!["hello", "bhai"]);
        assert_eq!(refined.right_context, vec!["ka", "IPO", "aa"]);
    }

    #[test]
    fn refine_pair_preserves_multi_word_protected_term() {
        let pool = mem_pool();
        assert!(vocabulary::upsert(&pool, "u1", "Urban Aura", 1.0, "manual"));
        let pair = DeterministicEditPair {
            variant_form: "urbanora".to_string(),
            correct_form: "Urban Aura mein".to_string(),
            edit_type: "replace".to_string(),
            left_context: vec![],
            right_context: vec![],
        };

        let refined =
            refine_stt_pair_for_learning(pair, &pool, "u1", "Urban Aura mein product launch karna")
                .expect("protected phrase should be extracted");

        assert_eq!(refined.variant_form, "");
        assert_eq!(refined.correct_form, "Urban Aura");
        assert_eq!(refined.edit_type, "insert");
        assert_eq!(refined.right_context, vec!["mein", "product", "launch"]);
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
