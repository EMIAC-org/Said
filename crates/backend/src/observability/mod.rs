//! Non-blocking observability pipeline for control-plane dictation history.

pub mod outbox;
pub mod uploader;

pub use outbox::{
    AliasBatchPayload, AliasLearnItem, DictationPatchPayload, DictationUpsertPayload,
    RecordingObservabilityExtras, after_recording_insert, enqueue_alias_batch,
    enqueue_dictation_patch, enqueue_dictation_upsert, should_enqueue,
};

use crate::AppState;
use crate::llm::analyzer::{AnalyzedChange, ChangeReason};
use serde_json::{Value, json};

pub struct ClassifyObservabilityInput<'a> {
    pub recording_id: &'a str,
    pub user_kept: &'a str,
    pub capture_method: &'a str,
    pub overall_class: &'a str,
    pub changes: &'a [AnalyzedChange],
    pub review_candidates: &'a [Value],
    pub promoted_terms: &'a [String],
}

pub fn observability_extras(client_run_id: Option<&str>) -> RecordingObservabilityExtras {
    RecordingObservabilityExtras {
        client_run_id: client_run_id.map(str::to_string),
        device_id: Some(said_core::paths::device_id()),
        platform: Some(std::env::consts::OS.to_string()),
        app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    }
}

pub fn schedule_classify_observability(state: &AppState, input: ClassifyObservabilityInput<'_>) {
    if !should_enqueue(&state.pool, &state.default_user_id) {
        return;
    }

    let change_items: Vec<Value> = input
        .changes
        .iter()
        .map(|c| {
            json!({
                "type": c.reason.as_str(),
                "from": c.original,
                "to": c.corrected,
                "should_learn": c.should_learn,
                "confidence": c.confidence,
            })
        })
        .collect();

    let promoted_aliases: Vec<Value> = input
        .changes
        .iter()
        .filter(|c| matches!(c.reason, ChangeReason::SttError | ChangeReason::PolishError))
        .filter(|c| c.should_learn)
        .map(|c| json!({ "heard": c.original, "correct": c.corrected }))
        .collect();

    let feedback = json!({
        "class": input.overall_class,
        "capture_method": input.capture_method,
        "change_count": input.changes.len(),
        "changes": change_items,
        "review_candidates": input.review_candidates,
        "promoted_aliases": promoted_aliases,
        "promoted_terms": input.promoted_terms,
    });

    let pool = state.pool.clone();
    let user_id = state.default_user_id.clone();
    let http = state.http_client.clone();
    let recording_id = input.recording_id.to_string();
    let user_kept = input.user_kept.to_string();

    let alias_items: Vec<AliasLearnItem> = input
        .changes
        .iter()
        .filter(|c| matches!(c.reason, ChangeReason::SttError))
        .filter(|c| c.should_learn && !c.original.trim().is_empty())
        .map(|c| AliasLearnItem {
            heard: c.original.clone(),
            correct: c.corrected.clone(),
            source: "classify".into(),
            safety: None,
            recording_id: Some(recording_id.clone()),
        })
        .collect();

    tokio::spawn(async move {
        let patch = DictationPatchPayload {
            recording_id,
            final_text: Some(user_kept),
            edit_feedback_json: Some(feedback),
        };
        if let Err(e) = enqueue_dictation_patch(&pool, &user_id, patch) {
            tracing::warn!("[observability] classify patch enqueue failed: {e}");
        }
        if !alias_items.is_empty() {
            let _ = enqueue_alias_batch(&pool, &user_id, AliasBatchPayload { items: alias_items });
        }
        uploader::maybe_upload_after_enqueue(&pool, &user_id, &http);
    });
}
