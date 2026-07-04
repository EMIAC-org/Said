//! DeepSeek profile updater — request/response types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::routes::runtime::UserEditSpan;

/// Client payload for `POST /v1/runtime/profile/learn-from-edit`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LearnFromEditRequest {
    pub edit_event_id: String,
    #[serde(default)]
    pub recording_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    #[serde(default)]
    pub client_run_id: Option<String>,
    #[serde(default)]
    pub raw_transcript: Option<String>,
    pub ai_output: String,
    pub user_kept: String,
    #[serde(default)]
    pub target_app: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub output_language: Option<String>,
    #[serde(default)]
    pub model_used: Option<String>,
    #[serde(default)]
    pub capture_confidence: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearnFromEditResponse {
    pub job_id: Uuid,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileUpdateRequest {
    pub schema_version: i32,
    pub request_id: Uuid,
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub edit: ProfileUpdateEdit,
    pub current_profile: ProfileUpdateCurrentProfile,
    pub policy: ProfileUpdatePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileUpdateEdit {
    pub edit_event_id: String,
    pub recording_id: Option<String>,
    pub run_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_run_id: Option<String>,
    pub captured_at: DateTime<Utc>,
    pub raw_transcript: Option<String>,
    pub ai_output: String,
    pub user_kept: String,
    pub edit_spans: Vec<UserEditSpan>,
    pub target_app: Option<String>,
    pub platform: Option<String>,
    pub output_language: Option<String>,
    pub model_used: Option<String>,
    pub capture_confidence: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileUpdateCurrentProfile {
    pub version: i64,
    pub schema_version: i32,
    pub profile_json: Value,
    pub profile_markdown: String,
    pub alias_summary: Vec<AliasSummaryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasSummaryEntry {
    pub source_phrase: String,
    pub canonical_phrase: String,
    pub status: String,
    pub evidence_count: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileUpdatePolicy {
    pub max_markdown_bytes: usize,
    pub max_json_bytes: usize,
    pub alias_min_confidence_candidate: f64,
    pub alias_min_confidence_active: f64,
    pub alias_min_evidence_active: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DeepSeekClassification {
    SttError,
    PolishError,
    StylePreference,
    DomainTerm,
    UserRewrite,
    NoLearning,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeepSeekProfileUpdateResponse {
    pub schema_version: i32,
    pub classification: DeepSeekClassification,
    pub confidence: f64,
    #[serde(default)]
    pub profile_patch: DeepSeekProfilePatch,
    #[serde(default)]
    pub alias_proposals: Vec<DeepSeekAliasProposal>,
    #[serde(default)]
    pub profile_markdown_patch: DeepSeekMarkdownPatch,
    #[serde(default)]
    pub review_required: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DeepSeekProfilePatch {
    #[serde(default)]
    pub user_background: Option<PatchUserBackground>,
    #[serde(default)]
    pub add_focus_areas: Vec<PatchFocusArea>,
    #[serde(default)]
    pub add_speech_patterns: Vec<PatchSpeechPattern>,
    #[serde(default)]
    pub add_recent_context: Vec<PatchRecentContext>,
    #[serde(default)]
    pub add_domains: Vec<PatchDomain>,
    #[serde(default)]
    pub add_stable_terms: Vec<PatchStableTerm>,
    #[serde(default)]
    pub add_stt_confusions: Vec<PatchSttConfusion>,
    #[serde(default)]
    pub add_negative_rules: Vec<PatchNegativeRule>,
    #[serde(default)]
    pub style_updates: Vec<PatchStyleUpdate>,
    #[serde(default)]
    pub remove_stable_terms: Vec<String>,
    #[serde(default)]
    pub demote_confusions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchUserBackground {
    pub summary: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchFocusArea {
    pub area: String,
    pub weight: f64,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchSpeechPattern {
    pub pattern: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchRecentContext {
    pub note: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchDomain {
    pub name: String,
    pub weight: f64,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchStableTerm {
    pub term: String,
    pub term_type: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchSttConfusion {
    pub heard: String,
    pub intended: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchNegativeRule {
    pub rule: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchStyleUpdate {
    pub category: String,
    pub preference: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeepSeekAliasProposal {
    pub source_phrase: String,
    pub canonical_phrase: String,
    pub term_type: String,
    pub proposal_status: String,
    pub confidence: f64,
    pub evidence_count_delta: i32,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct DeepSeekMarkdownPatch {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub markdown: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearnAuditPayload {
    pub edit_event_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepseek_classification: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepseek_confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepseek_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validator_decision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validator_reasons: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias_changes: Option<Vec<AliasChangeRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_json_delta_summary: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deepseek_request_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadow_would_apply: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AliasChangeRecord {
    pub source_phrase: String,
    pub canonical_phrase: String,
    pub from_status: String,
    pub to_status: String,
}

// --- Batched per-user profiling + KB run (deepseek-v4-flash over a bucket window) ---

/// One dictation in the analyzed window, as sent to DeepSeek.
#[derive(Debug, Clone, Serialize)]
pub struct BatchRunInput {
    pub was_edited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_transcript: Option<String>,
    pub polished_output: String,
    pub final_text: String,
}

/// DeepSeek's per-run classification of an unknown app into the fixed bucket enum.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AppBucketSuggestion {
    pub app_key: String,
    pub bucket: String,
    #[serde(default)]
    pub confidence: f64,
}

/// DeepSeek's structured output for one bucket window: per-bucket style + global KB
/// deltas + app-bucket classifications. Reuses the per-edit patch sub-structs.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BatchProfileResponse {
    /// The model's own recommendation to apply (gated further by `confidence`).
    #[serde(default)]
    pub apply: bool,
    #[serde(default)]
    pub confidence: f64,
    /// Per-bucket style (-> the bucket overlay).
    #[serde(default)]
    pub style_updates: Vec<PatchStyleUpdate>,
    #[serde(default)]
    pub speech_patterns: Vec<PatchSpeechPattern>,
    /// Global identity / KB (-> runtime_user_profiles, bucket-invariant).
    #[serde(default)]
    pub user_background: Option<PatchUserBackground>,
    #[serde(default)]
    pub add_domains: Vec<PatchDomain>,
    #[serde(default)]
    pub add_focus_areas: Vec<PatchFocusArea>,
    /// Classifications for apps not yet in the static/agent bucket map.
    #[serde(default)]
    pub app_bucket_suggestions: Vec<AppBucketSuggestion>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LearnJobRow {
    pub id: Uuid,
    pub account_id: Uuid,
    pub org_scope: Uuid,
    pub edit_event_id: String,
    pub status: String,
    pub request_json: Value,
    pub response_json: Option<Value>,
    pub from_version: i64,
    pub to_version: Option<i64>,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_payload_includes_run_link_fields() {
        let payload = LearnAuditPayload {
            edit_event_id: "evt-1".into(),
            recording_id: Some("rec-1".into()),
            client_run_id: Some("desktop-run-abc".into()),
            run_id: Some(Uuid::nil()),
            job_id: Some(Uuid::nil()),
            deepseek_classification: None,
            deepseek_confidence: None,
            deepseek_reason: None,
            validator_decision: Some("queued".into()),
            validator_reasons: None,
            alias_changes: None,
            profile_json_delta_summary: None,
            deepseek_request_id: None,
            latency_ms: None,
            shadow_would_apply: None,
        };
        let value = serde_json::to_value(&payload).expect("serialize");
        assert_eq!(value["client_run_id"], "desktop-run-abc");
        assert_eq!(value["run_id"], Uuid::nil().to_string());
    }
}
